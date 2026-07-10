package share

import (
	"testing"
	"time"

	"github.com/stretchr/testify/require"
)

func TestNormalizeSharePolicyDefaults(t *testing.T) {
	now := time.Date(2026, time.July, 11, 12, 0, 0, 0, time.UTC)

	expires, downloads, err := normalizeSharePolicy(now, 0, 0)

	require.NoError(t, err)
	require.Equal(t, now.Add(DefaultShareExpire), *expires)
	require.Equal(t, DefaultShareDownloadLimit, downloads)
}

func TestNormalizeSharePolicyAllowsConfiguredMaximums(t *testing.T) {
	now := time.Date(2026, time.July, 11, 12, 0, 0, 0, time.UTC)

	expires, downloads, err := normalizeSharePolicy(
		now,
		int(MaxShareExpire/time.Second),
		MaxShareDownloadLimit,
	)

	require.NoError(t, err)
	require.Equal(t, now.Add(MaxShareExpire), *expires)
	require.Equal(t, MaxShareDownloadLimit, downloads)
}

func TestNormalizeSharePolicyRejectsValuesAboveMaximums(t *testing.T) {
	now := time.Date(2026, time.July, 11, 12, 0, 0, 0, time.UTC)

	_, _, err := normalizeSharePolicy(now, int(MaxShareExpire/time.Second)+1, 1)
	require.Error(t, err)

	_, _, err = normalizeSharePolicy(now, 1, MaxShareDownloadLimit+1)
	require.Error(t, err)
}

func TestNormalizeSharePolicyReplacesUnlimitedValues(t *testing.T) {
	now := time.Date(2026, time.July, 11, 12, 0, 0, 0, time.UTC)

	expires, downloads, err := normalizeSharePolicy(now, -1, -1)

	require.NoError(t, err)
	require.Equal(t, now.Add(DefaultShareExpire), *expires)
	require.Equal(t, DefaultShareDownloadLimit, downloads)
}
