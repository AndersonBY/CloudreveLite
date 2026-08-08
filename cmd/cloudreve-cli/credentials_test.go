package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/zalando/go-keyring"
)

type memoryCredentialStore struct {
	values map[string]string
	err    error
}

func (s *memoryCredentialStore) Set(service, account, secret string) error {
	if s.err != nil {
		return s.err
	}
	s.values[service+"/"+account] = secret
	return nil
}

func (s *memoryCredentialStore) Get(service, account string) (string, error) {
	if s.err != nil {
		return "", s.err
	}
	secret, ok := s.values[service+"/"+account]
	if !ok {
		return "", keyring.ErrNotFound
	}
	return secret, nil
}

func (s *memoryCredentialStore) Delete(service, account string) error {
	delete(s.values, service+"/"+account)
	return nil
}

func useMemoryCredentialStore(t *testing.T) *memoryCredentialStore {
	t.Helper()
	previous := cliCredentialStore
	store := &memoryCredentialStore{values: make(map[string]string)}
	cliCredentialStore = store
	t.Cleanup(func() { cliCredentialStore = previous })
	t.Setenv("CLOUDREVE_CONFIG_DIR", t.TempDir())
	t.Setenv("CLOUDREVE_SERVER", "")
	t.Setenv("CLOUDREVE_API_KEY", "")
	t.Setenv("CLOUDREVE_TOKEN", "")
	return store
}

func TestConnectStoresAPIKeyWithoutPrintingIt(t *testing.T) {
	store := useMemoryCredentialStore(t)
	const apiKey = "crl_abcdefghijkl_0123456789012345678901234567890123456789"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/api/v4/session/api-key/exchange" {
			t.Fatalf("request = %s %s", r.Method, r.URL.Path)
		}
		var body map[string]string
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatal(err)
		}
		if body["code"] != "crp_pairing" {
			t.Fatalf("pairing body = %#v", body)
		}
		writeEnvelope(t, w, 0, map[string]any{
			"id":         "abcdefghijkl",
			"name":       "Browser setup",
			"key":        apiKey,
			"expires_at": "2027-01-01T00:00:00Z",
		})
	}))
	defer server.Close()

	var stdout bytes.Buffer
	root := newRootCommand(strings.NewReader(""), &stdout, &bytes.Buffer{})
	root.SetArgs([]string{"--server", server.URL, "connect", "--pairing-code", "crp_pairing"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
	requireOutput := stdout.String()
	if strings.Contains(requireOutput, apiKey) || !strings.Contains(requireOutput, `"connected": true`) {
		t.Fatalf("connect output = %s", requireOutput)
	}
	stored, err := store.Get(credentialService, credentialAccount(server.URL))
	if err != nil || stored != apiKey {
		t.Fatalf("stored credential = %q, err = %v", stored, err)
	}
}

func TestAuthenticatedCommandUsesSavedConnection(t *testing.T) {
	store := useMemoryCredentialStore(t)
	const apiKey = "crl_abcdefghijkl_0123456789012345678901234567890123456789"
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer "+apiKey {
			t.Fatalf("authorization = %q", r.Header.Get("Authorization"))
		}
		writeEnvelope(t, w, 0, map[string]string{"nickname": "agent"})
	}))
	defer server.Close()
	if err := saveConnection(server.URL, apiKey); err != nil {
		t.Fatal(err)
	}

	var stdout bytes.Buffer
	root := newRootCommand(strings.NewReader(""), &stdout, &bytes.Buffer{})
	root.SetArgs([]string{"me"})
	if err := root.Execute(); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(stdout.String(), `"nickname": "agent"`) {
		t.Fatalf("output = %s", stdout.String())
	}
	if len(store.values) != 1 {
		t.Fatalf("credential count = %d", len(store.values))
	}
}

func TestConnectFailsClosedWhenCredentialManagerIsUnavailable(t *testing.T) {
	store := useMemoryCredentialStore(t)
	store.err = errors.New("credential manager unavailable")
	if err := saveConnection("https://cloud.example.com", "crl_key"); err == nil || !strings.Contains(err.Error(), "credential manager") {
		t.Fatalf("expected credential manager error, got %v", err)
	}
}
