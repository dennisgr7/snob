//! Schema migrations. A published one is never edited; the next is added.
//!
//! - **No `PRAGMA foreign_keys` inside a migration**: it is a no-op in the
//!   chain's transaction. [`super::migrate`] turns it off around the run.
//! - **Rebuild a table by create-copy-drop-rename**: `writable_schema` is
//!   forbidden by `SQLITE_DBCONFIG_DEFENSIVE`.
//! - `foreign_key_check` misreports on `snapshot_members` (`WITHOUT ROWID`,
//!   so a NULL rowid): a dangling row surfaces as `InvalidColumnType`.

use std::sync::LazyLock;

use rusqlite_migration::{M, Migrations};

/// Every migration, in the order they are applied.
pub(crate) const CHAIN: [&str; 20] = [
    include_str!("sql/001_initial.sql"),
    include_str!("sql/002_watch.sql"),
    include_str!("sql/003_deliveries.sql"),
    include_str!("sql/004_claims.sql"),
    include_str!("sql/005_destination.sql"),
    include_str!("sql/006_rename_cursor.sql"),
    include_str!("sql/007_renames_sent.sql"),
    include_str!("sql/008_watch_state.sql"),
    include_str!("sql/009_newest_run.sql"),
    include_str!("sql/010_drop_member_index.sql"),
    include_str!("sql/011_newest_run_seeks.sql"),
    include_str!("sql/012_account_reads.sql"),
    include_str!("sql/013_mcp_acts.sql"),
    include_str!("sql/014_sitting.sql"),
    include_str!("sql/015_answers.sql"),
    include_str!("sql/016_own_writes.sql"),
    include_str!("sql/017_delivery_form.sql"),
    include_str!("sql/018_watch_posts.sql"),
    include_str!("sql/019_run_posts.sql"),
    include_str!("sql/020_run_times.sql"),
];

/// What `user_version` counts up to.
pub const COUNT: usize = CHAIN.len();

pub static MIGRATIONS: LazyLock<Migrations<'static>> =
    LazyLock::new(|| Migrations::new(CHAIN.iter().map(|sql| M::up(sql)).collect()));

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs every migration from scratch against an empty database. It is the
    /// safety net that makes adding migrations cheap.
    #[test]
    fn the_migration_chain_is_valid() {
        MIGRATIONS.validate().unwrap();
    }
}
