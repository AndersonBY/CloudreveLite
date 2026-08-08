package main

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestClientAuthenticationAndEnvelope(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/v4/user/me" {
			t.Errorf("path = %s", r.URL.Path)
		}
		if got := r.Header.Get("Authorization"); got != "Bearer secret" {
			t.Errorf("authorization = %q", got)
		}
		writeEnvelope(t, w, 0, map[string]string{"nickname": "agent"})
	}))
	defer server.Close()

	api, err := newClient(server.URL, "secret", false)
	if err != nil {
		t.Fatal(err)
	}
	raw, err := api.request(context.Background(), http.MethodGet, "/user/me", nil)
	if err != nil {
		t.Fatal(err)
	}
	if string(raw) != `{"nickname":"agent"}` {
		t.Fatalf("data = %s", raw)
	}
}

func TestClientRejectsRemotePlainHTTP(t *testing.T) {
	if _, err := newClient("http://example.com", "secret", false); err == nil || !strings.Contains(err.Error(), "plain HTTP") {
		t.Fatalf("expected plain HTTP rejection, got %v", err)
	}
	if _, err := newClient("http://example.com", "secret", true); err != nil {
		t.Fatalf("allow HTTP: %v", err)
	}
}

func TestClientReturnsCloudreveError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{
			"code":           40001,
			"msg":            "bad request",
			"correlation_id": "request-1",
		})
	}))
	defer server.Close()

	api, err := newClient(server.URL, "", false)
	if err != nil {
		t.Fatal(err)
	}
	_, err = api.request(context.Background(), http.MethodGet, "/file", nil)
	if err == nil || !strings.Contains(err.Error(), "request-1") {
		t.Fatalf("expected correlated API error, got %v", err)
	}
}

func writeEnvelope(t *testing.T, w http.ResponseWriter, code int, data any) {
	t.Helper()
	w.Header().Set("Content-Type", "application/json")
	if err := json.NewEncoder(w).Encode(map[string]any{"code": code, "data": data, "msg": ""}); err != nil {
		t.Fatal(err)
	}
}
