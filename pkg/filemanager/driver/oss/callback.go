package oss

import (
	"bytes"
	"crypto"
	"crypto/md5"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"

	"github.com/cloudreve/Cloudreve/v4/pkg/cache"
	"github.com/cloudreve/Cloudreve/v4/pkg/request"
)

const (
	pubKeyHeader      = "x-oss-pub-key-url"
	pubKeyHost        = "gosspublic.alicdn.com"
	pubKeyCachePrefix = "oss_public_key:"
	pubKeyCacheTTL    = 86400 * 7
)

func getPublicKeyURL(r *http.Request) (string, error) {
	encodedURL := r.Header.Get(pubKeyHeader)
	if encodedURL == "" {
		return "", errors.New("public key url missing")
	}

	decodedURL, err := base64.StdEncoding.DecodeString(encodedURL)
	if err != nil {
		return "", fmt.Errorf("decode public key url: %w", err)
	}

	pubURL, err := url.Parse(string(decodedURL))
	if err != nil || pubURL.User != nil || pubURL.Fragment != "" || pubURL.Port() != "" ||
		(pubURL.Scheme != "http" && pubURL.Scheme != "https") || pubURL.Hostname() != pubKeyHost {
		return "", errors.New("public key url invalid")
	}

	return pubURL.String(), nil
}

func publicKeyCacheKey(pubURL string) string {
	digest := sha256.Sum256([]byte(pubURL))
	return fmt.Sprintf("%s%x", pubKeyCachePrefix, digest)
}

func cachedPublicKey(value any) ([]byte, bool) {
	switch key := value.(type) {
	case []byte:
		return key, true
	case string:
		return []byte(key), true
	default:
		return nil, false
	}
}

func getPublicKey(r *http.Request, kv cache.Driver, client request.Client, forceRefresh bool) ([]byte, string, bool, error) {
	pubURL, err := getPublicKeyURL(r)
	if err != nil {
		return nil, "", false, err
	}

	cacheKey := publicKeyCacheKey(pubURL)
	if !forceRefresh {
		if value, exists := kv.Get(cacheKey); exists {
			if pubKey, ok := cachedPublicKey(value); ok {
				return pubKey, cacheKey, true, nil
			}
			_ = kv.Delete("", cacheKey)
		}
	}

	body, err := client.Request("GET", pubURL, nil).
		CheckHTTPResponse(200).
		GetResponse()
	if err != nil {
		return nil, cacheKey, false, fmt.Errorf("fetch public key: %w", err)
	}

	pubKey := []byte(body)
	_ = kv.Set(cacheKey, pubKey, pubKeyCacheTTL)
	return pubKey, cacheKey, false, nil
}

// GetPublicKey 从回调请求或缓存中获取OSS的回调签名公钥
func GetPublicKey(r *http.Request, kv cache.Driver, client request.Client) ([]byte, error) {
	pubKey, _, _, err := getPublicKey(r, kv, client, false)
	return pubKey, err
}

func getRequestMD5(r *http.Request) ([]byte, error) {
	var byteMD5 []byte

	// 获取请求正文
	body, err := io.ReadAll(r.Body)
	r.Body.Close()
	if err != nil {
		return byteMD5, err
	}
	r.Body = io.NopCloser(bytes.NewReader(body))

	strURLPathDecode, err := url.PathUnescape(r.URL.Path)
	if err != nil {
		return byteMD5, err
	}

	strAuth := fmt.Sprintf("%s\n%s", strURLPathDecode, string(body))
	md5Ctx := md5.New()
	md5Ctx.Write([]byte(strAuth))
	byteMD5 = md5Ctx.Sum(nil)

	return byteMD5, nil
}

func verifyCallbackSignature(bytePublicKey, byteMD5, authorization []byte) error {
	pubBlock, _ := pem.Decode(bytePublicKey)
	if pubBlock == nil {
		return errors.New("public key PEM block missing")
	}
	pubInterface, err := x509.ParsePKIXPublicKey(pubBlock.Bytes)
	if err != nil {
		return fmt.Errorf("parse public key: %w", err)
	}
	pub, ok := pubInterface.(*rsa.PublicKey)
	if !ok {
		return errors.New("public key is not RSA")
	}

	return rsa.VerifyPKCS1v15(pub, crypto.MD5, byteMD5, authorization)
}

// VerifyCallbackSignature 验证OSS回调请求
func VerifyCallbackSignature(r *http.Request, kv cache.Driver, client request.Client) error {
	strAuthorizationBase64 := r.Header.Get("authorization")
	if strAuthorizationBase64 == "" {
		return errors.New("no authorization field in Request header")
	}
	authorization, err := base64.StdEncoding.DecodeString(strAuthorizationBase64)
	if err != nil {
		return fmt.Errorf("decode callback authorization: %w", err)
	}

	byteMD5, err := getRequestMD5(r)
	if err != nil {
		return fmt.Errorf("hash callback request: %w", err)
	}

	bytePublicKey, cacheKey, fromCache, err := getPublicKey(r, kv, client, false)
	if err != nil {
		return err
	}

	err = verifyCallbackSignature(bytePublicKey, byteMD5, authorization)
	if errors.Is(err, rsa.ErrVerification) && fromCache {
		_ = kv.Delete("", cacheKey)
		bytePublicKey, _, _, refreshErr := getPublicKey(r, kv, client, true)
		if refreshErr != nil {
			return fmt.Errorf("refresh public key after signature failure: %w", refreshErr)
		}
		err = verifyCallbackSignature(bytePublicKey, byteMD5, authorization)
	}
	if err != nil {
		return fmt.Errorf("verify callback signature: %w", err)
	}

	return nil
}
