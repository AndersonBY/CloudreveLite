package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

const maxAPIResponseSize = 8 << 20

type client struct {
	base         *url.URL
	credential   string
	apiHTTP      *http.Client
	transferHTTP *http.Client
}

type envelope struct {
	Code          int             `json:"code"`
	Data          json.RawMessage `json:"data"`
	Msg           string          `json:"msg"`
	Err           string          `json:"error"`
	CorrelationID string          `json:"correlation_id"`
}

type apiError struct {
	Code          int
	Message       string
	Detail        string
	CorrelationID string
}

func (e *apiError) Error() string {
	message := strings.TrimSpace(e.Message)
	detail := strings.TrimSpace(e.Detail)
	if message == "" {
		message = "Cloudreve request failed"
	}
	if detail != "" && detail != message {
		message += ": " + detail
	}
	if e.CorrelationID != "" {
		message += " (correlation_id: " + e.CorrelationID + ")"
	}
	return fmt.Sprintf("Cloudreve error %d: %s", e.Code, message)
}

func newClient(server, credential string, allowHTTP bool) (*client, error) {
	base, err := url.Parse(strings.TrimSpace(server))
	if err != nil {
		return nil, fmt.Errorf("invalid CloudreveLite URL: %w", err)
	}
	if base.Scheme != "https" && base.Scheme != "http" {
		return nil, errors.New("CloudreveLite URL must use https (or http for an explicitly allowed local instance)")
	}
	if base.Host == "" || base.User != nil || base.RawQuery != "" || base.Fragment != "" {
		return nil, errors.New("CloudreveLite URL must contain only scheme, host, port, and an optional base path")
	}
	if base.Scheme == "http" && !allowHTTP && !isLoopbackHost(base.Hostname()) {
		return nil, errors.New("refusing to send credentials over plain HTTP; use HTTPS or pass --allow-http")
	}
	base.Path = strings.TrimRight(base.Path, "/")
	base.RawPath = ""

	return &client{
		base:         base,
		credential:   strings.TrimSpace(credential),
		apiHTTP:      &http.Client{Timeout: 60 * time.Second},
		transferHTTP: &http.Client{},
	}, nil
}

func isLoopbackHost(host string) bool {
	if strings.EqualFold(host, "localhost") {
		return true
	}
	ip := net.ParseIP(host)
	return ip != nil && ip.IsLoopback()
}

func (c *client) apiURL(endpoint string) (*url.URL, error) {
	relative, err := url.Parse(endpoint)
	if err != nil {
		return nil, fmt.Errorf("invalid API endpoint: %w", err)
	}
	if relative.IsAbs() || relative.Host != "" || !strings.HasPrefix(relative.Path, "/") {
		return nil, errors.New("API endpoint must be an absolute path")
	}

	target := *c.base
	target.Path = strings.TrimRight(c.base.Path, "/") + "/api/v4" + relative.Path
	target.RawPath = ""
	target.RawQuery = relative.RawQuery
	return &target, nil
}

func (c *client) doAPI(ctx context.Context, method, endpoint string, body any) (*envelope, error) {
	var reader io.Reader
	if body != nil {
		encoded, err := json.Marshal(body)
		if err != nil {
			return nil, fmt.Errorf("encode request body: %w", err)
		}
		reader = bytes.NewReader(encoded)
	}

	target, err := c.apiURL(endpoint)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, method, target.String(), reader)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/json")
	req.Header.Set("User-Agent", "cloudreve-cli/"+version)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if c.credential != "" {
		req.Header.Set("Authorization", "Bearer "+c.credential)
	}

	resp, err := c.apiHTTP.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	raw, err := readLimited(resp.Body, maxAPIResponseSize)
	if err != nil {
		return nil, fmt.Errorf("read API response: %w", err)
	}
	if resp.StatusCode < http.StatusOK || resp.StatusCode >= http.StatusMultipleChoices {
		return nil, fmt.Errorf("HTTP %s: %s", resp.Status, strings.TrimSpace(string(raw)))
	}

	var result envelope
	if err := json.Unmarshal(raw, &result); err != nil {
		return nil, fmt.Errorf("invalid Cloudreve response: %w", err)
	}
	return &result, nil
}

func (c *client) request(ctx context.Context, method, endpoint string, body any) (json.RawMessage, error) {
	result, err := c.doAPI(ctx, method, endpoint, body)
	if err != nil {
		return nil, err
	}
	if result.Code != 0 {
		return nil, errorFromEnvelope(result)
	}
	if len(result.Data) == 0 {
		return json.RawMessage("null"), nil
	}
	return result.Data, nil
}

func errorFromEnvelope(result *envelope) error {
	return &apiError{
		Code:          result.Code,
		Message:       result.Msg,
		Detail:        result.Err,
		CorrelationID: result.CorrelationID,
	}
}

func (c *client) resolveTransferURL(raw string) (*url.URL, error) {
	target, err := url.Parse(raw)
	if err != nil {
		return nil, fmt.Errorf("invalid transfer URL: %w", err)
	}
	if target.IsAbs() {
		return target, nil
	}
	return c.base.ResolveReference(target), nil
}

func readLimited(reader io.Reader, limit int64) ([]byte, error) {
	raw, err := io.ReadAll(io.LimitReader(reader, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(raw)) > limit {
		return nil, fmt.Errorf("response exceeds %d bytes", limit)
	}
	return raw, nil
}
