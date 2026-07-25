package inventory

import (
	"context"
	"database/sql"
	"database/sql/driver"
	"errors"
	"path/filepath"
	"testing"

	"github.com/stretchr/testify/require"
	"modernc.org/sqlite"
)

func TestSQLiteConnectionRecoversAfterBusyCommit(t *testing.T) {
	dbPath := filepath.Join(t.TempDir(), "commit-busy.db")
	openDB := func() *sql.DB {
		db, err := sql.Open("sqlite3", dbPath)
		require.NoError(t, err)
		db.SetMaxOpenConns(1)
		db.SetMaxIdleConns(1)
		t.Cleanup(func() { require.NoError(t, db.Close()) })
		return db
	}

	writer := openDB()
	reader := openDB()
	ctx := context.Background()

	_, err := writer.ExecContext(ctx, "CREATE TABLE files (id INTEGER PRIMARY KEY)")
	require.NoError(t, err)

	writerTx, err := writer.BeginTx(ctx, nil)
	require.NoError(t, err)
	readerTx, err := reader.BeginTx(ctx, nil)
	require.NoError(t, err)

	_, err = readerTx.ExecContext(ctx, "SELECT * FROM files")
	require.NoError(t, err)
	_, err = writerTx.ExecContext(ctx, "INSERT INTO files (id) VALUES (1)")
	require.NoError(t, err)

	err = writerTx.Commit()
	require.Error(t, err)
	var sqliteErr *sqlite.Error
	require.ErrorAs(t, err, &sqliteErr)
	require.Equal(t, 5, sqliteErr.Code()&0xff)
	require.ErrorIs(t, writerTx.Rollback(), sql.ErrTxDone)

	recoveredTx, err := writer.BeginTx(ctx, nil)
	require.NoError(t, err, "failed COMMIT must not poison the pooled SQLite connection")
	require.NoError(t, recoveredTx.Rollback())
	require.NoError(t, readerTx.Rollback())

	_, err = writer.ExecContext(ctx, "INSERT INTO files (id) VALUES (2)")
	require.NoError(t, err)
}

func TestSQLiteConnectionPragmas(t *testing.T) {
	db, err := sql.Open("sqlite3", filepath.Join(t.TempDir(), "pragmas.db"))
	require.NoError(t, err)
	t.Cleanup(func() { require.NoError(t, db.Close()) })

	var foreignKeys int
	require.NoError(t, db.QueryRow("PRAGMA foreign_keys").Scan(&foreignKeys))
	require.Equal(t, 1, foreignKeys)

	var busyTimeout int
	require.NoError(t, db.QueryRow("PRAGMA busy_timeout").Scan(&busyTimeout))
	require.Equal(t, 5000, busyTimeout)
}

func TestSQLiteTxCommitFailureResetsConnection(t *testing.T) {
	commitErr := errors.New("commit failed")
	tx := &fakeDriverTx{commitErr: commitErr}

	err := (&sqlite3Tx{Tx: tx}).Commit()
	require.ErrorIs(t, err, commitErr)
	require.NotErrorIs(t, err, driver.ErrBadConn)
	require.True(t, tx.rollbackCalled)
}

func TestSQLiteTxFailedResetDiscardsConnection(t *testing.T) {
	commitErr := errors.New("commit failed")
	rollbackErr := errors.New("rollback failed")
	tx := &fakeDriverTx{commitErr: commitErr, rollbackErr: rollbackErr}

	err := (&sqlite3Tx{Tx: tx}).Commit()
	require.ErrorIs(t, err, commitErr)
	require.ErrorIs(t, err, rollbackErr)
	require.ErrorIs(t, err, driver.ErrBadConn)
}

type fakeDriverTx struct {
	commitErr      error
	rollbackErr    error
	rollbackCalled bool
}

func (t *fakeDriverTx) Commit() error {
	return t.commitErr
}

func (t *fakeDriverTx) Rollback() error {
	t.rollbackCalled = true
	return t.rollbackErr
}
