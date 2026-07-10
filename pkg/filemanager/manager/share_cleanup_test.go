package manager

import (
	"context"
	"fmt"
	"testing"
	"time"

	"entgo.io/ent/dialect/sql"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/pkg/boolset"
	"github.com/stretchr/testify/require"
)

func TestHasProtectedSourceOverlap(t *testing.T) {
	sourceTree := map[int]struct{}{1: {}, 2: {}, 3: {}, 4: {}}

	require.True(t, hasProtectedSourceOverlap(sourceTree, map[int]struct{}{4: {}}))
	require.True(t, hasProtectedSourceOverlap(sourceTree, map[int]struct{}{1: {}}))
	require.False(t, hasProtectedSourceOverlap(sourceTree, map[int]struct{}{9: {}}))
	require.False(t, hasProtectedSourceOverlap(sourceTree, nil))
}

func TestLoadSourceTreeChunksWideDirectories(t *testing.T) {
	ctx := context.Background()
	client, owner := newShareCleanupTestClient(t)
	root, err := client.File.Create().SetType(1).SetName("").SetOwner(owner).Save(ctx)
	require.NoError(t, err)

	children := make([]*ent.FileCreate, 1001)
	for i := range children {
		children[i] = client.File.Create().
			SetType(1).
			SetName(fmt.Sprintf("folder-%04d", i)).
			SetOwner(owner).
			SetParent(root)
	}
	_, err = client.File.CreateBulk(children...).Save(ctx)
	require.NoError(t, err)

	tree, err := loadSourceTree(ctx, client, root)
	require.NoError(t, err)
	require.Len(t, tree, 1002)
}

func TestActiveReferenceRootsRespectGracePeriod(t *testing.T) {
	ctx := context.Background()
	client, owner := newShareCleanupTestClient(t)
	root, err := client.File.Create().SetType(1).SetName("").SetOwner(owner).Save(ctx)
	require.NoError(t, err)
	currentFile, err := client.File.Create().SetType(0).SetName("current.txt").SetOwner(owner).SetParent(root).Save(ctx)
	require.NoError(t, err)
	protectedFile, err := client.File.Create().SetType(0).SetName("protected.txt").SetOwner(owner).SetParent(root).Save(ctx)
	require.NoError(t, err)
	staleFile, err := client.File.Create().SetType(0).SetName("stale.txt").SetOwner(owner).SetParent(root).Save(ctx)
	require.NoError(t, err)

	cutoff := time.Now().Add(-time.Hour)
	current, err := client.Share.Create().
		SetUser(owner).
		SetFile(currentFile).
		SetExpires(cutoff.Add(-time.Minute)).
		SetRemainDownloads(0).
		Save(ctx)
	require.NoError(t, err)
	_, err = client.Share.Create().
		SetUser(owner).
		SetFile(protectedFile).
		SetExpires(time.Now().Add(time.Hour)).
		SetRemainDownloads(10).
		Save(ctx)
	require.NoError(t, err)
	_, err = client.Share.Create().
		SetUser(owner).
		SetFile(staleFile).
		SetExpires(cutoff.Add(-time.Hour)).
		SetRemainDownloads(10).
		Save(ctx)
	require.NoError(t, err)

	roots, err := activeOrGraceReferenceRoots(ctx, client, owner.ID, current.ID, cutoff)
	require.NoError(t, err)
	require.Contains(t, roots, protectedFile.ID)
	require.NotContains(t, roots, staleFile.ID)
}

func newShareCleanupTestClient(t *testing.T) (*ent.Client, *ent.User) {
	t.Helper()
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", fmt.Sprintf("file:%s?mode=memory&cache=shared", t.Name()))
	require.NoError(t, err)
	driver.DB().SetMaxOpenConns(1)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	group, err := client.Group.Create().
		SetName("test").
		SetPermissions(&boolset.BooleanSet{}).
		Save(ctx)
	require.NoError(t, err)
	owner, err := client.User.Create().
		SetEmail(fmt.Sprintf("%s@example.com", t.Name())).
		SetNick("owner").
		SetGroup(group).
		Save(ctx)
	require.NoError(t, err)
	return client, owner
}
