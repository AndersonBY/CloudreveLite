package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/zalando/go-keyring"
)

const credentialService = "CloudreveLite CLI"

type credentialStore interface {
	Set(service, account, secret string) error
	Get(service, account string) (string, error)
	Delete(service, account string) error
}

type systemCredentialStore struct{}

func (systemCredentialStore) Set(service, account, secret string) error {
	return keyring.Set(service, account, secret)
}

func (systemCredentialStore) Get(service, account string) (string, error) {
	return keyring.Get(service, account)
}

func (systemCredentialStore) Delete(service, account string) error {
	return keyring.Delete(service, account)
}

var cliCredentialStore credentialStore = systemCredentialStore{}

type cliConfig struct {
	Server string `json:"server"`
}

func saveConnection(server, apiKey string) error {
	account := credentialAccount(server)
	if err := cliCredentialStore.Set(credentialService, account, apiKey); err != nil {
		return fmt.Errorf("store API key in the operating system credential manager: %w", err)
	}

	path, err := cliConfigPath()
	if err != nil {
		_ = cliCredentialStore.Delete(credentialService, account)
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		_ = cliCredentialStore.Delete(credentialService, account)
		return fmt.Errorf("create CLI config directory: %w", err)
	}
	raw, err := json.MarshalIndent(cliConfig{Server: server}, "", "  ")
	if err != nil {
		_ = cliCredentialStore.Delete(credentialService, account)
		return err
	}
	raw = append(raw, '\n')
	if err := os.WriteFile(path, raw, 0o600); err != nil {
		_ = cliCredentialStore.Delete(credentialService, account)
		return fmt.Errorf("write CLI config: %w", err)
	}
	if err := os.Chmod(path, 0o600); err != nil {
		return fmt.Errorf("restrict CLI config permissions: %w", err)
	}
	return nil
}

func configuredServer() (string, error) {
	path, err := cliConfigPath()
	if err != nil {
		return "", err
	}
	raw, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("read CLI config: %w", err)
	}
	var config cliConfig
	if err := json.Unmarshal(raw, &config); err != nil {
		return "", fmt.Errorf("decode CLI config: %w", err)
	}
	return strings.TrimSpace(config.Server), nil
}

func storedAPIKey(server string) (string, error) {
	secret, err := cliCredentialStore.Get(credentialService, credentialAccount(server))
	if errors.Is(err, keyring.ErrNotFound) {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("read API key from the operating system credential manager: %w", err)
	}
	return strings.TrimSpace(secret), nil
}

func cliConfigPath() (string, error) {
	if override := strings.TrimSpace(os.Getenv("CLOUDREVE_CONFIG_DIR")); override != "" {
		return filepath.Join(override, "config.json"), nil
	}
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", fmt.Errorf("locate user config directory: %w", err)
	}
	return filepath.Join(dir, "cloudreve-cli", "config.json"), nil
}

func credentialAccount(server string) string {
	digest := sha256.Sum256([]byte(strings.TrimRight(strings.TrimSpace(server), "/")))
	return hex.EncodeToString(digest[:])
}
