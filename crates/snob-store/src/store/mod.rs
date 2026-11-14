//! SQLite storage: one file per account, and `shared.db` for what every
//! account shares. Synchronous on purpose: transactions take microseconds.

pub mod accounts;
pub mod answers;
pub mod deliveries;
pub mod mcp_acts;
mod migrations;
pub mod own_writes;
pub mod rate_budget;
pub mod shared;
pub mod snapshots;
pub mod users;
pub mod watch;
pub mod watch_posts;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::Migrations;
use thiserror::Error;

use crate::paths::{AccountPaths, PathError};
use snob_core::Pk;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not apply migrations: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error(transparent)]
    Paths(#[from] PathError),
    /// A downgrade, or an older `snob` earlier on the `PATH`, met a database
    /// with more migrations than it has. Said in our words so that nobody
    /// deletes a file that is not broken.
    #[error(
        "the database at {path} was written by a newer version of snob: it is at schema \
         {found} and this build knows {known}.\nUpgrade snob, or point this one at a \
         different data directory."
    )]
    SchemaFromNewerSnob {
        path: String,
        found: i64,
        known: usize,
    },
    /// A page arrived for a walk another process adopted: not a fault, and the
    /// walk stops rather than write a second stream of pages into one capture.
    #[error("another process took over this walk")]
    ClaimTaken,
}

/// SQLite has signed 64-bit integers only; Instagram ids are far below the
/// ceiling, so the cast is exact. **The only road**: [`Pk`] implements neither
/// `ToSql` nor `FromSql`, and `rusqlite`'s `fallible_uint` stays off.
#[inline]
pub(crate) fn pk_to_sql(pk: Pk) -> i64 {
    pk.get() as i64
}

#[inline]
pub(crate) fn pk_from_sql(value: i64) -> Pk {
    Pk::new(value as u64)
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// The account's database, created with its directory on first use.
    pub fn open(paths: &AccountPaths) -> Result<Self, StoreError> {
        paths.ensure_dirs()?;
        Self::open_at(&paths.db_file())
    }

    /// The account's database if it has one, never creating it or its
    /// directory: a pass over other accounts must not bring back one that
    /// `purge` removed while it ran.
    pub fn open_existing(paths: &AccountPaths) -> Result<Option<Self>, StoreError> {
        let flags =
            rusqlite::OpenFlags::default().difference(rusqlite::OpenFlags::SQLITE_OPEN_CREATE);
        match Connection::open_with_flags(paths.db_file(), flags) {
            Ok(conn) => Self::opened(conn, &paths.db_file()).map(Some),
            Err(rusqlite::Error::SqliteFailure(e, _))
                if e.code == rusqlite::ErrorCode::CannotOpen && !paths.db_file().exists() =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The account's database **for a report**: read, and nothing stored
    /// changed. `None` when it has none. See [`read_only`].
    pub fn read_existing(paths: &AccountPaths) -> Result<Option<Self>, StoreError> {
        let conn = read_only(&paths.db_file(), &migrations::MIGRATIONS, migrations::COUNT)?;
        Ok(conn.map(|conn| Self { conn }))
    }

    pub fn open_at(path: &Path) -> Result<Self, StoreError> {
        Self::opened(Connection::open(path)?, path)
    }

    fn opened(mut conn: Connection, path: &Path) -> Result<Self, StoreError> {
        configure(&conn)?;
        reject_newer_schema(&conn, path, migrations::COUNT)?;
        migrate(&mut conn, &migrations::MIGRATIONS)?;
        Ok(Self { conn })
    }

    /// Throwaway database for tests.
    #[doc(hidden)]
    pub fn in_memory() -> Result<Self, StoreError> {
        Self::opened(Connection::open_in_memory()?, Path::new(":memory:"))
    }

    /// Read access for the child modules and the CLI. Writes that span several
    /// tables go through functions that manage their own transaction.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// One remembered fact, by name, from the `meta` table.
    pub fn remembered(&self, key: &str) -> Result<Option<String>, StoreError> {
        meta_get(&self.conn, key)
    }

    /// Writes one.
    pub fn remember(&self, key: &str, value: &str) -> Result<(), StoreError> {
        meta_set(&self.conn, key, value)
    }
}

/// Runs the migration chain with foreign keys off, the **only** place that
/// can: the chain is one transaction, and `PRAGMA foreign_keys` inside one is
/// a no-op. Left on, the first table rebuilt by create-copy-drop-rename would
/// cascade its `DROP TABLE` and silently empty `snapshot_members`.
fn migrate(conn: &mut Connection, migrations: &Migrations<'_>) -> Result<(), StoreError> {
    let before = user_version(conn).unwrap_or(0);

    conn.pragma_update(None, "foreign_keys", "OFF")?;
    let outcome = migrations.to_latest(conn);
    // Back on even if the chain failed: the connection is handed back.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Another process may have migrated first (the runner reads the version
    // before opening its transaction); the second then fails on "table already
    // exists". The chain is one transaction, so with nothing pending the
    // error was only about who did it.
    if let Err(e) = outcome {
        if migrations
            .pending_migrations(conn)
            .map_or(true, |pending| pending > 0)
        {
            return Err(e.into());
        }
        tracing::debug!(error = %e, "another process migrated the database first");
    }

    // `DROP INDEX` only frees pages (42% of the file for the index 010
    // removes); the file shrinks on `VACUUM`, which cannot run inside the
    // chain's transaction. Only after a migration ran, and best-effort.
    if user_version(conn).unwrap_or(0) > before
        && let Err(e) = conn.execute_batch("VACUUM")
    {
        tracing::debug!(error = %e, "the database could not be compacted after migrating");
    }

    Ok(())
}

fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// How SQLite would run `sql`, one step a line, for the tests that hold a
/// statement to its index.
#[cfg(test)]
pub(crate) fn plan(conn: &Connection, sql: &str) -> String {
    let mut statement = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("the statement compiles");
    let unbound = vec![rusqlite::types::Null; statement.parameter_count()];
    statement
        .query_map(rusqlite::params_from_iter(unbound), |row| {
            row.get::<_, String>(3)
        })
        .expect("the plan is read")
        .collect::<Result<Vec<_>, _>>()
        .expect("every step is text")
        .join("\n")
}

/// What the migration runner counts up to. `0` on a database that has none.
fn user_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Settings applied on every open, by `SqliteRateBudget::open` and
/// `Shared::open` too. `foreign_keys` is not here: [`migrate`] owns it.
fn configure(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(Duration::from_millis(5_000))?;

    // `journal_mode` returns a row, so `pragma_update` would fail on it.
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;

    conn.pragma_update(None, "synchronous", "NORMAL")?;

    // Keeps a temporary b-tree (a query's working copy of a list) out of
    // TMPDIR, which `purge` does not know about.
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -8_000)?; // 8 MiB

    // Deleted rows are overwritten, not just unlinked: pruning removes
    // content without removing the file.
    conn.pragma_update(None, "secure_delete", "ON")?;

    // The log keeps the pre-image of what `secure_delete` scrubbed; bounding
    // it bounds how much of that survives.
    conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)?;

    // The schema of a file at a guessable path is executable content, and
    // nothing here needs a virtual table or a function in the schema.
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;

    Ok(())
}

/// The database at `path`, opened to be read and never changed; `None` when
/// there is no file. `query_only` refuses writes and nothing is checkpointed
/// on close. A schema from an older snob is copied into memory and migrated
/// there; one from a newer snob is refused.
pub(crate) fn read_only(
    path: &Path,
    migrations: &Migrations<'_>,
    known: usize,
) -> Result<Option<Connection>, StoreError> {
    if !path.exists() {
        return Ok(None);
    }
    // Read-write, not `SQLITE_OPEN_READ_ONLY`: a read-only connection cannot
    // set up the WAL index when the log files are absent. `query_only` is
    // what keeps it from writing.
    let flags = rusqlite::OpenFlags::default().difference(rusqlite::OpenFlags::SQLITE_OPEN_CREATE);
    let conn = match Connection::open_with_flags(path, flags) {
        Ok(conn) => conn,
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::CannotOpen && !path.exists() =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    conn.busy_timeout(Duration::from_millis(5_000))?;
    conn.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    conn.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.pragma_update(None, "query_only", "ON")?;
    reject_newer_schema(&conn, path, known)?;
    if usize::try_from(user_version(&conn)?).is_ok_and(|found| found == known) {
        return Ok(Some(conn));
    }

    let mut copy = Connection::open_in_memory()?;
    // Every page in one step; one that met a writer is retried after the pause.
    rusqlite::backup::Backup::new(&conn, &mut copy)?.run_to_completion(
        i32::MAX,
        Duration::from_millis(50),
        None,
    )?;
    drop(conn);
    configure(&copy)?;
    migrate(&mut copy, migrations)?;
    copy.pragma_update(None, "query_only", "ON")?;
    Ok(Some(copy))
}

/// Refuses a database written by a newer build. `known` is the length of the
/// file's own chain: `shared.db` has one of its own.
fn reject_newer_schema(conn: &Connection, path: &Path, known: usize) -> Result<(), StoreError> {
    let found = user_version(conn)?;
    if found as usize > known {
        return Err(StoreError::SchemaFromNewerSnob {
            path: path.display().to_string(),
            found,
            known,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use rusqlite_migration::M;

    use super::*;

    #[test]
    fn an_id_survives_the_round_trip() {
        for n in [0, 1, 4_340_136_074, 71_234_567_890, i64::MAX as u64] {
            let pk = Pk::new(n);
            assert_eq!(pk_from_sql(pk_to_sql(pk)), pk);
        }
    }

    #[test]
    fn migrations_are_applied_on_open() {
        let db = Store::in_memory().unwrap();
        let tables: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'users'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 1);
    }

    /// Proves STRICT really applied: without it SQLite would accept the text in
    /// an INTEGER column thanks to its loose typing.
    #[test]
    fn tables_reject_the_wrong_types() {
        let db = Store::in_memory().unwrap();
        let result = db.conn().execute(
            "INSERT INTO users (pk, username, first_seen, last_seen)
             VALUES ('not a number', 'x', 0, 0)",
            [],
        );
        assert!(result.is_err(), "STRICT is not active");
    }

    /// Proves the foreign keys are live once the store is open. `migrate` turns
    /// them off for the chain and has to turn them back on.
    #[test]
    fn foreign_keys_are_enforced() {
        let db = Store::in_memory().unwrap();
        let result = db.conn().execute(
            "INSERT INTO snapshot_members (snapshot_id, user_pk, ordinal) VALUES (999, 999, 0)",
            [],
        );
        assert!(result.is_err(), "foreign_keys is not active");
    }

    /// A migration that rebuilds a table must not take the rows of the tables
    /// that reference it; `migrate` says why only it can see to that. So this
    /// drives the real `migrate`, with a second migration shaped like the one
    /// somebody will eventually write.
    #[test]
    fn a_migration_that_rebuilds_a_table_keeps_its_children() {
        // The view has to go first and come back afterwards: SQLite checks
        // every view when a table is renamed, and `usable_snapshots` selects
        // from `snapshots`. Worth knowing before writing a real rebuild.
        let rebuild_snapshots = "
            DROP VIEW usable_snapshots;
            CREATE TABLE snapshots_new (
              id             INTEGER PRIMARY KEY,
              account_pk     INTEGER NOT NULL REFERENCES accounts(pk) ON DELETE CASCADE,
              kind           TEXT    NOT NULL,
              source         TEXT    NOT NULL DEFAULT 'live',
              started_at     INTEGER NOT NULL,
              taken_at       INTEGER,
              complete       INTEGER NOT NULL DEFAULT 0,
              member_count   INTEGER NOT NULL DEFAULT 0,
              declared_count INTEGER,
              pages          INTEGER NOT NULL DEFAULT 0,
              requests       INTEGER NOT NULL DEFAULT 0,
              next_cursor    TEXT,
              resumes        INTEGER NOT NULL DEFAULT 0,
              stopped_by     TEXT
            ) STRICT;
            INSERT INTO snapshots_new SELECT
              id, account_pk, kind, source, started_at, taken_at, complete,
              member_count, declared_count, pages, requests, next_cursor,
              resumes, stopped_by FROM snapshots;
            DROP TABLE snapshots;
            ALTER TABLE snapshots_new RENAME TO snapshots;
            CREATE VIEW usable_snapshots AS
              SELECT * FROM snapshots WHERE complete = 1 AND taken_at IS NOT NULL;";

        let mut conn = Connection::open_in_memory().unwrap();
        configure(&conn).unwrap();

        // The database as it is today, with one snapshot and one member in it.
        let first = Migrations::new(vec![M::up(include_str!("sql/001_initial.sql"))]);
        migrate(&mut conn, &first).unwrap();
        conn.execute_batch(
            "INSERT INTO users (pk, username, first_seen, last_seen) VALUES (1, 'someone', 0, 0);
             INSERT INTO accounts (pk, is_self, added_at) VALUES (1, 1, 0);
             INSERT INTO snapshots (id, account_pk, kind, source, started_at, taken_at, complete)
               VALUES (1, 1, 'followers', 'live', 0, 0, 1);
             INSERT INTO snapshot_members (snapshot_id, user_pk, ordinal) VALUES (1, 1, 0);",
        )
        .unwrap();

        let second = Migrations::new(vec![
            M::up(include_str!("sql/001_initial.sql")),
            M::up(rebuild_snapshots),
        ]);
        migrate(&mut conn, &second).unwrap();

        let members: i64 = conn
            .query_row("SELECT count(*) FROM snapshot_members", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            members, 1,
            "the rebuild cascaded the members away and left the snapshot claiming to be complete"
        );

        // And the keys are live again afterwards, or the next write is unguarded.
        let orphan = conn.execute(
            "INSERT INTO snapshot_members (snapshot_id, user_pk, ordinal) VALUES (999, 999, 0)",
            [],
        );
        assert!(orphan.is_err(), "migrate left foreign_keys off");
    }

    #[test]
    fn the_file_uses_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Store::open_at(&tmp.path().join("test.db")).unwrap();
        let mode: String = db
            .conn()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    /// Opening an account that has no database leaves nothing behind.
    #[test]
    fn an_account_without_a_database_is_not_given_one() {
        let tmp = tempfile::tempdir().unwrap();
        let account = crate::paths::AppPaths::rooted_at(tmp.path()).account(Pk::new(42));

        assert!(Store::open_existing(&account).unwrap().is_none());
        assert!(!account.dir().exists(), "nothing was created for it");

        drop(Store::open(&account).unwrap());
        assert!(Store::open_existing(&account).unwrap().is_some());
    }

    /// Reading for a report changes nothing in the file: not a database at
    /// the current schema, which refuses a write through it, and not one an
    /// older snob left, which is read migrated without being migrated.
    #[test]
    fn a_report_reads_the_database_and_leaves_it_as_it_was() {
        let tmp = tempfile::tempdir().unwrap();
        let account = crate::paths::AppPaths::rooted_at(tmp.path()).account(Pk::new(42));
        assert!(Store::read_existing(&account).unwrap().is_none());
        assert!(!account.dir().exists(), "nothing was created for it");

        // Two migrations short of the current schema, as an older snob left it.
        account.ensure_dirs().unwrap();
        let path = account.db_file();
        {
            let mut conn = Connection::open(&path).unwrap();
            configure(&conn).unwrap();
            let older = Migrations::new(
                migrations::CHAIN[..migrations::COUNT - 2]
                    .iter()
                    .map(|sql| rusqlite_migration::M::up(sql))
                    .collect(),
            );
            migrate(&mut conn, &older).unwrap();
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
                .unwrap();
        }
        let before = std::fs::read(&path).unwrap();

        let read = Store::read_existing(&account).unwrap().unwrap();
        assert_eq!(
            user_version(read.conn()).unwrap() as usize,
            migrations::COUNT,
            "the report reads the current schema"
        );
        drop(read);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "the file was changed"
        );

        drop(Store::open(&account).unwrap());
        let read = Store::read_existing(&account).unwrap().unwrap();
        assert!(
            read.remember("key", "value").is_err(),
            "a write went through a report's connection"
        );
    }

    /// A database from a newer snob says so, rather than letting the migration
    /// runner say it in its own words; `StoreError::SchemaFromNewerSnob` says
    /// why that matters.
    #[test]
    fn a_database_from_a_newer_snob_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ahead.db");

        // A real database, then wound forward the way a newer build would leave
        // it. The view has to be there or the *older*-schema check fires first.
        Store::open_at(&path).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", (migrations::COUNT + 3) as i64)
                .unwrap();
        }

        let Err(error) = Store::open_at(&path) else {
            panic!("a database from a newer snob should be refused");
        };
        assert!(
            matches!(error, StoreError::SchemaFromNewerSnob { .. }),
            "{error}"
        );
        let said = error.to_string();
        assert!(said.contains("newer version of snob"), "{said}");
        assert!(
            !said.contains("Delete"),
            "deleting it throws away every capture: {said}"
        );
    }
}
