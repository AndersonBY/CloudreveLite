package user

import (
	"context"
	"encoding/json"
	"testing"
	"time"

	"entgo.io/ent/dialect/sql"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/ent/apicredential"
	"github.com/cloudreve/Cloudreve/v4/ent/user"
	"github.com/cloudreve/Cloudreve/v4/inventory/types"
	"github.com/cloudreve/Cloudreve/v4/pkg/boolset"
	"github.com/stretchr/testify/require"
)

func newAPIKeyTestClient(t *testing.T) (*ent.Client, int) {
	t.Helper()
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:"+t.Name()+"?mode=memory&cache=shared")
	require.NoError(t, err)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	group, err := client.Group.Create().
		SetName("User").
		SetPermissions(&boolset.BooleanSet{}).
		SetSettings(&types.GroupSetting{}).
		Save(ctx)
	require.NoError(t, err)
	owner, err := client.User.Create().
		SetEmail("agent@example.com").
		SetNick("agent").
		SetStatus(user.StatusActive).
		SetGroupID(group.ID).
		Save(ctx)
	require.NoError(t, err)
	return client, owner.ID
}

func TestAPIKeyLifecycle(t *testing.T) {
	ctx := context.Background()
	client, userID := newAPIKeyTestClient(t)
	created, err := CreateAPIKey(ctx, client, userID, CreateAPIKeyRequest{Name: "Codex", ExpiresIn: 86400})
	require.NoError(t, err)
	require.True(t, IsAPIKey(created.Key))
	require.NotEqual(t, created.ID, created.Key)

	listed, err := ListAPIKeys(ctx, client, userID)
	require.NoError(t, err)
	require.Len(t, listed, 1)
	raw, err := json.Marshal(listed)
	require.NoError(t, err)
	require.NotContains(t, string(raw), created.Key)

	authenticatedUserID, err := AuthenticateAPIKey(ctx, client, created.Key)
	require.NoError(t, err)
	require.Equal(t, userID, authenticatedUserID)
	stored, err := client.APICredential.Query().Where(apicredential.PublicID(created.ID)).Only(ctx)
	require.NoError(t, err)
	require.NotNil(t, stored.LastUsedAt)

	_, err = AuthenticateAPIKey(ctx, client, created.Key+"invalid")
	require.ErrorIs(t, err, ErrInvalidAPICredential)
	require.NoError(t, RevokeAPIKey(ctx, client, userID, created.ID))
	_, err = AuthenticateAPIKey(ctx, client, created.Key)
	require.ErrorIs(t, err, ErrInvalidAPICredential)
}

func TestExpiredAPIKeyIsRejected(t *testing.T) {
	ctx := context.Background()
	client, userID := newAPIKeyTestClient(t)
	created, err := CreateAPIKey(ctx, client, userID, CreateAPIKeyRequest{Name: "Expired", ExpiresIn: 86400})
	require.NoError(t, err)
	credential, err := client.APICredential.Query().Where(apicredential.PublicID(created.ID)).Only(ctx)
	require.NoError(t, err)
	require.NoError(t, client.APICredential.UpdateOne(credential).SetExpiresAt(time.Now().Add(-time.Minute)).Exec(ctx))

	_, err = AuthenticateAPIKey(ctx, client, created.Key)
	require.ErrorIs(t, err, ErrInvalidAPICredential)
}

func TestPairingCodeCanOnlyBeExchangedOnce(t *testing.T) {
	ctx := context.Background()
	client, userID := newAPIKeyTestClient(t)
	pairing, err := CreatePairingCode(ctx, client, userID, CreateAPIKeyRequest{Name: "Desktop CLI", ExpiresIn: 86400})
	require.NoError(t, err)

	created, err := ExchangePairingCode(ctx, client, pairing.Code)
	require.NoError(t, err)
	_, err = AuthenticateAPIKey(ctx, client, created.Key)
	require.NoError(t, err)
	_, err = ExchangePairingCode(ctx, client, pairing.Code)
	require.ErrorIs(t, err, ErrInvalidAPICredential)
}

func TestExpiredPairingCodeIsRejected(t *testing.T) {
	ctx := context.Background()
	client, userID := newAPIKeyTestClient(t)
	pairing, err := CreatePairingCode(ctx, client, userID, CreateAPIKeyRequest{Name: "Desktop CLI", ExpiresIn: 86400})
	require.NoError(t, err)
	publicID, ok := parseCredential(pairing.Code, pairingCodePrefix)
	require.True(t, ok)
	credential, err := client.APICredential.Query().Where(apicredential.PublicID(publicID)).Only(ctx)
	require.NoError(t, err)
	require.NoError(t, client.APICredential.UpdateOne(credential).SetPairingExpiresAt(time.Now().Add(-time.Minute)).Exec(ctx))

	_, err = ExchangePairingCode(ctx, client, pairing.Code)
	require.ErrorIs(t, err, ErrInvalidAPICredential)
}

func TestAPIKeyLastUsedUpdateIsThrottled(t *testing.T) {
	ctx := context.Background()
	client, userID := newAPIKeyTestClient(t)
	created, err := CreateAPIKey(ctx, client, userID, CreateAPIKeyRequest{Name: "Automation", ExpiresIn: 86400})
	require.NoError(t, err)
	credential, err := client.APICredential.Query().Where(apicredential.PublicID(created.ID)).Only(ctx)
	require.NoError(t, err)
	fixed := time.Now().Add(-time.Minute).Truncate(time.Millisecond)
	require.NoError(t, client.APICredential.UpdateOne(credential).SetLastUsedAt(fixed).Exec(ctx))

	_, err = AuthenticateAPIKey(ctx, client, created.Key)
	require.NoError(t, err)
	stored, err := client.APICredential.Query().Where(apicredential.PublicID(created.ID)).Only(ctx)
	require.NoError(t, err)
	require.WithinDuration(t, fixed, *stored.LastUsedAt, time.Millisecond)

}

func TestAPIKeyConfigurationIsBounded(t *testing.T) {
	ctx := context.Background()
	client, userID := newAPIKeyTestClient(t)
	_, err := CreateAPIKey(ctx, client, userID, CreateAPIKeyRequest{Name: " ", ExpiresIn: 86400})
	require.ErrorIs(t, err, ErrInvalidAPIKeyConfig)
	_, err = CreateAPIKey(ctx, client, userID, CreateAPIKeyRequest{Name: "Too long", ExpiresIn: int64((366 * 24 * time.Hour) / time.Second)})
	require.ErrorIs(t, err, ErrInvalidAPIKeyConfig)
}
