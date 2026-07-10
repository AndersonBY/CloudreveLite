package statics

import (
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"testing"
	"testing/fstest"

	"github.com/cloudreve/Cloudreve/v4/application/constants"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/util"
	"github.com/stretchr/testify/require"
)

func TestNewServerStaticFSUsesLocalFilesWithoutCallingSupplier(t *testing.T) {
	useWorkingDir := util.UseWorkingDir
	util.UseWorkingDir = true
	t.Cleanup(func() { util.UseWorkingDir = useWorkingDir })
	t.Chdir(t.TempDir())

	staticDir := filepath.Join(util.DataFolder, StaticFolder)
	require.NoError(t, os.MkdirAll(staticDir, 0o700))
	require.NoError(t, os.WriteFile(
		filepath.Join(staticDir, "version.json"),
		[]byte(frontendVersion()),
		0o600,
	))

	supplierCalls := 0
	staticFS, err := NewServerStaticFS(logging.NewConsoleLogger(logging.LevelError), func() fs.FS {
		supplierCalls++
		return nil
	}, false)

	require.NoError(t, err)
	require.NotNil(t, staticFS)
	require.Zero(t, supplierCalls)
}

func TestNewServerStaticFSCallsSupplierForEmbeddedFallback(t *testing.T) {
	useWorkingDir := util.UseWorkingDir
	util.UseWorkingDir = true
	t.Cleanup(func() { util.UseWorkingDir = useWorkingDir })
	t.Chdir(t.TempDir())

	supplierCalls := 0
	staticFS, err := NewServerStaticFS(logging.NewConsoleLogger(logging.LevelError), func() fs.FS {
		supplierCalls++
		return fstest.MapFS{
			"assets/build/version.json": &fstest.MapFile{Data: []byte(frontendVersion())},
		}
	}, false)

	require.NoError(t, err)
	require.NotNil(t, staticFS)
	require.Equal(t, 1, supplierCalls)
}

func frontendVersion() string {
	return fmt.Sprintf(`{"name":"cloudreve-frontend","version":%q}`, constants.BackendVersion)
}
