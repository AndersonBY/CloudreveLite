package inventory

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"entgo.io/ent/dialect/sql"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/stretchr/testify/require"
)

func TestDownloadedAtomicallyStopsAtLimit(t *testing.T) {
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:share_downloaded?mode=memory&cache=shared")
	require.NoError(t, err)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	shared, err := client.Share.Create().SetRemainDownloads(1).Save(ctx)
	require.NoError(t, err)
	shareClient := &shareClient{client: client}

	require.NoError(t, shareClient.Downloaded(ctx, shared))
	require.ErrorIs(t, shareClient.Downloaded(ctx, shared), ErrShareLinkExpired)

	stored, err := client.Share.Get(ctx, shared.ID)
	require.NoError(t, err)
	require.Equal(t, 1, stored.Downloads)
	require.NotNil(t, stored.RemainDownloads)
	require.Equal(t, 0, *stored.RemainDownloads)
}

func TestDownloadedAllowsExactlyConfiguredLimitUnderConcurrency(t *testing.T) {
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:share_concurrent?mode=memory&cache=shared")
	require.NoError(t, err)
	driver.DB().SetMaxOpenConns(1)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	shared, err := client.Share.Create().
		SetRemainDownloads(10).
		SetExpires(time.Now().Add(time.Hour)).
		Save(ctx)
	require.NoError(t, err)
	shareClient := &shareClient{client: client}

	const attempts = 100
	results := make(chan error, attempts)
	var wg sync.WaitGroup
	for range attempts {
		wg.Add(1)
		go func() {
			defer wg.Done()
			results <- shareClient.Downloaded(ctx, shared)
		}()
	}
	wg.Wait()
	close(results)

	succeeded := 0
	for err := range results {
		if err == nil {
			succeeded++
			continue
		}
		require.ErrorIs(t, err, ErrShareLinkExpired)
	}
	require.Equal(t, 10, succeeded)

	stored, err := client.Share.Get(ctx, shared.ID)
	require.NoError(t, err)
	require.Equal(t, 10, stored.Downloads)
	require.NotNil(t, stored.RemainDownloads)
	require.Equal(t, 0, *stored.RemainDownloads)
}

func TestDownloadedRejectsExpiredShare(t *testing.T) {
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:share_expired?mode=memory&cache=shared")
	require.NoError(t, err)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	shared, err := client.Share.Create().
		SetRemainDownloads(1).
		SetExpires(time.Now().Add(-time.Second)).
		Save(ctx)
	require.NoError(t, err)
	shareClient := &shareClient{client: client}

	require.ErrorIs(t, shareClient.Downloaded(ctx, shared), ErrShareLinkExpired)
	stored, err := client.Share.Get(ctx, shared.ID)
	require.NoError(t, err)
	require.Equal(t, 0, stored.Downloads)
	require.Equal(t, 1, *stored.RemainDownloads)
}

func TestDownloadedRejectsSoftDeletedShare(t *testing.T) {
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:share_deleted?mode=memory&cache=shared")
	require.NoError(t, err)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	shared, err := client.Share.Create().
		SetRemainDownloads(1).
		SetExpires(time.Now().Add(time.Hour)).
		Save(ctx)
	require.NoError(t, err)
	require.NoError(t, client.Share.DeleteOne(shared).Exec(ctx))
	shareClient := &shareClient{client: client}

	require.ErrorIs(t, shareClient.Downloaded(ctx, shared), ErrShareLinkExpired)
}

func TestDownloadedKeepsLegacyUnlimitedShareCompatible(t *testing.T) {
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:share_unlimited?mode=memory&cache=shared")
	require.NoError(t, err)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	shared, err := client.Share.Create().Save(ctx)
	require.NoError(t, err)
	shareClient := &shareClient{client: client}

	require.NoError(t, shareClient.Downloaded(ctx, shared))
	stored, err := client.Share.Get(ctx, shared.ID)
	require.NoError(t, err)
	require.Equal(t, 1, stored.Downloads)
	require.True(t, errors.Is(IsShareExpired(stored), nil))
}
