package manager

import (
	"context"
	"testing"
	"time"

	"github.com/cloudreve/Cloudreve/v4/pkg/cache"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/fs"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/stretchr/testify/require"
)

func TestMarkChunkUploadedWaitsForEveryChunk(t *testing.T) {
	kv := cache.NewMemoStore("", logging.NewConsoleLogger(logging.LevelError))
	session := fs.UploadSession{
		ChunkSize: 4,
		Props: &fs.UploadProps{
			Size:            10,
			UploadSessionID: "chunk-session",
			ExpireAt:        time.Now().Add(time.Minute),
		},
	}
	require.NoError(t, kv.Set(UploadSessionCachePrefix+session.Props.UploadSessionID, session, 60))

	m := &manager{kv: kv}
	complete, err := m.MarkChunkUploaded(context.Background(), &session, 2)
	require.NoError(t, err)
	require.False(t, complete)

	complete, err = m.MarkChunkUploaded(context.Background(), &session, 0)
	require.NoError(t, err)
	require.False(t, complete)

	complete, err = m.MarkChunkUploaded(context.Background(), &session, 1)
	require.NoError(t, err)
	require.True(t, complete)

	complete, err = m.MarkChunkUploaded(context.Background(), &session, 1)
	require.NoError(t, err)
	require.False(t, complete)
}
