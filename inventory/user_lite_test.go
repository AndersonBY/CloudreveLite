package inventory

import (
	"context"
	"testing"

	"entgo.io/ent/dialect/sql"
	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/ent/user"
	"github.com/cloudreve/Cloudreve/v4/inventory/types"
	"github.com/cloudreve/Cloudreve/v4/pkg/boolset"
	"github.com/stretchr/testify/require"
)

func TestFirstUserBecomesAdminAfterSequenceWasUsed(t *testing.T) {
	ctx := context.Background()
	driver, err := sql.Open("sqlite3", "file:first_user_admin?mode=memory&cache=shared")
	require.NoError(t, err)
	client := ent.NewClient(ent.Driver(driver))
	t.Cleanup(func() { require.NoError(t, client.Close()) })
	require.NoError(t, client.Schema.Create(ctx))

	permissions := &boolset.BooleanSet{}
	adminGroup, err := client.Group.Create().
		SetName("Admin").
		SetPermissions(permissions).
		SetSettings(&types.GroupSetting{}).
		Save(ctx)
	require.NoError(t, err)
	userGroup, err := client.Group.Create().
		SetName("User").
		SetPermissions(permissions).
		SetSettings(&types.GroupSetting{}).
		Save(ctx)
	require.NoError(t, err)

	placeholder, err := client.User.Create().
		SetEmail("deleted@example.com").
		SetNick("deleted").
		SetStatus(user.StatusActive).
		SetGroupID(userGroup.ID).
		Save(ctx)
	require.NoError(t, err)
	require.NoError(t, client.User.DeleteOne(placeholder).Exec(ctx))

	created, err := NewUserClient(client).Create(ctx, &NewUserArgs{
		Email:   "admin@example.com",
		Status:  user.StatusActive,
		GroupID: userGroup.ID,
	})
	require.NoError(t, err)
	require.Greater(t, created.ID, 1)

	stored, err := client.User.Query().Where(user.ID(created.ID)).WithGroup().Only(ctx)
	require.NoError(t, err)
	require.Equal(t, adminGroup.ID, stored.GroupUsers)
	require.Equal(t, "Admin", stored.Edges.Group.Name)
}
