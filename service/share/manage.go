package share

import (
	"context"
	"fmt"
	"time"

	"github.com/cloudreve/Cloudreve/v4/application/dependency"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/inventory"
	"github.com/cloudreve/Cloudreve/v4/inventory/types"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/fs"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/manager"
	"github.com/cloudreve/Cloudreve/v4/pkg/hashid"
	"github.com/cloudreve/Cloudreve/v4/pkg/serializer"
	"github.com/cloudreve/Cloudreve/v4/service/explorer"
	"github.com/gin-gonic/gin"
)

const (
	DefaultShareExpire        = 7 * 24 * time.Hour
	MaxShareExpire            = 30 * 24 * time.Hour
	DefaultShareDownloadLimit = 10
	MaxShareDownloadLimit     = 100
)

type (
	// ShareCreateService 创建新分享服务
	ShareCreateService struct {
		Uri             string `json:"uri" binding:"required"`
		IsPrivate       bool   `json:"is_private"`
		Password        string `json:"password" binding:"omitempty,max=32,alphanum"`
		RemainDownloads int    `json:"downloads"`
		Expire          int    `json:"expire"`
		ShareView       bool   `json:"share_view"`
		ShowReadMe      bool   `json:"show_readme"`
	}
	ShareCreateParamCtx struct{}

	BatchDeleteShareService struct {
		ShareIDs []string `json:"ids" binding:"required"`
	}
	BatchDeleteParamCtx struct{}
)

func (service *BatchDeleteShareService) Delete(c *gin.Context) error {
	dep := dependency.FromContext(c)
	uid := inventory.UserIDFromContext(c)
	shareClient := dep.ShareClient()

	var ids []int

	for _, v := range service.ShareIDs {
		id, err := dep.HashIDEncoder().Decode(v, hashid.ShareID)
		if err != nil {
			return fmt.Errorf("failed to decode hash id %q: %w", v, err)
		}

		ids = append(ids, id)
	}

	if err := shareClient.DeleteBatchByUserID(c, uid, ids); err != nil {
		return serializer.NewError(serializer.CodeDBError, "Failed to delete shares", err)
	}

	return nil
}

// Upsert 创建或更新分享
func (service *ShareCreateService) Upsert(c *gin.Context, existed int) (string, error) {
	dep := dependency.FromContext(c)
	user := inventory.UserFromContext(c)
	m := manager.NewFileManager(dep, user)
	defer m.Recycle()

	// Check group permission for creating share link
	if !user.Edges.Group.Permissions.Enabled(int(types.GroupPermissionShare)) {
		return "", serializer.NewError(serializer.CodeGroupNotAllowed, "Group permission denied", nil)
	}

	uri, err := fs.NewUriFromString(service.Uri)
	if err != nil {
		return "", serializer.NewError(serializer.CodeParamErr, "unknown uri", err)
	}

	expires, downloads, err := normalizeSharePolicy(time.Now(), service.Expire, service.RemainDownloads)
	if err != nil {
		return "", serializer.NewError(serializer.CodeParamErr, "Invalid temporary share policy", err)
	}

	share, err := m.CreateOrUpdateShare(c, uri, &manager.CreateShareArgs{
		IsPrivate:       service.IsPrivate,
		Password:        service.Password,
		RemainDownloads: downloads,
		Expire:          expires,
		ExistedShareID:  existed,
		ShareView:       service.ShareView,
		ShowReadMe:      service.ShowReadMe,
	})
	if err != nil {
		return "", err
	}

	base := dep.SettingProvider().SiteURL(c)
	return explorer.BuildShareLink(share, dep.HashIDEncoder(), base, true), nil
}

func normalizeSharePolicy(now time.Time, expireSeconds, downloads int) (*time.Time, int, error) {
	if expireSeconds <= 0 {
		expireSeconds = int(DefaultShareExpire / time.Second)
	}
	if expireSeconds > int(MaxShareExpire/time.Second) {
		return nil, 0, fmt.Errorf("share expiration must not exceed %d days", int(MaxShareExpire/(24*time.Hour)))
	}

	if downloads <= 0 {
		downloads = DefaultShareDownloadLimit
	}
	if downloads > MaxShareDownloadLimit {
		return nil, 0, fmt.Errorf("share download limit must not exceed %d", MaxShareDownloadLimit)
	}

	expires := now.Add(time.Duration(expireSeconds) * time.Second)
	return &expires, downloads, nil
}

func DeleteShare(c *gin.Context, shareId int) error {
	dep := dependency.FromContext(c)
	user := inventory.UserFromContext(c)
	shareClient := dep.ShareClient()

	ctx := context.WithValue(c, inventory.LoadShareFile{}, true)
	var (
		share *ent.Share
		err   error
	)
	if user.Edges.Group.Permissions.Enabled(int(types.GroupPermissionIsAdmin)) {
		share, err = shareClient.GetByID(ctx, shareId)
	} else {
		share, err = shareClient.GetByIDUser(ctx, shareId, user.ID)
	}
	if err != nil {
		return serializer.NewError(serializer.CodeNotFound, "share not found", err)
	}

	if err := shareClient.Delete(c, share.ID); err != nil {
		return serializer.NewError(serializer.CodeDBError, "Failed to delete share", err)
	}

	return nil
}
