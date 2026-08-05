package dbfs

import (
	"testing"

	"github.com/cloudreve/Cloudreve/v4/application/constants"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/stretchr/testify/require"
)

func TestRootUriFallsBackToFileSystemRoot(t *testing.T) {
	file := newFile(nil, &ent.File{Name: "expired.zip"})
	file.Path[pathIndexRoot] = newTrashUri(file.Name())
	file.Path[pathIndexUser] = file.Path[pathIndexRoot]
	t.Cleanup(file.Recycle)

	rootUri := file.RootUri()
	require.Equal(t, constants.FileSystemTrash, rootUri.FileSystem())
	require.Empty(t, rootUri.PathTrimmed())
}

func TestRootUriReturnsNilWithoutResolvedPath(t *testing.T) {
	file := newFile(nil, &ent.File{Name: "unresolved"})
	t.Cleanup(file.Recycle)

	require.Nil(t, file.RootUri())
}
