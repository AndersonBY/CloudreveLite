package conf

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/util"
	"github.com/go-ini/ini"
	"github.com/stretchr/testify/require"
)

func TestNewIniConfigProviderCreatesDefaultConfig(t *testing.T) {
	configPath := filepath.Join(t.TempDir(), "conf.ini")
	provider, err := NewIniConfigProvider(configPath, logging.NewConsoleLogger(logging.LevelError))
	require.NoError(t, err)
	require.True(t, util.Exists(configPath))
	require.Equal(t, ":5212", provider.System().Listen)
}

func TestNewIniConfigProviderRejectsMalformedConfig(t *testing.T) {
	configPath := filepath.Join(t.TempDir(), "conf.ini")
	require.NoError(t, os.WriteFile(configPath, []byte(`
[Database]
Type = mysql
User = root
Password233root
Host = 127.0.0.1:3306
Name = v3
TablePrefix = v3_
`), 0o600))

	_, err := NewIniConfigProvider(configPath, logging.NewConsoleLogger(logging.LevelError))
	require.Error(t, err)
}

func TestNewIniConfigProviderLoadsValidConfig(t *testing.T) {
	configPath := filepath.Join(t.TempDir(), "conf.ini")
	require.NoError(t, os.WriteFile(configPath, []byte(`
[System]
Listen = :3000
HashIDSalt = 1

[Database]
Type = mysql
User = root
Password = root
Host = 127.0.0.1:3306
Name = v3
TablePrefix = v3_
`), 0o600))

	provider, err := NewIniConfigProvider(configPath, logging.NewConsoleLogger(logging.LevelError))
	require.NoError(t, err)
	require.Equal(t, ":3000", provider.System().Listen)
	require.Equal(t, DBType("mysql"), provider.Database().Type)
}

func TestMapSection(t *testing.T) {
	cfg, err := ini.Load([]byte(`
[Database]
Type = mysql
User = root
Password = root
Host = 127.0.0.1:3306
Name = v3
TablePrefix = v3_
`))
	require.NoError(t, err)

	database := *DatabaseConfig
	require.NoError(t, mapSection(cfg, "Database", &database))
	require.Equal(t, DBType("mysql"), database.Type)
}

func TestOverrideConfSupportsDoubleUnderscoreAndSkipsMalformedKeys(t *testing.T) {
	t.Setenv("CR_CONF_System__Listen", ":8080")
	t.Setenv("CR_CONF_BadFormat", "ignored")

	result := getOverrideConfFromEnv(logging.NewConsoleLogger(logging.LevelError))
	require.Contains(t, result, "[System]")
	require.Contains(t, result, "Listen = :8080")
	require.False(t, strings.Contains(result, "BadFormat"))
}
