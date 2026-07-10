package application

import (
	"testing"

	"github.com/stretchr/testify/assert"
)

func TestNewServerOptionsUsesDefaultStartupProfile(t *testing.T) {
	assert.Equal(t, startupProfile{
		oneDriveCredentials: true,
		emailQueue:          true,
		mediaMetaQueue:      true,
		entityRecycleQueue:  true,
		ioIntenseQueue:      true,
		remoteDownloadQueue: true,
		thumbQueue:          true,
	}, newServerOptions().startupProfile)
}

func TestWithLiteModeUsesLiteStartupProfile(t *testing.T) {
	assert.Equal(t, startupProfile{entityRecycleQueue: true}, newServerOptions(WithLiteMode()).startupProfile)
}
