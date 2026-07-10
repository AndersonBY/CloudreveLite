package manager

import (
	"context"
	"fmt"
	"sync/atomic"
	"time"

	"github.com/cloudreve/Cloudreve/v4/application/dependency"
	"github.com/cloudreve/Cloudreve/v4/ent"
	fileent "github.com/cloudreve/Cloudreve/v4/ent/file"
	"github.com/cloudreve/Cloudreve/v4/ent/predicate"
	shareent "github.com/cloudreve/Cloudreve/v4/ent/share"
	"github.com/cloudreve/Cloudreve/v4/ent/user"
	"github.com/cloudreve/Cloudreve/v4/inventory"
	"github.com/cloudreve/Cloudreve/v4/pkg/crontab"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/fs"
	"github.com/cloudreve/Cloudreve/v4/pkg/setting"
)

const (
	minimumShareCleanupGrace = time.Hour
	shareCleanupBatchSize    = 100
	shareTreeQueryBatchSize  = 400
)

var shareCleanupRunning atomic.Bool

func init() {
	crontab.Register(setting.CronTypeShareCleanup, CronCleanupExpiredShares)
}

// CronCleanupExpiredShares deletes source files whose temporary shares have
// expired or exhausted their download limit. A grace period keeps the final
// signed object URL usable until it expires.
func CronCleanupExpiredShares(ctx context.Context) {
	if !shareCleanupRunning.CompareAndSwap(false, true) {
		return
	}
	defer shareCleanupRunning.Store(false)

	dep := dependency.FromContext(ctx)
	grace := dep.SettingProvider().EntityUrlValidDuration(ctx)
	if grace < minimumShareCleanupGrace {
		grace = minimumShareCleanupGrace
	}
	cutoff := time.Now().Add(-grace)
	lastID := 0

	for {
		shares, err := dep.DBClient().Share.Query().
			Where(shareent.IDGT(lastID), shareCleanupReady(cutoff)).
			WithFile().
			WithUser(func(q *ent.UserQuery) { q.WithGroup() }).
			Order(shareent.ByID()).
			Limit(shareCleanupBatchSize).
			All(ctx)
		if err != nil {
			dep.Logger().Error("Failed to list expired temporary shares: %s", err)
			return
		}
		if len(shares) == 0 {
			return
		}

		for _, shared := range shares {
			if err := cleanupExpiredShare(ctx, dep, shared, cutoff); err != nil {
				dep.Logger().Error("Failed to clean expired share %d: %s", shared.ID, err)
			}
		}
		lastID = shares[len(shares)-1].ID
		if len(shares) < shareCleanupBatchSize {
			return
		}
	}
}

func shareCleanupReady(cutoff time.Time) predicate.Share {
	return shareent.Or(
		shareent.And(shareent.ExpiresNotNil(), shareent.ExpiresLTE(cutoff)),
		shareent.And(
			shareent.RemainDownloadsNotNil(),
			shareent.RemainDownloadsLTE(0),
			shareent.UpdatedAtLTE(cutoff),
		),
	)
}

func cleanupExpiredShare(ctx context.Context, dep dependency.Dep, candidate *ent.Share, cutoff time.Time) error {
	owner, ownerErr := candidate.Edges.UserOrErr()
	root, rootErr := candidate.Edges.FileOrErr()
	if ownerErr != nil || rootErr != nil {
		return deleteOrphanShare(ctx, dep, candidate.ID)
	}

	ownerCtx := context.WithValue(ctx, inventory.UserCtx{}, owner)
	fm := NewFileManager(dep, owner)
	defer fm.Recycle()
	rootFile, err := fm.TraverseFile(ownerCtx, root.ID)
	if err != nil {
		return fmt.Errorf("traverse shared source %d: %w", root.ID, err)
	}
	lockSession, err := fm.Lock(
		ownerCtx,
		-1,
		owner,
		false,
		fs.LockApp(fs.ApplicationShare),
		rootFile.Uri(false),
		"",
	)
	if err != nil {
		return fmt.Errorf("lock shared source %d: %w", root.ID, err)
	}
	lockToken := lockSession.LastToken()
	defer func() {
		if err := fm.Unlock(ownerCtx, lockToken); err != nil {
			dep.Logger().Warning("Failed to unlock shared source %d: %s", root.ID, err)
		}
	}()
	ownerCtx = fs.LockSessionToContext(ownerCtx, lockSession)

	// The candidate may have been renewed or moved before the source lock was
	// acquired. Re-read it under the lock before making any deletion decision.
	current, err := dep.DBClient().Share.Query().
		Where(shareent.ID(candidate.ID), shareCleanupReady(cutoff)).
		WithFile().
		WithUser(func(q *ent.UserQuery) { q.WithGroup() }).
		Only(ctx)
	if ent.IsNotFound(err) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("reload expired share %d: %w", candidate.ID, err)
	}
	owner, ownerErr = current.Edges.UserOrErr()
	root, rootErr = current.Edges.FileOrErr()
	if ownerErr != nil || rootErr != nil {
		return deleteOrphanShare(ctx, dep, current.ID)
	}

	referenceRoots, err := activeOrGraceReferenceRoots(ctx, dep.DBClient(), owner.ID, current.ID, cutoff)
	if err != nil {
		return err
	}
	protectedByAncestor, err := hasProtectedAncestor(ctx, dep.DBClient(), root, referenceRoots)
	if err != nil {
		return err
	}
	if protectedByAncestor {
		return nil
	}

	sourceTree, err := loadSourceTree(ctx, dep.DBClient(), root)
	if err != nil {
		return err
	}
	if hasProtectedSourceOverlap(sourceTree, referenceRoots) {
		return nil
	}

	if err := fm.Delete(ownerCtx, []*fs.URI{rootFile.Uri(false)}, fs.WithSkipSoftDelete(true)); err != nil {
		return fmt.Errorf("delete unreferenced share source: %w", err)
	}
	return nil
}

func deleteOrphanShare(ctx context.Context, dep dependency.Dep, shareID int) error {
	if err := dep.ShareClient().Delete(ctx, shareID); err != nil && !ent.IsNotFound(err) {
		return fmt.Errorf("delete orphan share %d: %w", shareID, err)
	}
	return nil
}

func activeOrGraceReferenceRoots(
	ctx context.Context,
	client *ent.Client,
	ownerID, excludedShareID int,
	cutoff time.Time,
) (map[int]struct{}, error) {
	shares, err := client.Share.Query().
		Where(
			shareent.IDNEQ(excludedShareID),
			shareent.HasUserWith(user.ID(ownerID)),
			shareent.And(
				shareent.Or(shareent.ExpiresIsNil(), shareent.ExpiresGT(cutoff)),
				shareent.Or(
					shareent.RemainDownloadsIsNil(),
					shareent.RemainDownloadsGT(0),
					shareent.UpdatedAtGT(cutoff),
				),
			),
		).
		WithFile().
		All(ctx)
	if err != nil {
		return nil, fmt.Errorf("list active share references: %w", err)
	}

	roots := make(map[int]struct{}, len(shares))
	for _, shared := range shares {
		if file, err := shared.Edges.FileOrErr(); err == nil {
			roots[file.ID] = struct{}{}
		}
	}
	return roots, nil
}

func hasProtectedAncestor(
	ctx context.Context,
	client *ent.Client,
	root *ent.File,
	protected map[int]struct{},
) (bool, error) {
	current := root
	for {
		if _, ok := protected[current.ID]; ok {
			return true, nil
		}
		if current.FileChildren <= 0 {
			return false, nil
		}

		parent, err := client.File.Get(ctx, current.FileChildren)
		if ent.IsNotFound(err) {
			return false, nil
		}
		if err != nil {
			return false, fmt.Errorf("load source ancestor %d: %w", current.FileChildren, err)
		}
		current = parent
	}
}

func loadSourceTree(ctx context.Context, client *ent.Client, root *ent.File) (map[int]struct{}, error) {
	nodes := map[int]struct{}{root.ID: {}}
	frontier := []int{root.ID}
	for len(frontier) > 0 {
		next := make([]int, 0)
		for start := 0; start < len(frontier); start += shareTreeQueryBatchSize {
			end := min(start+shareTreeQueryBatchSize, len(frontier))
			level, err := client.File.Query().Where(fileent.FileChildrenIn(frontier[start:end]...)).All(ctx)
			if err != nil {
				return nil, fmt.Errorf("load source descendants: %w", err)
			}
			for _, child := range level {
				nodes[child.ID] = struct{}{}
				next = append(next, child.ID)
			}
		}
		frontier = next
	}
	return nodes, nil
}

func hasProtectedSourceOverlap(sourceTree, protected map[int]struct{}) bool {
	for id := range protected {
		if _, ok := sourceTree[id]; ok {
			return true
		}
	}
	return false
}
