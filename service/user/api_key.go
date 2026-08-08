package user

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/base64"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/ent/apicredential"
)

const (
	apiKeyPrefix       = "crl_"
	pairingCodePrefix  = "crp_"
	defaultKeyLifetime = 90 * 24 * time.Hour
	maxKeyLifetime     = 365 * 24 * time.Hour
	minKeyLifetime     = 24 * time.Hour
	pairingLifetime    = 10 * time.Minute
	lastUsedInterval   = 5 * time.Minute
)

var (
	ErrInvalidAPICredential = errors.New("invalid or expired API credential")
	ErrInvalidAPIKeyConfig  = errors.New("invalid API key settings")
)

func IsAPIKey(raw string) bool {
	return strings.HasPrefix(raw, apiKeyPrefix)
}

type APIKey struct {
	ID         string     `json:"id"`
	Name       string     `json:"name"`
	CreatedAt  time.Time  `json:"created_at"`
	LastUsedAt *time.Time `json:"last_used_at,omitempty"`
	ExpiresAt  time.Time  `json:"expires_at"`
}

type CreatedAPIKey struct {
	APIKey
	Key string `json:"key"`
}

type PairingCode struct {
	Code      string    `json:"code"`
	ExpiresAt time.Time `json:"expires_at"`
}

type CreateAPIKeyRequest struct {
	Name      string `json:"name" binding:"required,max=64"`
	ExpiresIn int64  `json:"expires_in"`
}

func ListAPIKeys(ctx context.Context, client *ent.Client, userID int) ([]APIKey, error) {
	now := time.Now()
	_, _ = client.APICredential.Delete().Where(
		apicredential.UserID(userID),
		apicredential.ActivatedAtIsNil(),
		apicredential.PairingExpiresAtLT(now),
	).Exec(ctx)

	credentials, err := client.APICredential.Query().Where(
		apicredential.UserID(userID),
		apicredential.ActivatedAtNotNil(),
	).Order(ent.Desc(apicredential.FieldCreatedAt)).All(ctx)
	if err != nil {
		return nil, err
	}

	result := make([]APIKey, 0, len(credentials))
	for _, credential := range credentials {
		result = append(result, buildAPIKey(credential))
	}
	return result, nil
}

func CreateAPIKey(ctx context.Context, client *ent.Client, userID int, request CreateAPIKeyRequest) (*CreatedAPIKey, error) {
	name, expiresAt, err := validateAPIKeyRequest(request)
	if err != nil {
		return nil, err
	}

	publicID, err := randomToken(9)
	if err != nil {
		return nil, err
	}
	secret, err := randomToken(32)
	if err != nil {
		return nil, err
	}
	key := apiKeyPrefix + publicID + "_" + secret
	now := time.Now()
	credential, err := client.APICredential.Create().
		SetUserID(userID).
		SetName(name).
		SetPublicID(publicID).
		SetKeyDigest(digestCredential(key)).
		SetActivatedAt(now).
		SetExpiresAt(expiresAt).
		Save(ctx)
	if err != nil {
		return nil, err
	}

	return &CreatedAPIKey{APIKey: buildAPIKey(credential), Key: key}, nil
}

func CreatePairingCode(ctx context.Context, client *ent.Client, userID int, request CreateAPIKeyRequest) (*PairingCode, error) {
	name, expiresAt, err := validateAPIKeyRequest(request)
	if err != nil {
		return nil, err
	}
	publicID, err := randomToken(9)
	if err != nil {
		return nil, err
	}
	secret, err := randomToken(32)
	if err != nil {
		return nil, err
	}
	code := pairingCodePrefix + publicID + "_" + secret
	pairingExpiresAt := time.Now().Add(pairingLifetime)
	_, err = client.APICredential.Create().
		SetUserID(userID).
		SetName(name).
		SetPublicID(publicID).
		SetPairingDigest(digestCredential(code)).
		SetPairingExpiresAt(pairingExpiresAt).
		SetExpiresAt(expiresAt).
		Save(ctx)
	if err != nil {
		return nil, err
	}

	return &PairingCode{Code: code, ExpiresAt: pairingExpiresAt}, nil
}

func ExchangePairingCode(ctx context.Context, client *ent.Client, code string) (*CreatedAPIKey, error) {
	publicID, ok := parseCredential(code, pairingCodePrefix)
	if !ok {
		return nil, ErrInvalidAPICredential
	}
	now := time.Now()
	credential, err := client.APICredential.Query().Where(
		apicredential.PublicID(publicID),
		apicredential.ActivatedAtIsNil(),
		apicredential.PairingDigestNotNil(),
		apicredential.PairingExpiresAtGT(now),
	).Only(ctx)
	if err != nil || subtle.ConstantTimeCompare(credential.PairingDigest, digestCredential(code)) != 1 {
		return nil, ErrInvalidAPICredential
	}

	secret, err := randomToken(32)
	if err != nil {
		return nil, err
	}
	key := apiKeyPrefix + publicID + "_" + secret
	updated, err := client.APICredential.Update().Where(
		apicredential.ID(credential.ID),
		apicredential.ActivatedAtIsNil(),
		apicredential.PairingExpiresAtGT(now),
	).SetKeyDigest(digestCredential(key)).
		SetActivatedAt(now).
		ClearPairingDigest().
		ClearPairingExpiresAt().
		Save(ctx)
	if err != nil || updated != 1 {
		return nil, ErrInvalidAPICredential
	}

	credential, err = client.APICredential.Get(ctx, credential.ID)
	if err != nil {
		return nil, err
	}
	return &CreatedAPIKey{APIKey: buildAPIKey(credential), Key: key}, nil
}

func RevokeAPIKey(ctx context.Context, client *ent.Client, userID int, publicID string) error {
	deleted, err := client.APICredential.Delete().Where(
		apicredential.UserID(userID),
		apicredential.PublicID(publicID),
		apicredential.ActivatedAtNotNil(),
	).Exec(ctx)
	if err != nil {
		return err
	}
	if deleted != 1 {
		return ErrInvalidAPICredential
	}
	return nil
}

func AuthenticateAPIKey(ctx context.Context, client *ent.Client, rawKey string) (int, error) {
	publicID, ok := parseCredential(rawKey, apiKeyPrefix)
	if !ok {
		return 0, ErrInvalidAPICredential
	}
	now := time.Now()
	credential, err := client.APICredential.Query().Where(
		apicredential.PublicID(publicID),
		apicredential.ActivatedAtNotNil(),
		apicredential.KeyDigestNotNil(),
		apicredential.ExpiresAtGT(now),
	).Only(ctx)
	if err != nil || subtle.ConstantTimeCompare(credential.KeyDigest, digestCredential(rawKey)) != 1 {
		return 0, ErrInvalidAPICredential
	}

	if credential.LastUsedAt == nil || now.Sub(*credential.LastUsedAt) >= lastUsedInterval {
		if err := client.APICredential.UpdateOneID(credential.ID).SetLastUsedAt(now).Exec(ctx); err != nil {
			return 0, fmt.Errorf("update API key last used time: %w", err)
		}
	}
	return credential.UserID, nil
}

func validateAPIKeyRequest(request CreateAPIKeyRequest) (string, time.Time, error) {
	name := strings.TrimSpace(request.Name)
	if name == "" || len(name) > 64 {
		return "", time.Time{}, fmt.Errorf("%w: name must contain 1 to 64 characters", ErrInvalidAPIKeyConfig)
	}
	lifetime := time.Duration(request.ExpiresIn) * time.Second
	if request.ExpiresIn == 0 {
		lifetime = defaultKeyLifetime
	}
	if lifetime < minKeyLifetime || lifetime > maxKeyLifetime {
		return "", time.Time{}, fmt.Errorf("%w: lifetime must be between 1 and 365 days", ErrInvalidAPIKeyConfig)
	}
	return name, time.Now().Add(lifetime), nil
}

func buildAPIKey(credential *ent.APICredential) APIKey {
	return APIKey{
		ID:         credential.PublicID,
		Name:       credential.Name,
		CreatedAt:  credential.CreatedAt,
		LastUsedAt: credential.LastUsedAt,
		ExpiresAt:  credential.ExpiresAt,
	}
}

func parseCredential(raw, prefix string) (string, bool) {
	if !strings.HasPrefix(raw, prefix) || strings.ContainsAny(raw, "\r\n") {
		return "", false
	}
	payload := strings.TrimPrefix(raw, prefix)
	const publicIDLength = 12
	if len(payload) < publicIDLength+1+40 || payload[publicIDLength] != '_' {
		return "", false
	}
	return payload[:publicIDLength], true
}

func randomToken(size int) (string, error) {
	raw := make([]byte, size)
	if _, err := rand.Read(raw); err != nil {
		return "", fmt.Errorf("generate secure credential: %w", err)
	}
	return base64.RawURLEncoding.EncodeToString(raw), nil
}

func digestCredential(raw string) []byte {
	digest := sha256.Sum256([]byte(raw))
	return digest[:]
}
