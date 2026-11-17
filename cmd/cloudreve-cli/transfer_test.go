package main

import (
	"context"
	"encoding/json"
	"encoding/xml"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
)

func TestUploadFileDirectOSSAndS3(t *testing.T) {
	for _, policyType := range []string{"oss", "s3"} {
		t.Run(policyType, func(t *testing.T) {
			var mutex sync.Mutex
			chunks := map[string]string{}
			callbackCalled := false
			cancelCalled := false

			var connections atomic.Int32
			storage := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if got := r.Header.Get("Authorization"); got != "" {
					t.Errorf("Cloudreve authorization leaked to storage: %q", got)
				}
				switch r.URL.Path {
				case "/part/1", "/part/2":
					body, err := io.ReadAll(r.Body)
					if err != nil {
						t.Fatal(err)
					}
					mutex.Lock()
					chunks[r.URL.Path] = string(body)
					mutex.Unlock()
					w.Header().Set("ETag", `"etag-`+r.URL.Path[len(r.URL.Path)-1:]+`"`)
					w.WriteHeader(http.StatusOK)
				case "/complete":
					if policyType == "oss" {
						if r.Header.Get("x-oss-complete-all") != "yes" || r.Header.Get("x-oss-forbid-overwrite") != "true" {
							t.Errorf("missing OSS completion headers")
						}
						if body, _ := io.ReadAll(r.Body); len(body) != 0 {
							t.Errorf("OSS completion body = %q", body)
						}
					} else {
						var completed completeMultipartUpload
						if err := xml.NewDecoder(r.Body).Decode(&completed); err != nil {
							t.Fatal(err)
						}
						if len(completed.Parts) != 2 || completed.Parts[1].ETag != `"etag-2"` {
							t.Fatalf("completed parts = %#v", completed.Parts)
						}
					}
					w.WriteHeader(http.StatusOK)
					_, _ = w.Write([]byte("<CompleteMultipartUploadResult/>"))
				default:
					t.Fatalf("unexpected storage path %s", r.URL.Path)
				}
			}))
			storage.Config.ConnState = func(_ net.Conn, state http.ConnState) {
				if state == http.StateNew {
					connections.Add(1)
				}
			}
			storage.Start()
			defer storage.Close()

			apiServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if got := r.Header.Get("Authorization"); got != "Bearer access" {
					t.Errorf("API authorization = %q", got)
				}
				switch {
				case r.Method == http.MethodPut && r.URL.Path == "/api/v4/file/upload":
					writeEnvelope(t, w, 0, map[string]any{
						"session_id":      "session-1",
						"chunk_size":      3,
						"upload_urls":     []string{storage.URL + "/part/1", storage.URL + "/part/2"},
						"completeURL":     storage.URL + "/complete",
						"uri":             "cloudreve://my/result.txt",
						"callback_secret": "callback-key",
						"storage_policy":  map[string]any{"type": policyType, "relay": false},
					})
				case r.Method == http.MethodGet && r.URL.Path == "/api/v4/callback/s3/session-1/callback-key":
					callbackCalled = true
					writeEnvelope(t, w, 0, nil)
				case r.Method == http.MethodGet && r.URL.Path == "/api/v4/file/info":
					writeEnvelope(t, w, 0, map[string]any{"name": "result.txt", "size": 6})
				case r.Method == http.MethodDelete && r.URL.Path == "/api/v4/file/upload":
					cancelCalled = true
					writeEnvelope(t, w, 0, nil)
				default:
					t.Fatalf("unexpected API request %s %s", r.Method, r.URL.Path)
				}
			}))
			defer apiServer.Close()

			api, err := newClient(apiServer.URL, "access", false)
			if err != nil {
				t.Fatal(err)
			}
			localPath := filepath.Join(t.TempDir(), "result.txt")
			if err := os.WriteFile(localPath, []byte("abcdef"), 0o600); err != nil {
				t.Fatal(err)
			}
			result, err := uploadFile(context.Background(), api, localPath, "cloudreve://my/", uploadOptions{})
			if err != nil {
				t.Fatal(err)
			}
			if !strings.Contains(string(result), `"size":6`) {
				t.Fatalf("result = %s", result)
			}
			if chunks["/part/1"] != "abc" || chunks["/part/2"] != "def" {
				t.Fatalf("chunks = %#v", chunks)
			}
			if callbackCalled != (policyType == "s3") {
				t.Fatalf("callback called = %v", callbackCalled)
			}
			if cancelCalled {
				t.Fatal("completed upload was canceled")
			}
			if got := connections.Load(); got != 1 {
				t.Fatalf("parts and completion used %d storage connections, want 1", got)
			}
		})
	}
}

func TestUploadFailureCancelsSession(t *testing.T) {
	cancelCalled := false
	storage := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "bad signature", http.StatusForbidden)
	}))
	defer storage.Close()

	apiServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodPut:
			writeEnvelope(t, w, 0, map[string]any{
				"session_id":      "session-1",
				"chunk_size":      3,
				"upload_urls":     []string{storage.URL + "/part"},
				"completeURL":     storage.URL + "/complete",
				"uri":             "cloudreve://my/result.txt",
				"callback_secret": "callback-key",
				"storage_policy":  map[string]any{"type": "oss", "relay": false},
			})
		case http.MethodDelete:
			cancelCalled = true
			writeEnvelope(t, w, 0, nil)
		default:
			t.Fatalf("unexpected request %s", r.Method)
		}
	}))
	defer apiServer.Close()

	api, err := newClient(apiServer.URL, "access", false)
	if err != nil {
		t.Fatal(err)
	}
	localPath := filepath.Join(t.TempDir(), "result.txt")
	if err := os.WriteFile(localPath, []byte("abc"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := uploadFile(context.Background(), api, localPath, "cloudreve://my/result.txt", uploadOptions{}); err == nil {
		t.Fatal("expected upload error")
	}
	if !cancelCalled {
		t.Fatal("failed upload session was not canceled")
	}
}

func TestDownloadDoesNotLeakAuthorization(t *testing.T) {
	storage := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.Header.Get("Authorization"); got != "" {
			t.Fatalf("Cloudreve authorization leaked to download: %q", got)
		}
		_, _ = w.Write([]byte("downloaded"))
	}))
	defer storage.Close()

	apiServer := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if got := r.Header.Get("Authorization"); got != "Bearer access" {
			t.Fatalf("API authorization = %q", got)
		}
		writeEnvelope(t, w, 0, map[string]any{"urls": []map[string]string{{"Url": storage.URL + "/file"}}})
	}))
	defer apiServer.Close()

	api, err := newClient(apiServer.URL, "access", false)
	if err != nil {
		t.Fatal(err)
	}
	localPath := filepath.Join(t.TempDir(), "download.txt")
	result, err := downloadFile(context.Background(), api, "cloudreve://my/download.txt", localPath, false)
	if err != nil {
		t.Fatal(err)
	}
	content, err := os.ReadFile(localPath)
	if err != nil {
		t.Fatal(err)
	}
	if string(content) != "downloaded" || result.Bytes != int64(len(content)) {
		t.Fatalf("content = %q, result = %#v", content, result)
	}
}

func TestResolveDestinationURI(t *testing.T) {
	got, err := resolveDestinationURI("cloudreve://my/results/", "a b.txt")
	if err != nil {
		t.Fatal(err)
	}
	if got != "cloudreve://my/results/a%20b.txt" {
		t.Fatalf("destination = %q", got)
	}
}

func TestUploadSessionJSONContract(t *testing.T) {
	var session uploadSession
	if err := json.Unmarshal([]byte(`{"completeURL":"complete","storage_policy":{"type":"oss"}}`), &session); err != nil {
		t.Fatal(err)
	}
	if session.CompleteURL != "complete" || session.StoragePolicy.Type != "oss" {
		t.Fatalf("session = %#v", session)
	}
}
