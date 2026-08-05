package oss

import (
	"bytes"
	"crypto"
	"crypto/md5"
	"crypto/rand"
	"crypto/rsa"
	"crypto/x509"
	"encoding/base64"
	"encoding/pem"
	"io"
	"net/http"
	"strings"
	"testing"

	"github.com/cloudreve/Cloudreve/v4/pkg/cache"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/request"
	"github.com/stretchr/testify/require"
)

type publicKeyClient struct {
	keys  map[string][]byte
	calls []string
}

func (c *publicKeyClient) Apply(opts ...request.Option) {}

func (c *publicKeyClient) Request(method, target string, body io.Reader, opts ...request.Option) *request.Response {
	c.calls = append(c.calls, target)
	key, ok := c.keys[target]
	status := http.StatusOK
	if !ok {
		status = http.StatusNotFound
	}

	return &request.Response{Response: &http.Response{
		StatusCode: status,
		Body:       io.NopCloser(bytes.NewReader(key)),
	}}
}

func newTestCache() cache.Driver {
	return cache.NewMemoStore("", logging.NewConsoleLogger(logging.LevelError))
}

func encodePublicKeyURL(rawURL string) string {
	return base64.StdEncoding.EncodeToString([]byte(rawURL))
}

func publicKeyPEM(t *testing.T, privateKey *rsa.PrivateKey) []byte {
	t.Helper()
	der, err := x509.MarshalPKIXPublicKey(&privateKey.PublicKey)
	require.NoError(t, err)
	return pem.EncodeToMemory(&pem.Block{Type: "PUBLIC KEY", Bytes: der})
}

func signedCallbackRequest(t *testing.T, rawURL string, privateKey *rsa.PrivateKey) *http.Request {
	t.Helper()
	body := []byte("{\"object\":\"test.bin\"}")
	req, err := http.NewRequest(http.MethodPost, "https://share.example/api/v4/callback/oss/session/key", bytes.NewReader(body))
	require.NoError(t, err)
	req.Header.Set(pubKeyHeader, encodePublicKeyURL(rawURL))

	digest := md5.Sum(append([]byte(req.URL.Path+"\n"), body...))
	signature, err := rsa.SignPKCS1v15(rand.Reader, privateKey, crypto.MD5, digest[:])
	require.NoError(t, err)
	req.Header.Set("authorization", base64.StdEncoding.EncodeToString(signature))
	return req
}

func TestGetPublicKeyCachesByURL(t *testing.T) {
	firstURL := "https://gosspublic.alicdn.com/Callback_1.0.pub"
	secondURL := "https://gosspublic.alicdn.com/Callback_2.0.pub"
	client := &publicKeyClient{keys: map[string][]byte{
		firstURL:  []byte("first"),
		secondURL: []byte("second"),
	}}
	kv := newTestCache()

	firstRequest, err := http.NewRequest(http.MethodPost, "/callback", nil)
	require.NoError(t, err)
	firstRequest.Header.Set(pubKeyHeader, encodePublicKeyURL(firstURL))
	secondRequest, err := http.NewRequest(http.MethodPost, "/callback", nil)
	require.NoError(t, err)
	secondRequest.Header.Set(pubKeyHeader, encodePublicKeyURL(secondURL))

	first, err := GetPublicKey(firstRequest, kv, client)
	require.NoError(t, err)
	second, err := GetPublicKey(secondRequest, kv, client)
	require.NoError(t, err)
	require.Equal(t, []byte("first"), first)
	require.Equal(t, []byte("second"), second)
	require.Equal(t, []string{firstURL, secondURL}, client.calls)
}

func TestVerifyCallbackSignatureRefreshesStaleCachedKey(t *testing.T) {
	pubURL := "https://gosspublic.alicdn.com/Callback_1.0.pub"
	stalePrivateKey, err := rsa.GenerateKey(rand.Reader, 1024)
	require.NoError(t, err)
	currentPrivateKey, err := rsa.GenerateKey(rand.Reader, 1024)
	require.NoError(t, err)

	kv := newTestCache()
	require.NoError(t, kv.Set(publicKeyCacheKey(pubURL), publicKeyPEM(t, stalePrivateKey), pubKeyCacheTTL))
	client := &publicKeyClient{keys: map[string][]byte{pubURL: publicKeyPEM(t, currentPrivateKey)}}

	err = VerifyCallbackSignature(signedCallbackRequest(t, pubURL, currentPrivateKey), kv, client)
	require.NoError(t, err)
	require.Equal(t, []string{pubURL}, client.calls)
	refreshed, ok := kv.Get(publicKeyCacheKey(pubURL))
	require.True(t, ok)
	refreshedKey, ok := cachedPublicKey(refreshed)
	require.True(t, ok)
	require.Equal(t, client.keys[pubURL], refreshedKey)
}

func TestGetPublicKeyRejectsUntrustedURL(t *testing.T) {
	client := &publicKeyClient{keys: map[string][]byte{}}
	req, err := http.NewRequest(http.MethodPost, "/callback", strings.NewReader(""))
	require.NoError(t, err)
	req.Header.Set(pubKeyHeader, encodePublicKeyURL("https://gosspublic.alicdn.com.evil.example/Callback_1.0.pub"))

	_, err = GetPublicKey(req, newTestCache(), client)
	require.ErrorContains(t, err, "public key url invalid")
	require.Empty(t, client.calls)
}
