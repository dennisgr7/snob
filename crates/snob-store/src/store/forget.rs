//! Forgetting an account's history: what it walked and what the monitor
//! made of it, and nothing that paces it. **The budget, the sitting, the
//! cooldown and every other rule's memory stay**, so forgetting never lets
//! an account send sooner than it could have before (AGENTS.md, "A human
//! pace, and a hard stop on the first push-back").
//!
//! Every table of the account's database is in exactly one of [`FORGOTTEN`]
//! and [`KEPT`], and a test fails on a new one that is in neither.

use super::{Store, StoreError};

/// What goes, children before what they name.
///
/// - `snapshot_members`, `snapshots`: every list captured, whole or not.
/// - `watch_marks`, `watch_runs`, `watch_deliveries`, `watch_renames`,
///   `watch_renames_sent`, `watch_posts`: the monitor's baselines, its run
///   log, the reports still owed to a webhook (never sent now) and the posts
///   it saw. When the monitor last started is `shared.db`'s too, so its
///   schedule holds.
/// - `own_writes`: the follows and unfollows applied to a stored list.
/// - `automation_marks`: what each automation's actions found last.
/// - `username_history`, `accounts`, `users`: the accounts seen and their
///   names, but the rows `watch_post_marks` still names.
pub const FORGOTTEN: [&str; 13] = [
    "snapshot_members",
    "automation_marks",
    "watch_marks",
    "watch_runs",
    "watch_deliveries",
    "watch_renames",
    "watch_renames_sent",
    "watch_posts",
    "own_writes",
    "snapshots",
    "username_history",
    "accounts",
    "users",
];

/// What stays, each because a rule reads it before sending:
///
/// - `rate_budget`: the pace, the day's requests and the writes (GCRA).
/// - `sitting`: the sitting and the rest after it.
/// - `cooldowns`: the stop after a push-back.
/// - `account_reads`: the accounts read off lists in the last day.
/// - `answers`: the latest answers the slow-answer rule waits by.
/// - `mcp_acts`: the agents' writes their daily cap counts.
/// - `watch_post_marks`: when the monitor last read the posts, which keeps
///   it to once an hour (and the account rows it names).
/// - `watch_state`: when an interval schedule started counting.
/// - `meta`: the database's own facts.
pub const KEPT: [&str; 9] = [
    "rate_budget",
    "sitting",
    "cooldowns",
    "account_reads",
    "answers",
    "mcp_acts",
    "watch_post_marks",
    "watch_state",
    "meta",
];

impl Store {
    /// Deletes the account's history ([`FORGOTTEN`]) in one transaction,
    /// keeps what paces it ([`KEPT`]), then folds the file down so the room
    /// is given back to the disk.
    pub fn forget_history(&mut self) -> Result<(), StoreError> {
        let held = self.conn_mut().transaction()?;
        for table in FORGOTTEN {
            let sql = match table {
                "accounts" => {
                    "DELETE FROM accounts WHERE pk NOT IN (SELECT account_pk FROM watch_post_marks)"
                }
                "users" => "DELETE FROM users WHERE pk NOT IN (SELECT pk FROM accounts)",
                _ => &format!("DELETE FROM {table}"),
            };
            held.execute(sql, [])?;
        }
        held.commit()?;
        self.conn().execute_batch(
            "VACUUM;
             PRAGMA wal_checkpoint(TRUNCATE);",
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(store: &Store) -> Vec<String> {
        let mut found = store
            .conn()
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let names = found.query_map([], |row| row.get(0)).unwrap();
        names.collect::<Result<_, _>>().unwrap()
    }

    fn rows(store: &Store, table: &str) -> i64 {
        store
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn every_table_is_either_forgotten_or_kept() {
        let store = Store::in_memory().unwrap();
        for table in tables(&store) {
            let forgotten = FORGOTTEN.contains(&table.as_str());
            let kept = KEPT.contains(&table.as_str());
            assert!(forgotten != kept, "{table} must be in exactly one list");
        }
        assert_eq!(tables(&store).len(), FORGOTTEN.len() + KEPT.len());
    }

    #[test]
    fn forgetting_keeps_exactly_what_paces_the_account() {
        let mut store = Store::in_memory().unwrap();
        store
            .conn()
            .execute_batch(
                "INSERT INTO users (pk, username, first_seen, last_seen)
                   VALUES (1, 'me', 0, 0), (2, 'someone', 0, 0), (3, 'watched', 0, 0);
                 INSERT INTO username_history (pk, username, changed_at) VALUES (2, 'old', 0);
                 INSERT INTO accounts (pk, is_self, added_at) VALUES (1, 1, 0), (3, 0, 0);
                 INSERT INTO snapshots (id, account_pk, kind, source, started_at)
                   VALUES (1, 1, 'followers', 'live', 0);
                 INSERT INTO snapshot_members (snapshot_id, user_pk, ordinal) VALUES (1, 2, 0);
                 INSERT INTO watch_marks (account_pk, kind, snapshot_id, compared_at)
                   VALUES (1, 'followers', 1, 0);
                 INSERT INTO watch_runs (account_pk, started_at) VALUES (1, 0);
                 INSERT INTO watch_deliveries (run_id, account_pk, created_at, body)
                   VALUES ('r', 1, 0, '{}');
                 INSERT INTO watch_renames (account_pk, cursor, marked_at) VALUES (1, 1, 0);
                 INSERT INTO watch_renames_sent (account_pk, history_id, sent_at) VALUES (1, 1, 0);
                 INSERT INTO watch_posts (account_pk, media_pk, code, taken_at, first_seen_at)
                   VALUES (3, '7', 'C', 0, 0);
                 INSERT INTO own_writes (at, target_pk, verb) VALUES (0, 2, 'follow');
                 INSERT INTO automation_marks (automation, node, taken_at, members)
                   VALUES ('a1', 2, 0, '[2]');
                 INSERT INTO rate_budget (bucket, tat_ms, emission_ms, burst_ms, updated_at_ms)
                   VALUES ('pace', 1, 1, 1, 1);
                 INSERT INTO sitting (id, pages, last_page_at) VALUES (1, 4, 0);
                 INSERT INTO cooldowns (scope, until_ms, set_at_ms, reason)
                   VALUES ('session', 9, 0, 'rate_limited');
                 INSERT INTO account_reads (at_ms, accounts) VALUES (0, 12);
                 INSERT INTO answers (at_ms, operation, status, waited_ms, via)
                   VALUES (0, 'followers', 200, 10, 'browser');
                 INSERT INTO mcp_acts (at_ms, action, via) VALUES (0, 'follow', 'form');
                 INSERT INTO watch_post_marks (account_pk, baseline_at, read_at, newest_taken_at)
                   VALUES (3, 0, 0, 0);
                 INSERT INTO watch_state (key, value) VALUES ('interval_seeded_at', 0);",
            )
            .unwrap();
        let meta = rows(&store, "meta");
        for table in FORGOTTEN.iter().chain(&KEPT) {
            assert!(
                rows(&store, table) > 0 || *table == "meta",
                "{table} was not filled"
            );
        }

        store.forget_history().unwrap();

        for table in KEPT {
            let expected = if table == "meta" { meta } else { 1 };
            assert_eq!(rows(&store, table), expected, "{table} kept");
        }
        for table in FORGOTTEN {
            // The account the post marks name, and its row of users.
            let expected = i64::from(matches!(table, "accounts" | "users"));
            assert_eq!(rows(&store, table), expected, "{table} forgotten");
        }
        let left: i64 = store
            .conn()
            .query_row("SELECT pk FROM accounts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 3, "the account whose posts are watched");
    }
}
