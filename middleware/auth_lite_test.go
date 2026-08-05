package middleware

import (
	"net/http/httptest"
	"testing"

	"github.com/cloudreve/Cloudreve/v4/application/dependency"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/inventory/types"
	"github.com/cloudreve/Cloudreve/v4/pkg/cache"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/fs"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/manager"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
)

func TestUploadCallbackRejectsInvalidSecret(t *testing.T) {
	logger := logging.NewConsoleLogger(logging.LevelError)
	kv := cache.NewMemoStore("", logger)
	sessionID := "session-id"
	require.NoError(t, kv.Set(manager.UploadSessionCachePrefix+sessionID, fs.UploadSession{
		CallbackSecret: "expected-secret",
		Policy:         &ent.StoragePolicy{Type: string(types.PolicyTypeOss)},
	}, 60))

	dep := dependency.NewDependency(
		dependency.WithKV(kv),
		dependency.WithLogger(logger),
	)
	recorder := httptest.NewRecorder()
	c, engine := gin.CreateTestContext(recorder)
	engine.ContextWithFallback = true
	request := httptest.NewRequest("POST", "/api/v4/callback/oss/session-id/wrong-secret", nil)
	c.Request = request.WithContext(dep.ForkWithLogger(request.Context(), logger))
	c.Params = gin.Params{
		{Key: "sessionID", Value: sessionID},
		{Key: "key", Value: "wrong-secret"},
	}

	err := uploadCallbackCheck(c, types.PolicyTypeOss)
	require.ErrorContains(t, err, "Invalid callback secret")
}
