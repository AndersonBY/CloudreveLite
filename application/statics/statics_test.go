package statics

import (
	"fmt"
	"os"
	"path/filepath"
	"testing"

	"github.com/cloudreve/Cloudreve/v4/application/constants"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/util"
	"github.com/stretchr/testify/require"
)

func TestNewServerStaticFSUsesLocalFiles(t *testing.T) {
	useWorkingDir := util.UseWorkingDir
	util.UseWorkingDir = true
	t.Cleanup(func() { util.UseWorkingDir = useWorkingDir })
	t.Chdir(t.TempDir())

	staticDir := filepath.Join(util.DataFolder, StaticFolder)
	require.NoError(t, os.MkdirAll(staticDir, 0o700))
	writeFrontendVersion(t, staticDir)

	staticFS, err := NewServerStaticFS(logging.NewConsoleLogger(logging.LevelError), false)

	require.NoError(t, err)
	require.NotNil(t, staticFS)
}

func TestNewServerStaticFSUsesEnvironmentOverride(t *testing.T) {
	staticDir := t.TempDir()
	writeFrontendVersion(t, staticDir)
	t.Setenv(StaticPathEnv, staticDir)

	staticFS, err := NewServerStaticFS(logging.NewConsoleLogger(logging.LevelError), false)

	require.NoError(t, err)
	require.NotNil(t, staticFS)
}

func TestNewServerStaticFSRejectsMissingDirectory(t *testing.T) {
	t.Setenv(StaticPathEnv, filepath.Join(t.TempDir(), "missing"))

	staticFS, err := NewServerStaticFS(logging.NewConsoleLogger(logging.LevelError), false)

	require.Error(t, err)
	require.Nil(t, staticFS)
}

func writeFrontendVersion(t *testing.T, staticDir string) {
	t.Helper()
	require.NoError(t, os.WriteFile(
		filepath.Join(staticDir, "version.json"),
		[]byte(fmt.Sprintf(`{"name":"cloudreve-frontend","version":%q}`, constants.BackendVersion)),
		0o600,
	))
}
