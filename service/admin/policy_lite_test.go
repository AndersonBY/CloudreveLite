package admin

import (
	"context"
	"fmt"
	"net/http/httptest"
	"strconv"
	"testing"

	"entgo.io/ent/dialect/sql"
	"github.com/cloudreve/Cloudreve/v4/application/dependency"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/inventory/types"
	"github.com/cloudreve/Cloudreve/v4/pkg/cache"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/gin-gonic/gin"
	"github.com/stretchr/testify/require"
)

func TestLitePoliciesListOnlyOSSAndS3(t *testing.T) {
	c, client := newLitePolicyTestContext(t)
	for _, policyType := range []types.PolicyType{
		types.PolicyTypeLocal,
		types.PolicyTypeOss,
		types.PolicyTypeQiniu,
		types.PolicyTypeS3,
	} {
		createPolicyFixture(t, client, policyType)
	}

	result, err := (&AdminListService{
		Page:       1,
		PageSize:   10,
		Conditions: map[string]string{},
	}).Policies(c)

	require.NoError(t, err)
	require.Equal(t, 2, result.Pagination.TotalItems)
	require.Len(t, result.Policies, 2)
	require.Equal(t, types.PolicyTypeOss, result.Policies[0].Type)
	require.Equal(t, types.PolicyTypeS3, result.Policies[1].Type)

	_, err = (&AdminListService{
		Page:       1,
		PageSize:   10,
		Conditions: map[string]string{policyTypeCondition: types.PolicyTypeLocal},
	}).Policies(c)
	require.ErrorContains(t, err, litePolicyTypeError)
}

func TestLitePolicyMutationsRejectUnsupportedTypes(t *testing.T) {
	c, client := newLitePolicyTestContext(t)

	created, err := (&CreateStoragePolicyService{Policy: policyFixture(types.PolicyTypeOss)}).Create(c)
	require.NoError(t, err)
	require.Equal(t, types.PolicyTypeOss, created.StoragePolicy.Type)

	_, err = (&CreateStoragePolicyService{Policy: policyFixture(types.PolicyTypeLocal)}).Create(c)
	require.ErrorContains(t, err, litePolicyTypeError)
	require.Equal(t, 1, policyCount(t, client))

	c.Params = gin.Params{{Key: "id", Value: strconv.Itoa(created.StoragePolicy.ID)}}
	updated, err := (&UpdateStoragePolicyService{Policy: policyFixture(types.PolicyTypeS3)}).Update(c)
	require.NoError(t, err)
	require.Equal(t, types.PolicyTypeS3, updated.StoragePolicy.Type)

	_, err = (&UpdateStoragePolicyService{Policy: policyFixture(types.PolicyTypeQiniu)}).Update(c)
	require.ErrorContains(t, err, litePolicyTypeError)

	legacy := createPolicyFixture(t, client, types.PolicyTypeQiniu)
	c.Params = gin.Params{{Key: "id", Value: strconv.Itoa(legacy.ID)}}
	_, err = (&UpdateStoragePolicyService{Policy: policyFixture(types.PolicyTypeS3)}).Update(c)
	require.ErrorContains(t, err, litePolicyTypeError)

	_, err = (&SingleStoragePolicyService{ID: legacy.ID}).Get(c)
	require.ErrorContains(t, err, litePolicyTypeError)

	err = (&SingleStoragePolicyService{ID: legacy.ID}).Delete(c)
	require.ErrorContains(t, err, litePolicyTypeError)

	err = (&CreateStoragePolicyCorsService{Policy: policyFixture(types.PolicyTypeQiniu)}).Create(c)
	require.ErrorContains(t, err, litePolicyTypeError)
}

func newLitePolicyTestContext(t *testing.T) (*gin.Context, *ent.Client) {
	t.Helper()
	driver, err := sql.Open("sqlite3", fmt.Sprintf("file:%s?mode=memory&cache=shared", t.Name()))
	require.NoError(t, err)
	driver.DB().SetMaxOpenConns(1)

	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(context.Background()))

	logger := logging.NewConsoleLogger(logging.LevelError)
	dep := dependency.NewDependency(
		dependency.WithDbClient(client),
		dependency.WithKV(cache.NewMemoStore("", logger)),
		dependency.WithLogger(logger),
		dependency.WithLiteMode(true),
	)
	recorder := httptest.NewRecorder()
	c, engine := gin.CreateTestContext(recorder)
	engine.ContextWithFallback = true
	request := httptest.NewRequest("GET", "/", nil)
	c.Request = request.WithContext(dep.ForkWithLogger(request.Context(), logger))
	return c, client
}

func createPolicyFixture(t *testing.T, client *ent.Client, policyType types.PolicyType) *ent.StoragePolicy {
	t.Helper()
	policy, err := client.StoragePolicy.Create().
		SetName(string(policyType)).
		SetType(string(policyType)).
		Save(context.Background())
	require.NoError(t, err)
	return policy
}

func policyFixture(policyType types.PolicyType) *ent.StoragePolicy {
	return &ent.StoragePolicy{
		Name:     string(policyType),
		Type:     string(policyType),
		Settings: &types.PolicySetting{},
	}
}

func policyCount(t *testing.T, client *ent.Client) int {
	t.Helper()
	count, err := client.StoragePolicy.Query().Count(context.Background())
	require.NoError(t, err)
	return count
}
