//! What every account shares: `data/shared.db`, with a migration chain of its
//! own. The push-backs any account received, so that each account's budget
//! sees the others' and stops with them ([`snob_core::budget::common_brake`]),
//! the monitor's moments, which are the machine's, and what each automation's
//! runs came to ([`AutomationRun`]).

use std::sync::LazyLock;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};

use snob_core::budget::PushBack;
use snob_core::{Epoch, EpochMs, Pk};

use super::{StoreError, pk_from_sql, pk_to_sql};
use crate::paths::AppPaths;

/// Every migration of `shared.db`, in the order they are applied. The rules at
/// the top of [`super::migrations`] hold here too.
const CHAIN: [&str; 3] = [
    include_str!("sql/shared/001_pushbacks.sql"),
    include_str!("sql/shared/002_automation_runs.sql"),
    include_str!("sql/shared/003_automation_found.sql"),
];

static MIGRATIONS: LazyLock<Migrations<'static>> =
    LazyLock::new(|| Migrations::new(CHAIN.iter().map(|sql| M::up(sql)).collect()));

/// How long a push-back is kept: longer than any cooldown, which is capped at
/// a day, plus the brake's window.
const KEPT: Duration = Duration::from_secs(2 * 24 * 3600);

/// The `meta` key the interval seed is kept under.
const INTERVAL_SEED: &str = "interval_seeded_at";

/// When a timer first asked `snob watch once --when-due` whether a run is
/// due: what a calendar that never ran counts its owed moments from
/// (`schedule::due_asked_since`).
const ASKED_SINCE: &str = "monitor_asked_since";

/// When the monitor last started a run, written before its work: a run that
/// fails on every account may write no row in any `watch_runs`, and would be
/// due again at every look.
const STARTED_AT: &str = "monitor_started_at";

/// Since when the monitor has held a due run back for a critical battery.
const WAITING_FOR_POWER: &str = "monitor_waiting_for_power_since";

/// When it last looked at the battery while holding that run, which tells a
/// monitor still waiting from a marker left by one that was killed.
const WAITING_FOR_POWER_SEEN: &str = "monitor_waiting_for_power_seen";

/// A due run held back for a critical battery, as the monitor wrote it down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerWait {
    /// When it started waiting.
    pub since: Epoch,
    /// When it last looked and found the battery still critical.
    pub seen: Epoch,
}

pub struct Shared {
    conn: Connection,
}

/// How many runs of one automation are kept.
const RUNS_KEPT: i64 = 100;

/// How often a run going writes down that it still is
/// ([`Shared::beat_automation_run`]).
pub const RUN_BEAT: Duration = Duration::from_secs(20);

/// How long a run with no end may go without a sign of life and still be
/// going: three beats missed and some, so a busy database or a slow beat is
/// not taken for a killed run.
const RUN_QUIET_AFTER: i64 = 75;

/// One run of an automation, as its history lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRun {
    pub id: i64,
    pub automation: String,
    pub started_at: Epoch,
    /// `None` while it runs, or when it was killed.
    pub ended_at: Option<Epoch>,
    /// The exit token (`ok`, `rate_limited`, ...); `None` while it runs.
    pub outcome: Option<String>,
    pub requests: u32,
    /// Plain sentences, one a line.
    pub summary: String,
    /// Whether it is still going: no end, and a sign of life within
    /// `RUN_QUIET_AFTER` of the moment it was read at. A run killed is not.
    pub running: bool,
    /// What it found as data (`snob_engine::automations::said`), JSON;
    /// `None` before it ended, and in a run kept before this was.
    pub found: Option<String>,
}

impl Shared {
    /// Opens `shared.db`, creating it on first use.
    /// Opens `shared.db`, creating it on first use. `synchronous = FULL` as
    /// in `SqliteRateBudget::open`: losing a push-back lets the other
    /// accounts out of a brake early.
    pub fn open(paths: &AppPaths) -> Result<Self, StoreError> {
        paths.ensure_dirs()?;
        let path = paths.shared_db_file();
        let mut conn = Connection::open(&path)?;
        super::configure(&conn)?;
        super::reject_newer_schema(&conn, &path, CHAIN.len())?;
        super::migrate(&mut conn, &MIGRATIONS)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { conn })
    }

    /// `shared.db` for a command that only reports: read, never created and
    /// nothing in it changed (`store::read_only`). `None` when there is no
    /// file yet, which is before any account's budget was first opened, or
    /// when the file is from a newer snob.
    pub fn read_existing(paths: &AppPaths) -> Result<Option<Self>, StoreError> {
        match super::read_only(&paths.shared_db_file(), &MIGRATIONS, CHAIN.len()) {
            Ok(conn) => Ok(conn.map(|conn| Self { conn })),
            Err(StoreError::SchemaFromNewerSnob { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The connection, for a test that moves a moment kept here by hand, as
    /// tests move the account database's ([`super::Store::conn`]).
    #[doc(hidden)]
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Records a push-back on `pk`, and forgets the ones too old to matter.
    pub fn record(
        &self,
        pk: Pk,
        at: EpochMs,
        until: EpochMs,
        reason: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO pushbacks (pk, at_ms, until_ms, reason) VALUES (?1, ?2, ?3, ?4)",
            params![pk_to_sql(pk), at.get(), until.get(), reason],
        )?;
        self.conn.execute(
            "DELETE FROM pushbacks WHERE at_ms < ?1",
            params![(at - KEPT).get()],
        )?;
        Ok(())
    }

    /// The push-backs kept at `now`: every one that can still be part of a
    /// brake.
    pub fn pushbacks(&self, now: EpochMs) -> Result<Vec<PushBack>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT pk, at_ms, until_ms FROM pushbacks WHERE at_ms >= ?1")?;
        let rows = statement.query_map(params![(now - KEPT).get()], |row| {
            Ok(PushBack {
                pk: pk_from_sql(row.get(0)?),
                at: EpochMs::new(row.get(1)?),
                until: EpochMs::new(row.get(2)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// A moment kept in `meta` under `key`; `None` when absent or unreadable.
    fn moment(&self, key: &str) -> Result<Option<Epoch>, StoreError> {
        let at = super::meta_get(&self.conn, key)?;
        Ok(at.and_then(|at| at.parse().ok()).map(Epoch::new))
    }

    /// Writes `at` under `key` unless a moment is already there, and answers
    /// with the one kept: of two writers racing, either is right.
    fn keep_first(&self, key: &str, at: Epoch) -> Result<Epoch, StoreError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, at.get().to_string()],
        )?;
        Ok(self.moment(key)?.unwrap_or(at))
    }

    /// When an interval schedule started counting, if it was written down.
    /// One machine, one monitor: every account's runs count from it.
    pub fn interval_seeded_at(&self) -> Result<Option<Epoch>, StoreError> {
        self.moment(INTERVAL_SEED)
    }

    /// Writes the seed down unless one already is, and answers with the one
    /// kept: a later start never moves the moment the interval counts from.
    pub fn seed_interval(&self, at: Epoch) -> Result<Epoch, StoreError> {
        self.keep_first(INTERVAL_SEED, at)
    }

    /// Since when a timer has been asking whether a run is due, if one ever
    /// asked.
    pub fn asked_since(&self) -> Result<Option<Epoch>, StoreError> {
        self.moment(ASKED_SINCE)
    }

    /// Writes down that a timer asked at `at`, unless one already did, and
    /// answers with the moment kept.
    pub fn ask_since(&self, at: Epoch) -> Result<Epoch, StoreError> {
        self.keep_first(ASKED_SINCE, at)
    }

    /// Writes down that a run of `automation` starts, before any of its work
    /// (**a run counts from its start**), and forgets all but its newest
    /// runs. Answers with the row [`Self::finish_automation_run`] fills in.
    pub fn record_automation_run(&self, automation: &str, at: Epoch) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO automation_runs (automation, started_at, alive_at) VALUES (?1, ?2, ?2)",
            params![automation, at.get()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "DELETE FROM automation_runs WHERE automation = ?1 AND id NOT IN
               (SELECT id FROM automation_runs WHERE automation = ?1
                ORDER BY started_at DESC, id DESC LIMIT ?2)",
            params![automation, RUNS_KEPT],
        )?;
        Ok(id)
    }

    /// Writes down that the run `id` is still going at `at`. Nothing once it
    /// has ended: a beat late is not a run brought back.
    pub fn beat_automation_run(&self, id: i64, at: Epoch) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE automation_runs SET alive_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![id, at.get()],
        )?;
        Ok(())
    }

    /// What the run `id` came to: its sentences in `summary`, and what it
    /// found as data in `found`.
    pub fn finish_automation_run(
        &self,
        id: i64,
        at: Epoch,
        outcome: &str,
        requests: u32,
        summary: &str,
        found: Option<&str>,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE automation_runs
             SET ended_at = ?2, outcome = ?3, requests = ?4, summary = ?5, found = ?6
             WHERE id = ?1",
            params![id, at.get(), outcome, i64::from(requests), summary, found],
        )?;
        Ok(())
    }

    /// The newest `limit` runs of `automation`, the newest first, as they
    /// stand at `now`.
    pub fn automation_runs(
        &self,
        automation: &str,
        limit: usize,
        now: Epoch,
    ) -> Result<Vec<AutomationRun>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, automation, started_at, ended_at, outcome, requests, summary,
                    ended_at IS NULL AND alive_at IS NOT NULL AND alive_at >= ?3 - ?4,
                    found
             FROM automation_runs WHERE automation = ?1
             ORDER BY started_at DESC, id DESC LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement.query_map(
            params![automation, limit, now.get(), RUN_QUIET_AFTER],
            |row| {
                Ok(AutomationRun {
                    id: row.get(0)?,
                    automation: row.get(1)?,
                    started_at: Epoch::new(row.get(2)?),
                    ended_at: row.get::<_, Option<i64>>(3)?.map(Epoch::new),
                    outcome: row.get(4)?,
                    requests: u32::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
                    summary: row.get(6)?,
                    running: row.get(7)?,
                    found: row.get(8)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// When a run of `automation` last started, if one ever did.
    pub fn automation_last_started(&self, automation: &str) -> Result<Option<Epoch>, StoreError> {
        let started = self
            .conn
            .query_row(
                "SELECT started_at FROM automation_runs WHERE automation = ?1
                 ORDER BY started_at DESC, id DESC LIMIT 1",
                params![automation],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        Ok(started.map(Epoch::new))
    }

    /// Forgets the runs of an automation that is no more.
    pub fn forget_automation_runs(&self, automation: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM automation_runs WHERE automation = ?1",
            params![automation],
        )?;
        Ok(())
    }

    /// When the monitor last started a run, if it ever wrote one down.
    pub fn monitor_last_started(&self) -> Result<Option<Epoch>, StoreError> {
        self.moment(STARTED_AT)
    }

    /// Writes down that the monitor started a run at `at`. Never moved back:
    /// of two runs racing, the later start is the one kept, in one statement.
    pub fn monitor_started(&self, at: Epoch) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value
             WHERE CAST(meta.value AS INTEGER) < CAST(excluded.value AS INTEGER)",
            params![STARTED_AT, at.get().to_string()],
        )?;
        Ok(())
    }

    /// The run that is due being held back for a critical battery; `None`
    /// while it is not.
    pub fn waiting_for_power(&self) -> Result<Option<PowerWait>, StoreError> {
        let Some(since) = self.moment(WAITING_FOR_POWER)? else {
            return Ok(None);
        };
        let seen = self.moment(WAITING_FOR_POWER_SEEN)?.unwrap_or(since);
        Ok(Some(PowerWait { since, seen }))
    }

    /// Writes down that the monitor is holding a due run for power, at `now`.
    /// The moment it started waiting is kept from the first call; the moment
    /// it last looked moves with every one.
    pub fn wait_for_power(&self, now: Epoch) -> Result<(), StoreError> {
        self.keep_first(WAITING_FOR_POWER, now)?;
        super::meta_set(&self.conn, WAITING_FOR_POWER_SEEN, &now.get().to_string())
    }

    /// The machine is on power again, the battery out of danger, or the
    /// monitor no longer waiting.
    pub fn power_is_back(&self) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM meta WHERE key IN (?1, ?2)",
            params![WAITING_FOR_POWER, WAITING_FOR_POWER_SEEN],
        )?;
        Ok(())
    }

    /// Removes every push-back on `pk`, for `purge --account`.
    pub fn forget(&self, pk: Pk) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM pushbacks WHERE pk = ?1",
            params![pk_to_sql(pk)],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::clock::now_ms;

    /// The first moment the monitor started waiting for power is the one
    /// kept, the last look moves, and both go when power is back.
    #[test]
    fn waiting_for_power_keeps_when_it_started() {
        let (_tmp, shared) = shared();
        assert_eq!(shared.waiting_for_power().unwrap(), None);

        shared.wait_for_power(Epoch::new(100)).unwrap();
        shared.wait_for_power(Epoch::new(200)).unwrap();
        assert_eq!(
            shared.waiting_for_power().unwrap(),
            Some(PowerWait {
                since: Epoch::new(100),
                seen: Epoch::new(200),
            })
        );

        shared.power_is_back().unwrap();
        assert_eq!(shared.waiting_for_power().unwrap(), None);
        shared.wait_for_power(Epoch::new(300)).unwrap();
        assert_eq!(
            shared.waiting_for_power().unwrap(),
            Some(PowerWait {
                since: Epoch::new(300),
                seen: Epoch::new(300),
            }),
            "a new wait starts from its own first look"
        );
    }

    #[test]
    fn the_migration_chain_is_valid() {
        MIGRATIONS.validate().unwrap();
    }

    fn shared() -> (tempfile::TempDir, Shared) {
        let tmp = tempfile::tempdir().unwrap();
        let shared = Shared::open(&AppPaths::rooted_at(tmp.path())).unwrap();
        (tmp, shared)
    }

    #[test]
    fn a_push_back_is_read_back() {
        let (_tmp, shared) = shared();
        let now = now_ms();
        let until = now + Duration::from_secs(3600);
        shared.record(Pk::new(1), now, until, "429").unwrap();
        assert_eq!(
            shared.pushbacks(now).unwrap(),
            vec![PushBack {
                pk: Pk::new(1),
                at: now,
                until
            }]
        );
    }

    #[test]
    fn old_push_backs_are_pruned_as_new_ones_arrive() {
        let (_tmp, shared) = shared();
        let now = now_ms();
        let old = now - KEPT - Duration::from_secs(1);
        shared.record(Pk::new(1), old, old, "429").unwrap();
        shared.record(Pk::new(2), now, now, "429").unwrap();
        let left: i64 = shared
            .conn
            .query_row("SELECT count(*) FROM pushbacks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn forgetting_an_account_leaves_the_others() {
        let (_tmp, shared) = shared();
        let now = now_ms();
        shared.record(Pk::new(1), now, now, "429").unwrap();
        shared.record(Pk::new(2), now, now, "429").unwrap();
        shared.forget(Pk::new(1)).unwrap();
        let left: Vec<Pk> = shared
            .pushbacks(now)
            .unwrap()
            .into_iter()
            .map(|p| p.pk)
            .collect();
        assert_eq!(left, vec![Pk::new(2)]);
    }

    #[test]
    fn the_interval_seed_is_written_once() {
        let (_tmp, shared) = shared();
        assert_eq!(shared.interval_seeded_at().unwrap(), None);
        assert_eq!(
            shared.seed_interval(Epoch::new(1_000)).unwrap(),
            Epoch::new(1_000)
        );
        assert_eq!(
            shared.seed_interval(Epoch::new(2_000)).unwrap(),
            Epoch::new(1_000),
            "a later start keeps the first seed"
        );
        assert_eq!(
            shared.interval_seeded_at().unwrap(),
            Some(Epoch::new(1_000))
        );
    }

    #[test]
    fn the_first_ask_of_a_timer_is_the_one_kept() {
        let (_tmp, shared) = shared();
        assert_eq!(shared.asked_since().unwrap(), None);
        assert_eq!(
            shared.ask_since(Epoch::new(1_000)).unwrap(),
            Epoch::new(1_000)
        );
        assert_eq!(
            shared.ask_since(Epoch::new(2_000)).unwrap(),
            Epoch::new(1_000),
            "a later look keeps the first"
        );
        assert_eq!(shared.asked_since().unwrap(), Some(Epoch::new(1_000)));
    }

    #[test]
    fn the_latest_start_of_the_monitor_is_the_one_kept() {
        let (_tmp, shared) = shared();
        assert_eq!(shared.monitor_last_started().unwrap(), None);
        shared.monitor_started(Epoch::new(2_000)).unwrap();
        shared.monitor_started(Epoch::new(1_000)).unwrap();
        assert_eq!(
            shared.monitor_last_started().unwrap(),
            Some(Epoch::new(2_000)),
            "a start never moves back"
        );
        shared.monitor_started(Epoch::new(3_000)).unwrap();
        assert_eq!(
            shared.monitor_last_started().unwrap(),
            Some(Epoch::new(3_000))
        );
    }

    #[test]
    fn a_run_is_running_while_it_beats_and_not_once_it_goes_quiet() {
        let (_tmp, shared) = shared();
        let id = shared
            .record_automation_run("abcd1234", Epoch::new(1_000))
            .unwrap();
        let running_at = |now: i64| {
            shared
                .automation_runs("abcd1234", 1, Epoch::new(now))
                .unwrap()[0]
                .running
        };
        assert!(running_at(1_000), "a run is going from its start");
        assert!(running_at(1_000 + RUN_QUIET_AFTER));
        assert!(
            !running_at(1_001 + RUN_QUIET_AFTER),
            "no sign of life for longer than that is a run killed"
        );
        shared.beat_automation_run(id, Epoch::new(1_060)).unwrap();
        assert!(running_at(1_060 + RUN_QUIET_AFTER), "a beat keeps it going");
        assert_eq!(
            shared.automation_last_started("abcd1234").unwrap(),
            Some(Epoch::new(1_000))
        );
    }

    #[test]
    fn a_finished_run_keeps_what_it_found_beside_its_sentences() {
        let (_tmp, shared) = shared();
        let id = shared
            .record_automation_run("abcd1234", Epoch::new(1_000))
            .unwrap();
        let found = r#"[{"kind":"cannot_run"}]"#;
        shared
            .finish_automation_run(
                id,
                Epoch::new(1_010),
                "error",
                3,
                "It cannot run.",
                Some(found),
            )
            .unwrap();
        let run = &shared
            .automation_runs("abcd1234", 1, Epoch::new(1_010))
            .unwrap()[0];
        assert_eq!(run.summary, "It cannot run.");
        assert_eq!(run.found.as_deref(), Some(found));
        assert_eq!(run.outcome.as_deref(), Some("error"));
        assert!(!run.running, "a run that ended is not going");
    }

    #[test]
    fn a_beat_after_the_end_changes_nothing() {
        let (_tmp, shared) = shared();
        let id = shared
            .record_automation_run("abcd1234", Epoch::new(1_000))
            .unwrap();
        shared
            .finish_automation_run(id, Epoch::new(1_010), "ok", 0, "", None)
            .unwrap();
        shared.beat_automation_run(id, Epoch::new(1_020)).unwrap();
        let alive_at: i64 = shared
            .conn
            .query_row(
                "SELECT alive_at FROM automation_runs WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alive_at, 1_000);
        let run = &shared
            .automation_runs("abcd1234", 1, Epoch::new(1_020))
            .unwrap()[0];
        assert!(!run.running && run.found.is_none());
    }

    /// STRICT, like every table in the accounts' databases.
    #[test]
    fn the_table_rejects_the_wrong_types() {
        let (_tmp, shared) = shared();
        let result = shared.conn.execute(
            "INSERT INTO pushbacks (pk, at_ms, until_ms, reason) VALUES ('x', 1, 1, '429')",
            [],
        );
        assert!(result.is_err());
    }
}
