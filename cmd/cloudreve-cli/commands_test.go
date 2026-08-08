package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestLoginCompletesTOTPFromEnvironment(t *testing.T) {
	t.Setenv("CLOUDREVE_PASSWORD", "correct horse battery staple")
	t.Setenv("CLOUDREVE_OTP", "123456")
	t.Setenv("CLOUDREVE_TOKEN", "")

	requestCount := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requestCount++
		switch r.URL.Path {
		case "/api/v4/session/token":
			var body map[string]string
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatal(err)
			}
			if body["email"] != "agent@example.com" || body["password"] != "correct horse battery staple" {
				t.Fatalf("login body = %#v", body)
			}
			writeEnvelope(t, w, 203, "totp-session")
		case "/api/v4/session/token/2fa":
			var body map[string]string
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatal(err)
			}
			if body["session_id"] != "totp-session" || body["otp"] != "123456" {
				t.Fatalf("2FA body = %#v", body)
			}
			writeEnvelope(t, w, 0, map[string]any{"token": map[string]string{"access_token": "access"}})
		default:
			t.Fatalf("unexpected path %s", r.URL.Path)
		}
	}))
	defer server.Close()

	var stdout bytes.Buffer
	root := newRootCommand(strings.NewReader(""), &stdout, &bytes.Buffer{})
	root.SetArgs([]string{"--server", server.URL, "login", "agent@example.com"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
	if requestCount != 2 || !strings.Contains(stdout.String(), `"access_token": "access"`) {
		t.Fatalf("requests = %d, output = %s", requestCount, stdout.String())
	}
}

func TestLoginDoesNotAcceptPasswordArgument(t *testing.T) {
	t.Setenv("CLOUDREVE_PASSWORD", "secret")
	root := newRootCommand(strings.NewReader(""), &bytes.Buffer{}, &bytes.Buffer{})
	root.SetArgs([]string{"--server", "http://127.0.0.1:1", "login", "agent@example.com", "secret"})
	if err := root.Execute(); err == nil || !strings.Contains(err.Error(), "accepts 1 arg") {
		t.Fatalf("expected positional password rejection, got %v", err)
	}
}

func TestShareCreateUsesLiteDefaults(t *testing.T) {
	t.Setenv("CLOUDREVE_TOKEN", "access")
	t.Setenv("CLOUDREVE_SHARE_PASSWORD", "")
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPut || r.URL.Path != "/api/v4/share" {
			t.Fatalf("request = %s %s", r.Method, r.URL.Path)
		}
		var body struct {
			URI       string `json:"uri"`
			Downloads int    `json:"downloads"`
			Expire    int    `json:"expire"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body.URI != "cloudreve://my/file.txt" || body.Downloads != 10 || body.Expire != 7*24*60*60 {
			t.Fatalf("share body = %#v", body)
		}
		writeEnvelope(t, w, 0, "https://share.example/s/abc")
	}))
	defer server.Close()

	var stdout bytes.Buffer
	root := newRootCommand(strings.NewReader(""), &stdout, &bytes.Buffer{})
	root.SetArgs([]string{"--server", server.URL, "share", "create", "cloudreve://my/file.txt"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(stdout.String(), "share.example") {
		t.Fatalf("output = %s", stdout.String())
	}
}

func TestShareListSendsRequiredCursorPagination(t *testing.T) {
	t.Setenv("CLOUDREVE_TOKEN", "access")
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v4/share" || r.URL.Query().Get("page_size") != "50" || r.URL.Query().Get("next_page_token") != "next" {
			t.Fatalf("request URL = %s", r.URL.String())
		}
		writeEnvelope(t, w, 0, map[string]any{"shares": []any{}})
	}))
	defer server.Close()

	root := newRootCommand(strings.NewReader(""), &bytes.Buffer{}, &bytes.Buffer{})
	root.SetArgs([]string{"--server", server.URL, "share", "list", "--page-token", "next"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
}
