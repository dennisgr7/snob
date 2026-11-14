//! The monitor's own tables: what has been reported, and what each run did.
//!
//! `snapshots` records that a list was walked; `watch_marks` that a walk was
//! reported. The diff reads the receipt, never guesses it from the captures.

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use super::{StoreError, pk_from_sql, pk_to_sql};
use snob_core::model::ListKind;
use snob_core::watch::{RecordedOutcome, Rename};
use snob_core::{Epoch, Pk};

/// The receipt for one account and list: what was reported, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    /// The capture that was reported against. `None` once it has been pruned,
    /// which means the baseline is gone and the next run has to lay a new one.
    pub snapshot_id: Option<i64>,
    /// When the report was made. The rename window is bounded by
    /// [`rename_cursor`] instead.
    pub compared_at: Epoch,
}

/// The receipt for this account and list, if there is one.
pub fn mark(conn: &Connection, account_pk: Pk, kind: ListKind) -> Result<Option<Mark>, StoreError> {
    let mark = conn
        .query_row(
            "SELECT snapshot_id, compared_at FROM watch_marks
             WHERE account_pk = ?1 AND kind = ?2",
            params![pk_to_sql(account_pk), kind.as_str()],
            |row| {
                Ok(Mark {
                    snapshot_id: row.get(0)?,
                    compared_at: Epoch::new(row.get(1)?),
                })
            },
        )
        .optional()?;
    Ok(mark)
}

/// How long a capture is kept once nothing needs it: thirty days to look back
/// over, since the diff only compares against the last reported capture. The
/// posts the monitor has seen are kept as long (`watch_posts::prune`).
pub(super) const KEEP_FOR_SECS: i64 = 30 * 24 * 3_600;

/// What one sweep of the retention rules settled. `captures` is housekeeping;
/// `given_up` is news lost for good, which somebody has to be told.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Swept {
    /// Captures deleted because nothing needs them any more, and posts the
    /// monitor no longer needs to remember.
    pub captures: usize,
    /// Reports abandoned by this sweep because they grew too old to be news.
    pub given_up: usize,
}

/// The monitor's sweep: the captures nothing needs any more
/// ([`super::snapshots::prune`], which says what is kept), the reports too
/// old to be news, the runs past their keeping and the posts no longer
/// remembered.
pub fn prune(conn: &Connection, now: Epoch) -> Result<Swept, StoreError> {
    let removed = super::snapshots::prune(conn, now)?;

    // The outbox's own rules, in its own module. The count of reports given
    // up comes back out: that is news lost, not housekeeping.
    let given_up = super::deliveries::expire_stale(conn, now)?;
    super::deliveries::forget_settled(conn, now)?;

    // The newest run of each account is kept whatever its age: `status` reads
    // it to decide whether the monitor works, and a run that ended in
    // `no_session` must not age into "it has not run yet".
    conn.execute(
        // Through the view `last_runs` reads, so both break a tie alike.
        "DELETE FROM watch_runs
         WHERE started_at < ?1
           AND id NOT IN (SELECT id FROM newest_run)",
        params![(now - Duration::from_secs(KEEP_RUNS_FOR_SECS as u64)).get()],
    )?;

    let posts = super::watch_posts::prune(conn, now)?;

    Ok(Swept {
        captures: removed + posts,
        given_up,
    })
}

/// How long the run log is kept: enough for `status` to describe a bad week.
const KEEP_RUNS_FOR_SECS: i64 = 30 * 24 * 3_600;

/// What one run of the monitor did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub account_pk: Pk,
    pub started_at: Epoch,
    pub finished_at: Option<Epoch>,
    pub requests: u32,
    /// What came of it, in the vocabulary of `snob_core::watch`, parsed once
    /// in the row mapping; a token this build does not know arrives as
    /// [`RecordedOutcome::Unknown`].
    pub outcome: Option<RecordedOutcome>,
    pub changes: u32,
    /// Why the run drew no conclusion about the account's posts, as a
    /// report's `run.posts.skipped` says it; `None` when it looked at them or
    /// does not watch them.
    pub posts_skipped: Option<String>,
    /// The moment the schedule said the run was for; `None` for one started
    /// by hand.
    pub due_at: Option<Epoch>,
    /// When the run began, before any of its work; `None` for a row written
    /// before it was recorded.
    pub began_at: Option<Epoch>,
}

/// Records a run, whatever came of it, the ones that reported nothing
/// included: the marks say when something was last reported, this says when
/// the monitor last ran.
pub fn record_run(conn: &Connection, run: &Run) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO watch_runs
             (account_pk, started_at, finished_at, requests, outcome, changes, posts_skipped,
              due_at, began_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            pk_to_sql(run.account_pk),
            run.started_at.get(),
            run.finished_at.map(Epoch::get),
            run.requests,
            run.outcome.as_ref().map(RecordedOutcome::as_str),
            run.changes,
            run.posts_skipped,
            run.due_at.map(Epoch::get),
            run.began_at.map(Epoch::get),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The newest run of every account that ever ran. The accounts come from this
/// table, not from the marks: one whose every run was refused has no mark.
pub fn last_runs(conn: &Connection) -> Result<Vec<Run>, StoreError> {
    let mut stmt = conn.prepare(
        // The view `prune` exempts from its sweep, so both break a tie alike.
        "SELECT account_pk, started_at, finished_at, requests, outcome, changes, posts_skipped,
                due_at, began_at
         FROM newest_run
         ORDER BY account_pk",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Run {
            account_pk: pk_from_sql(row.get(0)?),
            started_at: Epoch::new(row.get(1)?),
            finished_at: row.get::<_, Option<i64>>(2)?.map(Epoch::new),
            requests: row.get(3)?,
            outcome: row
                .get::<_, Option<String>>(4)?
                .as_deref()
                .map(RecordedOutcome::from_token),
            changes: row.get(5)?,
            posts_skipped: row.get(6)?,
            due_at: row.get::<_, Option<i64>>(7)?.map(Epoch::new),
            began_at: row.get::<_, Option<i64>>(8)?.map(Epoch::new),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// When the monitor last started a run, for any account: one loop covers
/// every watched account. The scheduled loop seeds its `--every` clock from
/// it, so that a restart does not start the interval again.
pub fn last_started(conn: &Connection) -> Result<Option<Epoch>, StoreError> {
    let started = conn
        .query_row(LAST_STARTED, [], |row| row.get::<_, i64>(0))
        .optional()?;
    Ok(started.map(Epoch::new))
}

/// Read off the end of `watch_runs_started` (`021_lookup_indexes.sql`).
const LAST_STARTED: &str =
    "SELECT started_at FROM watch_runs ORDER BY started_at DESC, id DESC LIMIT 1";

/// One receipt, with what it is a receipt for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountMark {
    pub account_pk: Pk,
    pub kind: ListKind,
    pub snapshot_id: Option<i64>,
    pub compared_at: Epoch,
}

/// Every receipt there is, newest first.
pub fn all_marks(conn: &Connection) -> Result<Vec<AccountMark>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT account_pk, kind, snapshot_id, compared_at
         FROM watch_marks ORDER BY compared_at DESC, account_pk, kind",
    )?;
    let rows = stmt.query_map([], |row| {
        let kind: String = row.get(1)?;
        Ok((
            row.get::<_, i64>(0)?,
            kind,
            row.get(2)?,
            Epoch::new(row.get(3)?),
        ))
    })?;

    let mut marks = Vec::new();
    for row in rows {
        let (pk, kind, snapshot_id, compared_at) = row?;
        // Skipped rather than guessed at: a default would answer about a list
        // nobody asked about.
        let Ok(kind) = kind.parse::<ListKind>() else {
            tracing::warn!(kind, "a watch mark names a list this version does not know");
            continue;
        };
        marks.push(AccountMark {
            account_pk: pk_from_sql(pk),
            kind,
            snapshot_id,
            compared_at,
        });
    }
    Ok(marks)
}

/// The newest id in the rename history; zero when it is empty. Ids rather
/// than whole-second timestamps make "already reported" decidable.
pub fn history_head(conn: &Connection) -> Result<i64, StoreError> {
    let head = conn.query_row(
        "SELECT coalesce(MAX(id), 0) FROM username_history",
        [],
        |row| row.get(0),
    )?;
    Ok(head)
}

/// Records that everything up to this capture has been reported. Runs inside
/// the caller's transaction ([`commit_report`]); `at` is one moment for every
/// list of a report.
///
/// **Unconditional on purpose.** Two overlapping runs can put the mark back on
/// the older capture, which costs a duplicate report (receivers deduplicate
/// on `run_id`) and never loses a window. A `WHERE` on `compared_at` orders
/// nothing (`at` is stamped before the comparison), and one on `snapshot_id`
/// would refuse for ever a mark whose capture was pruned to NULL.
fn set_mark(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
    snapshot_id: i64,
    at: Epoch,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO watch_marks (account_pk, kind, snapshot_id, compared_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_pk, kind) DO UPDATE SET
             snapshot_id    = excluded.snapshot_id,
             compared_at    = excluded.compared_at",
        params![pk_to_sql(account_pk), kind.as_str(), snapshot_id, at.get()],
    )?;
    Ok(())
}

/// How far along the rename history this account has been reported; zero
/// when it never has. One number per account, not one per list.
pub fn rename_cursor(conn: &Connection, account_pk: Pk) -> Result<i64, StoreError> {
    let cursor = conn
        .query_row(
            "SELECT cursor FROM watch_renames WHERE account_pk = ?1",
            params![pk_to_sql(account_pk)],
            |row| row.get(0),
        )
        .optional()?;
    Ok(cursor.unwrap_or(0))
}

/// Records that the rename history has been reported up to `cursor`. Only
/// when a window was actually read: moving it otherwise steps over a rename
/// another walk filed meanwhile.
fn set_rename_cursor(
    conn: &Connection,
    account_pk: Pk,
    cursor: i64,
    at: Epoch,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO watch_renames (account_pk, cursor, marked_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(account_pk) DO UPDATE SET
             cursor    = excluded.cursor,
             marked_at = excluded.marked_at",
        params![pk_to_sql(account_pk), cursor, at.get()],
    )?;
    Ok(())
}

const INTERVAL_SEED: &str = "interval_seeded_at";

/// When an interval schedule started counting, as an account's database kept
/// it before `shared.db` did. Read only to copy it there once.
pub fn interval_seeded_at(conn: &Connection) -> Result<Option<Epoch>, StoreError> {
    let at = conn
        .query_row(
            "SELECT value FROM watch_state WHERE key = ?1",
            params![INTERVAL_SEED],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(at.map(Epoch::new))
}

/// Which renames in the open window have already gone out for this account.
/// The cursor says how far was scanned; a run with one list refused announces
/// what it sees without moving it, and must not announce it again
/// (`007_renames_sent.sql`).
pub fn renames_already_sent(
    conn: &Connection,
    account_pk: Pk,
) -> Result<std::collections::HashSet<i64>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT history_id FROM watch_renames_sent WHERE account_pk = ?1")?;
    let rows = stmt.query_map(params![pk_to_sql(account_pk)], |row| row.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Files the renames this report is announcing, so no later one repeats them.
fn record_renames_sent(
    conn: &Connection,
    account_pk: Pk,
    history_ids: &[i64],
    at: Epoch,
) -> Result<(), StoreError> {
    if history_ids.is_empty() {
        return Ok(());
    }
    let mut insert = conn.prepare(
        "INSERT INTO watch_renames_sent (account_pk, history_id, sent_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(account_pk, history_id) DO NOTHING",
    )?;
    for id in history_ids {
        insert.execute(params![pk_to_sql(account_pk), id, at.get()])?;
    }
    Ok(())
}

/// A report ready to be made durable: the bytes, since the signature covers
/// bytes and a retry sends the same string.
pub struct Queued<'a> {
    pub run_id: &'a str,
    pub body: &'a str,
    /// Where it is addressed, which [`super::deliveries::due`] filters on. A
    /// report with nowhere to go is never queued.
    pub destination: &'a str,
    /// The event and the type it goes as; `None` is snob's JSON, its event
    /// read from the body.
    pub form: Option<&'a super::deliveries::Form>,
}

/// Commits a report in **one transaction**: queues it, then moves the marks
/// it makes stale. The report has to be durable with the mark that retires
/// it, or a process killed in between loses a window for good; delivery is
/// at-least-once, deduplicated on `run_id`.
///
/// `marks` names only the lists the report spoke about, `posts` what it
/// concluded about the account's posts (written nowhere else), and
/// `rename_cursor` is `None` when no rename window was read.
#[expect(
    clippy::too_many_arguments,
    reason = "everything one report commits, in the one transaction that has to hold it"
)]
pub fn commit_report(
    store: &mut super::Store,
    account_pk: Pk,
    marks: &[(ListKind, i64)],
    at: Epoch,
    rename_cursor: Option<i64>,
    renames_sent: &[i64],
    posts: Option<&super::watch_posts::Commit>,
    delivery: Option<Queued<'_>>,
) -> Result<Option<i64>, StoreError> {
    let tx = store.conn_mut().transaction()?;

    let queued = match delivery {
        Some(queued) => Some(super::deliveries::enqueue_as(
            &tx,
            queued.run_id,
            account_pk,
            queued.body,
            at,
            queued.destination,
            queued.form,
        )?),
        None => None,
    };

    for &(kind, snapshot_id) in marks {
        set_mark(&tx, account_pk, kind, snapshot_id, at)?;
    }

    if let Some(posts) = posts {
        super::watch_posts::write(&tx, account_pk, posts, at)?;
    }

    if let Some(cursor) = rename_cursor {
        set_rename_cursor(&tx, account_pk, cursor, at)?;
    }

    // Rows below the cursor are never read again, so they go.
    record_renames_sent(&tx, account_pk, renames_sent, at)?;
    if let Some(cursor) = rename_cursor {
        tx.execute(
            "DELETE FROM watch_renames_sent WHERE account_pk = ?1 AND history_id <= ?2",
            params![pk_to_sql(account_pk), cursor],
        )?;
    }

    tx.commit()?;
    Ok(queued)
}

/// Renames filed after history id `since` and up to `head`, among the members
/// of one capture, each account once: `from` is the earliest name in the
/// window, `to` the name it has now. Ids, not timestamps, bound the window
/// (`changed_at` is in whole seconds), and `head` closes it so that a rename
/// another process files meanwhile is not announced under two `run_id`s.
pub fn renames_since(
    conn: &Connection,
    snapshot_id: i64,
    since: i64,
    head: i64,
) -> Result<Vec<Rename>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT h.pk, h.id, h.username, u.username, h.changed_at
         FROM username_history h
         JOIN users u            ON u.pk = h.pk
         JOIN snapshot_members m ON m.user_pk = h.pk AND m.snapshot_id = ?1
         WHERE h.id > ?2 AND h.id <= ?3
           AND h.id = (
             SELECT MIN(e.id) FROM username_history e
             WHERE e.pk = h.pk AND e.id > ?2 AND e.id <= ?3
           )
         ORDER BY h.id",
    )?;

    moved(stmt.query_map(params![snapshot_id, since, head], row_to_rename)?)
}

/// One row of either rename query: the pk, the history id, the old name, the
/// new one and when.
fn row_to_rename(row: &rusqlite::Row<'_>) -> rusqlite::Result<Rename> {
    Ok(Rename {
        pk: pk_from_sql(row.get(0)?),
        history_id: row.get(1)?,
        from: row.get(2)?,
        to: row.get(3)?,
        at: Epoch::new(row.get(4)?),
    })
}

/// The renames among `rows`: a name changed and changed back is none.
fn moved(rows: impl Iterator<Item = rusqlite::Result<Rename>>) -> Result<Vec<Rename>, StoreError> {
    let mut renames = Vec::new();
    for rename in rows {
        let rename = rename?;
        if rename.from != rename.to {
            renames.push(rename);
        }
    }
    Ok(renames)
}

/// The renames among the members of `snapshots` noticed after `after` and up
/// to `until`, each once, for `snob changes`: no cursor, nothing recorded.
///
/// `username_history` files the name an account **had**, so the new name is
/// the first row filed after the window, or the current one.
pub fn renames_between(
    conn: &Connection,
    snapshots: &[i64],
    after: Epoch,
    until: Epoch,
) -> Result<Vec<Rename>, StoreError> {
    let mut stmt = conn.prepare(RENAMES_BETWEEN)?;
    let ids = serde_json::to_string(snapshots).unwrap_or_else(|_| "[]".into());
    moved(stmt.query_map(params![ids, after.get(), until.get()], row_to_rename)?)
}

/// `?1` the captures, as a JSON array of ids, `?2` and `?3` the window. The
/// window is sought in `username_history_changed` (`021_lookup_indexes.sql`).
const RENAMES_BETWEEN: &str = "SELECT h.pk, h.id, h.username,
                coalesce(
                  (SELECT n.username FROM username_history n
                   WHERE n.pk = h.pk AND n.changed_at > ?3
                   ORDER BY n.changed_at, n.id LIMIT 1),
                  u.username),
                h.changed_at
         FROM username_history h
         JOIN users u ON u.pk = h.pk
         WHERE h.changed_at > ?2 AND h.changed_at <= ?3
           AND h.id = (
             SELECT MIN(e.id) FROM username_history e
             WHERE e.pk = h.pk AND e.changed_at > ?2 AND e.changed_at <= ?3
           )
           AND EXISTS (
             SELECT 1 FROM snapshot_members m, json_each(?1) j
             WHERE m.user_pk = h.pk AND m.snapshot_id = j.value
           )
         ORDER BY h.changed_at, h.id";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, accounts, snapshots, users};
    use snob_core::model::User;
    use snob_core::watch::RunOutcome;

    /// The newest run of one account, read out of the answer the production
    /// code reads.
    fn one_of(runs: &[Run], account_pk: Pk) -> &Run {
        runs.iter()
            .find(|run| run.account_pk == account_pk)
            .unwrap_or_else(|| panic!("no run recorded for {account_pk}"))
    }

    fn user(pk: u64, name: &str) -> User {
        User {
            pk: Pk::new(pk),
            username: name.into(),
            full_name: None,
            is_private: None,
            is_verified: None,
            pfp_url: None,
        }
    }

    /// The top of the rename window, for a test that only cares about the
    /// bottom of it. A run reads this before comparing, so "everything filed so
    /// far" is what these are asking for.
    fn head_now(conn: &Connection) -> i64 {
        history_head(conn).unwrap()
    }

    /// An account with one capture holding `members`, ready to be marked.
    fn account_with_capture(db: &mut Store, pk: Pk, members: &[User]) -> i64 {
        users::ensure(db.conn(), pk).unwrap();
        accounts::upsert(db.conn(), pk, true).unwrap();
        let opened = snapshots::begin(db.conn(), pk, ListKind::Followers, None).unwrap();
        snapshots::save_page(db, opened.id, members, None).unwrap();
        snapshots::close(
            db.conn(),
            opened.id,
            snob_core::model::StopReason::Completed,
        )
        .unwrap();
        opened.id
    }

    #[test]
    fn an_account_that_was_never_watched_has_no_mark() {
        let db = Store::in_memory().unwrap();
        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers).unwrap(),
            None
        );
    }

    #[test]
    fn a_mark_is_written_and_read_back() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            id,
            Epoch::new(1_700),
        )
        .unwrap();
        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers).unwrap(),
            Some(Mark {
                snapshot_id: Some(id),
                compared_at: Epoch::new(1_700),
            })
        );
    }

    /// The two lists are marked separately. Sharing a row would make walking
    /// the followers list look like the following list had been reported too,
    /// and the changes in it would never be announced.
    #[test]
    fn the_two_lists_are_marked_apart() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            id,
            Epoch::new(1_700),
        )
        .unwrap();
        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Following).unwrap(),
            None
        );
    }

    #[test]
    fn marking_again_moves_the_mark_rather_than_failing() {
        let mut db = Store::in_memory().unwrap();
        let first = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        let second = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            first,
            Epoch::new(1_700),
        )
        .unwrap();
        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            second,
            Epoch::new(1_800),
        )
        .unwrap();
        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers).unwrap(),
            Some(Mark {
                snapshot_id: Some(second),
                compared_at: Epoch::new(1_800),
            })
        );
    }

    /// The run retention keeps is the run `status` shows.
    ///
    /// "The newest run of each account" was spelled twice, forty lines apart:
    /// `prune` exempts it from the thirty-day sweep so `status` never says "it
    /// has not run yet" about a monitor that has been failing for weeks, and
    /// `last_runs` reads it to say what happened.
    ///
    /// `started_at` is whole seconds, so two runs of one account inside the same
    /// second are a tie -- reachable through `snob watch once` beside a
    /// scheduled tick, which consults no schedule and no gap. The `id DESC`
    /// tie-break is what breaks it, and added to one site and not the other,
    /// prune keeps one of the two rows while `last_runs` reads the other: the
    /// run a person is shown is deleted on the next settle and the survivor is
    /// one nothing displays. Nothing fails and the suite stays green.
    ///
    /// Both rows are older than the window, so the only thing keeping either of
    /// them is the exemption -- which is what makes this a test of the exemption
    /// and not of the sweep. And it is asked by id rather than by count: prune
    /// keeping *a* row while the reader reads *the other* leaves one row either
    /// way, which is exactly why the disagreement was invisible.
    #[test]
    fn the_run_retention_keeps_is_the_run_status_shows() {
        let db = Store::in_memory().unwrap();
        users::upsert(db.conn(), &user(7, "me")).unwrap();
        accounts::upsert(db.conn(), Pk::new(7), true).unwrap();
        let now = Epoch::new(2_000_000_000);
        let long_ago = now - Duration::from_secs(KEEP_RUNS_FOR_SECS as u64 + 1);

        // Two runs of one account in the same second, which `started_at` cannot
        // tell apart.
        for _ in 0..2 {
            record_run(
                db.conn(),
                &Run {
                    account_pk: Pk::new(7),
                    started_at: long_ago,
                    finished_at: Some(long_ago),
                    requests: 1,
                    outcome: Some(RunOutcome::Ok.into()),
                    changes: 0,
                    posts_skipped: None,
                    due_at: None,
                    began_at: None,
                },
            )
            .unwrap();
        }

        // The row `status` would show, before anything is swept.
        let shown: i64 = db
            .conn()
            .query_row("SELECT id FROM newest_run", [], |row| row.get(0))
            .unwrap();

        prune(db.conn(), now).unwrap();

        let kept: Vec<i64> = db
            .conn()
            .prepare("SELECT id FROM watch_runs ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            kept,
            vec![shown],
            "retention keeps exactly the row the reader reads, and nothing else"
        );
        assert_eq!(
            last_runs(db.conn()).unwrap().len(),
            1,
            "and the reader still finds it"
        );
    }

    /// Why a run skipped the posts is kept with the run and read back with
    /// the newest one, through the view, which takes a new column as it comes.
    #[test]
    fn why_a_run_skipped_the_posts_is_read_back_with_it() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        record_run(
            db.conn(),
            &Run {
                account_pk: Pk::new(7),
                started_at: Epoch::new(1_000),
                finished_at: Some(Epoch::new(1_000)),
                requests: 1,
                outcome: None,
                changes: 0,
                posts_skipped: Some("no_browser".into()),
                due_at: None,
                began_at: None,
            },
        )
        .unwrap();
        assert_eq!(
            last_runs(db.conn()).unwrap()[0].posts_skipped.as_deref(),
            Some("no_browser")
        );
    }

    /// When a run was due and when it began are kept with it.
    #[test]
    fn when_a_run_was_due_and_began_is_read_back_with_it() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        record_run(
            db.conn(),
            &Run {
                account_pk: Pk::new(7),
                started_at: Epoch::new(5_000),
                finished_at: Some(Epoch::new(5_100)),
                requests: 1,
                outcome: None,
                changes: 0,
                posts_skipped: None,
                due_at: Some(Epoch::new(1_000)),
                began_at: Some(Epoch::new(4_900)),
            },
        )
        .unwrap();
        let run = &last_runs(db.conn()).unwrap()[0];
        assert_eq!(run.due_at, Some(Epoch::new(1_000)));
        assert_eq!(run.began_at, Some(Epoch::new(4_900)));
    }

    /// The two retention windows this file owns, written out.
    ///
    /// Every fixture below is expressed as a multiple of the constant it
    /// exercises -- `KEEP_FOR_SECS * 5`, `KEEP_RUNS_FOR_SECS + 1` -- so all of
    /// them agree with any value at all: set the capture window to an hour and
    /// the whole suite still passes while a month of history disappears
    /// overnight, or set it to a decade and the database grows without end. A
    /// relative fixture tests the arithmetic. This tests the decision.
    #[test]
    fn how_long_a_capture_and_a_run_are_kept_are_the_documented_ones() {
        assert_eq!(KEEP_FOR_SECS, 30 * 24 * 3_600, "thirty days of captures");
        assert_eq!(KEEP_RUNS_FOR_SECS, 30 * 24 * 3_600, "thirty days of runs");
    }

    /// A report this store queues is always addressed, and the type is what
    /// says so.
    ///
    /// `Queued::destination` says why it is not an `Option`.
    #[test]
    fn a_queued_report_carries_the_address_it_was_made_for() {
        const HERE: &str = "https://n8n.local/webhook/snob";
        const THE_TEST_RUN: &str = "https://n8n.local/webhook-test/snob";

        let mut db = Store::in_memory().unwrap();
        let capture = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        commit_report(
            &mut db,
            Pk::new(7),
            &[(ListKind::Followers, capture)],
            Epoch::new(1_700),
            None,
            &[],
            None,
            Some(Queued {
                run_id: "run-1",
                body: "{}",
                destination: HERE,
                form: None,
            }),
        )
        .unwrap();

        assert!(
            crate::store::deliveries::due(db.conn(), Epoch::new(1_700), 10, THE_TEST_RUN)
                .unwrap()
                .is_empty(),
            "a report committed for {HERE} was drained to {THE_TEST_RUN}"
        );
        assert_eq!(
            crate::store::deliveries::due(db.conn(), Epoch::new(1_700), 10, HERE)
                .unwrap()
                .len(),
            1,
            "and it is still owed where it was addressed"
        );
    }

    /// The posts a report names are written with it or not at all: a report
    /// that cannot be queued (here, a run id already used) leaves no post
    /// recorded as reported, so the next run names it again.
    #[test]
    fn a_report_and_the_posts_it_names_land_together_or_not_at_all() {
        use crate::store::watch_posts;

        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        let queued = |run_id| Queued {
            run_id,
            body: "{}",
            destination: "https://n8n.local/webhook/snob",
            form: None,
        };
        commit_report(
            &mut db,
            Pk::new(7),
            &[],
            Epoch::new(1_700),
            None,
            &[],
            None,
            Some(queued("run-1")),
        )
        .unwrap();

        let posts = watch_posts::Commit {
            seen: vec![watch_posts::Seen {
                media_pk: 9,
                code: "C9".to_string(),
                taken_at: Epoch::new(1_600),
                pinned: false,
            }],
            news: vec![9],
            floor: Epoch::new(1_600),
            ..watch_posts::Commit::default()
        };
        let refused = commit_report(
            &mut db,
            Pk::new(7),
            &[],
            Epoch::new(1_800),
            None,
            &[],
            Some(&posts),
            Some(queued("run-1")),
        );
        assert!(refused.is_err(), "the run id was used already");
        assert_eq!(watch_posts::mark(db.conn(), Pk::new(7)).unwrap(), None);
        assert!(
            watch_posts::newest(db.conn(), Pk::new(7), 12)
                .unwrap()
                .is_empty()
        );

        commit_report(
            &mut db,
            Pk::new(7),
            &[],
            Epoch::new(1_800),
            None,
            &[],
            Some(&posts),
            Some(queued("run-2")),
        )
        .unwrap();
        assert_eq!(
            watch_posts::mark(db.conn(), Pk::new(7))
                .unwrap()
                .unwrap()
                .reported_at,
            Some(Epoch::new(1_800))
        );
        assert_eq!(
            watch_posts::newest(db.conn(), Pk::new(7), 12).unwrap(),
            vec![9]
        );
    }

    /// The quiet run: the same capture, a later moment.
    ///
    /// This is `Basis::Unchanged`, whose `mark_to()` is the id already in the
    /// row, and it is the common case -- two unmoved counters, one request. It
    /// is a tripwire for the guard somebody will reach for: an upsert
    /// conditioned on `excluded.snapshot_id > watch_marks.snapshot_id` declines
    /// this write, and `compared_at` -- what `snob watch status` prints as "last
    /// reported on" -- stops moving on a monitor that is working perfectly.
    #[test]
    fn a_quiet_run_moves_the_moment_without_moving_the_capture() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            id,
            Epoch::new(1_700),
        )
        .unwrap();
        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            id,
            Epoch::new(1_800),
        )
        .unwrap();

        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers).unwrap(),
            Some(Mark {
                snapshot_id: Some(id),
                compared_at: Epoch::new(1_800),
            }),
            "a run that found nothing still reported, and status says when"
        );
    }

    /// And a receipt whose capture retention took can be marked again.
    ///
    /// The other tripwire, and the sharper one. `snapshot_id` is nullable by
    /// `ON DELETE SET NULL` -- the neighboring test pins that state -- so an
    /// ordering guard written as `excluded.snapshot_id >= watch_marks.snapshot_id`
    /// compares against NULL, which is NULL, which `DO UPDATE ... WHERE` reads as
    /// false. The write is declined with no error, the account never gets a new
    /// baseline, and every run from then on is a `Basis::Baseline` that reports
    /// nothing. A monitor gone silent for good, closed by the guard meant to
    /// protect it.
    #[test]
    fn a_receipt_whose_capture_was_pruned_can_be_marked_again() {
        let mut db = Store::in_memory().unwrap();
        let first = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            first,
            Epoch::new(1_700),
        )
        .unwrap();

        db.conn()
            .execute("DELETE FROM snapshots WHERE id = ?1", params![first])
            .unwrap();
        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers)
                .unwrap()
                .unwrap()
                .snapshot_id,
            None,
            "the fixture depends on the receipt outliving its capture"
        );

        let second = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            second,
            Epoch::new(1_800),
        )
        .unwrap();

        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers).unwrap(),
            Some(Mark {
                snapshot_id: Some(second),
                compared_at: Epoch::new(1_800),
            }),
            "the next run lays a new baseline, and this is where it is recorded"
        );
    }

    /// What happens when retention takes the capture a mark points at.
    ///
    /// The row has to survive with its `compared_at` intact: the baseline is
    /// gone, so the next run lays down a new one and reports nothing — but the
    /// left edge of the rename window is still known, which is why this is
    /// `ON DELETE SET NULL` and not `CASCADE`. With CASCADE the whole receipt
    /// would go and the next run would have no idea which renames it had
    /// already announced.
    #[test]
    fn pruning_the_marked_capture_leaves_the_receipt_behind() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            id,
            Epoch::new(1_700),
        )
        .unwrap();

        db.conn()
            .execute("DELETE FROM snapshots WHERE id = ?1", params![id])
            .unwrap();

        assert_eq!(
            mark(db.conn(), Pk::new(7), ListKind::Followers).unwrap(),
            Some(Mark {
                snapshot_id: None,
                compared_at: Epoch::new(1_700),
            }),
            "the baseline is gone but what has already been reported is not"
        );
    }

    #[test]
    fn a_rename_inside_the_window_is_reported_with_both_names() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "before")]);

        // The walk that sees the new name is what files the history row.
        users::upsert(db.conn(), &user(1, "after")).unwrap();

        let found = renames_since(db.conn(), id, 0, head_now(db.conn())).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pk, Pk::new(1));
        assert_eq!(found[0].from, "before");
        assert_eq!(found[0].to, "after");
    }

    #[test]
    fn renames_between_two_moments_name_the_first_and_the_last_name() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "first")]);
        users::upsert(db.conn(), &user(1, "second")).unwrap();
        users::upsert(db.conn(), &user(1, "third")).unwrap();
        let now = snob_core::clock::now();

        let found = renames_between(db.conn(), &[id], Epoch::new(0), now).unwrap();
        assert_eq!(found.len(), 1, "one account, one rename: {found:?}");
        assert_eq!(
            (found[0].from.as_str(), found[0].to.as_str()),
            ("first", "third")
        );

        // A window that ended before the renames holds none, and nor does a
        // capture the account is not in.
        assert!(
            renames_between(db.conn(), &[id], Epoch::new(0), Epoch::new(1))
                .unwrap()
                .is_empty()
        );
        assert!(
            renames_between(db.conn(), &[id + 99], Epoch::new(0), now)
                .unwrap()
                .is_empty()
        );
    }

    /// The bound that stops one event being announced by two runs in a row.
    ///
    /// The whole point of bounding by id: this holds even though both the
    /// rename and both reports happen inside the same second, which is exactly
    /// the case a `changed_at` comparison cannot decide.
    #[test]
    fn a_rename_already_reported_is_not_reported_again() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "before")]);
        users::upsert(db.conn(), &user(1, "after")).unwrap();

        assert_eq!(
            renames_since(db.conn(), id, 0, head_now(db.conn()))
                .unwrap()
                .len(),
            1
        );

        let head = history_head(db.conn()).unwrap();
        assert!(
            renames_since(db.conn(), id, head, head_now(db.conn()))
                .unwrap()
                .is_empty(),
            "the run that reported it stopped at that id, so the next window starts after it"
        );
    }

    /// The window has a top as well as a bottom, and a rename filed above it is
    /// left for the next run.
    ///
    /// `renames_since` says why the top matters.
    #[test]
    fn a_rename_filed_after_the_head_was_read_waits_for_the_next_window() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "before"), user(2, "other")]);

        // What this run will report on, and the head it read before comparing.
        users::upsert(db.conn(), &user(1, "after")).unwrap();
        let head = head_now(db.conn());

        // And what another process files while the comparison is running.
        users::upsert(db.conn(), &user(2, "other_after")).unwrap();

        let found = renames_since(db.conn(), id, 0, head).unwrap();
        assert_eq!(found.len(), 1, "only what was inside the window: {found:?}");
        assert_eq!(found[0].pk, Pk::new(1));

        // And the next run, starting where this one stopped, picks up the one
        // that arrived late rather than losing it.
        let next = renames_since(db.conn(), id, head, head_now(db.conn())).unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].pk, Pk::new(2));
    }

    /// The newest run of each account outlives the retention window.
    ///
    /// The delete was unqualified where every other rule in `prune` protects the
    /// newest row of its kind, and its comment said nothing else read the table
    /// back — false about this very module, since `watch::status::health` reads
    /// `last_runs` and decides from it whether the monitor is working. So a
    /// monitor whose session expired went, after thirty days, from "the last run
    /// ended in no_session" and exit 1 to "it has not run yet" and exit 0, and a
    /// probe polling that code watched it turn green with nothing fixed.
    ///
    /// Reachable without anything else running: `settle` calls `prune` on the
    /// no-session branch of `once`, which records no run of its own.
    #[test]
    fn the_last_run_of_an_account_is_never_pruned() {
        let mut db = Store::in_memory().unwrap();
        let _ = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        let long_ago = Epoch::new(1_000);
        for started_at in [long_ago, long_ago + Duration::from_secs(1)] {
            record_run(
                db.conn(),
                &Run {
                    account_pk: Pk::new(7),
                    started_at,
                    finished_at: Some(started_at),
                    requests: 0,
                    outcome: Some(RunOutcome::NoSession.into()),
                    changes: 0,
                    posts_skipped: None,
                    due_at: None,
                    began_at: None,
                },
            )
            .unwrap();
        }

        prune(
            db.conn(),
            long_ago + Duration::from_secs(KEEP_RUNS_FOR_SECS as u64 + 10),
        )
        .unwrap();

        let runs = last_runs(db.conn()).unwrap();
        assert_eq!(runs.len(), 1, "the newest one stays: {runs:?}");
        assert_eq!(runs[0].started_at, long_ago + Duration::from_secs(1));
        assert_eq!(
            runs[0].outcome,
            Some(RunOutcome::NoSession.into()),
            "and it still says what stopped it, which is the whole point"
        );
    }

    /// Zero is what a mark that has never reported carries, and an empty
    /// history has to agree with it — otherwise the first window is off by one
    /// row in whichever direction the disagreement goes.
    #[test]
    fn an_empty_history_starts_where_an_unreported_mark_does() {
        let db = Store::in_memory().unwrap();
        assert_eq!(history_head(db.conn()).unwrap(), 0);
    }

    /// Back-dates a capture, so retention can be tested without waiting a
    /// month. Raw SQL for the same reason `tests/cache.rs` uses it: there is no
    /// legitimate way to write a `taken_at` in the past.
    fn age(db: &Store, id: i64, seconds: i64) {
        db.conn()
            .execute(
                "UPDATE snapshots SET taken_at = taken_at - ?2, started_at = started_at - ?2
                 WHERE id = ?1",
                params![id, seconds],
            )
            .unwrap();
    }

    #[test]
    fn an_old_capture_nothing_needs_is_removed_with_its_members() {
        let mut db = Store::in_memory().unwrap();
        let old = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        // The one `snob changes` compares the newest with, which is kept.
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        let newer = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        age(&db, old, KEEP_FOR_SECS + 1);

        assert_eq!(
            prune(db.conn(), snob_core::clock::now()).unwrap().captures,
            1
        );
        assert!(snapshots::find_usable(db.conn(), old).unwrap().is_none());
        assert!(snapshots::find_usable(db.conn(), newer).unwrap().is_some());

        let members: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM snapshot_members WHERE snapshot_id = ?1",
                params![old],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(members, 0, "the members go with the capture");
    }

    #[test]
    fn the_capture_before_the_newest_is_kept_however_old_it_is() {
        let mut db = Store::in_memory().unwrap();
        let before = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        let newest = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        age(&db, before, KEEP_FOR_SECS * 5);

        assert_eq!(
            snapshots::prune(db.conn(), snob_core::clock::now()).unwrap(),
            0
        );
        let whole: Vec<i64> = snapshots::usable(db.conn(), Pk::new(7), ListKind::Followers)
            .unwrap()
            .iter()
            .map(|capture| capture.id)
            .collect();
        assert_eq!(whole, [newest, before]);
    }

    /// The sweep a walk runs is of captures alone: a report still owed is the
    /// monitor's to give up on, since only the monitor's run can say so.
    #[test]
    fn sweeping_captures_expires_no_report() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        let long_ago = Epoch::new(1_000);
        let id = super::super::deliveries::enqueue(
            db.conn(),
            "run-1",
            Pk::new(7),
            "{}",
            long_ago,
            "https://example.test/hook",
        )
        .unwrap();

        snapshots::prune(db.conn(), snob_core::clock::now()).unwrap();
        assert_eq!(
            super::super::deliveries::state(db.conn(), id)
                .unwrap()
                .as_deref(),
            Some("pending")
        );
    }

    #[test]
    fn the_monitors_last_start_and_a_window_of_renames_are_sought() {
        let db = Store::in_memory().unwrap();
        let last = crate::store::plan(db.conn(), LAST_STARTED);
        assert!(last.contains("watch_runs_started"), "{last}");
        assert!(!last.contains("TEMP B-TREE"), "it is sorted: {last}");

        let renames = crate::store::plan(db.conn(), RENAMES_BETWEEN);
        assert!(renames.contains("username_history_changed"), "{renames}");
        assert!(!renames.contains("SCAN h"), "{renames}");
    }

    /// The rule the whole of retention has to respect. Taking the marked
    /// capture leaves the next run with no baseline, so it reports nothing —
    /// one silently missed report, and no error anywhere.
    /// `snob changes`, asked nothing else, compares a list's newest whole
    /// capture with the one before it. A list walked less than once a month
    /// would otherwise lose that one to the sweep its own next walk runs.
    #[test]
    fn the_capture_a_mark_points_at_is_kept_however_old_it_is() {
        let mut db = Store::in_memory().unwrap();
        let marked = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        // A newer one, so the marked capture is not kept merely for being last.
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        set_mark(
            db.conn(),
            Pk::new(7),
            ListKind::Followers,
            marked,
            Epoch::new(1_000),
        )
        .unwrap();
        age(&db, marked, KEEP_FOR_SECS * 10);

        prune(db.conn(), snob_core::clock::now()).unwrap();
        assert!(
            snapshots::find_usable(db.conn(), marked).unwrap().is_some(),
            "the baseline of the next diff was pruned"
        );
    }

    /// However old everything is, the newest of each list stays: it is what the
    /// cache serves and what a crossing reads.
    #[test]
    fn the_newest_capture_of_each_list_is_always_kept() {
        let mut db = Store::in_memory().unwrap();
        let followers = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        users::ensure(db.conn(), Pk::new(7)).unwrap();
        let opened = snapshots::begin(db.conn(), Pk::new(7), ListKind::Following, None).unwrap();
        snapshots::save_page(&mut db, opened.id, &[user(2, "two")], None).unwrap();
        snapshots::close(
            db.conn(),
            opened.id,
            snob_core::model::StopReason::Completed,
        )
        .unwrap();

        age(&db, followers, KEEP_FOR_SECS * 5);
        age(&db, opened.id, KEEP_FOR_SECS * 5);

        assert_eq!(
            prune(db.conn(), snob_core::clock::now()).unwrap().captures,
            0
        );
        assert!(
            snapshots::find_usable(db.conn(), followers)
                .unwrap()
                .is_some()
        );
        assert!(
            snapshots::find_usable(db.conn(), opened.id)
                .unwrap()
                .is_some()
        );
    }

    /// An interrupted walk is resumed from its rows. Taking them would end a
    /// resume somebody is in the middle of, and the resume window is what
    /// decides when a partial stops being useful — not this.
    #[test]
    fn an_unfinished_walk_is_left_alone() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        let partial = snapshots::begin(db.conn(), Pk::new(7), ListKind::Followers, None)
            .unwrap()
            .id;
        snapshots::save_page(&mut db, partial, &[user(3, "three")], Some("cursor")).unwrap();
        age(&db, partial, KEEP_FOR_SECS * 5);

        prune(db.conn(), snob_core::clock::now()).unwrap();
        let still_there: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM snapshots WHERE id = ?1",
                params![partial],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(still_there, 1);
    }

    /// A report still owed is work, not history. Deleting one would lose a
    /// change that was still going to be delivered.
    #[test]
    fn a_report_still_waiting_is_never_pruned() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        let now = snob_core::clock::now();
        // A year, written out rather than taken from the constant it is meant
        // to exercise. `deliveries` owns that number now, and
        // `how_long_a_settled_report_is_kept_is_the_documented_one` is what pins
        // it; a fixture expressed as a multiple of it agrees with anything.
        let long_ago = now - Duration::from_secs(365 * 24 * 3_600);
        // Owed, and young enough to still be news — the other half of the rule
        // is the test below.
        let owed = crate::store::deliveries::enqueue(
            db.conn(),
            "owed",
            Pk::new(7),
            "{}",
            now - Duration::from_secs(60),
            "https://receiver.example",
        )
        .unwrap();
        let done = crate::store::deliveries::enqueue(
            db.conn(),
            "done",
            Pk::new(7),
            "{}",
            long_ago,
            "https://receiver.example",
        )
        .unwrap();
        crate::store::deliveries::delivered(db.conn(), done, 200, long_ago).unwrap();

        prune(db.conn(), now).unwrap();

        assert_eq!(
            crate::store::deliveries::state(db.conn(), owed)
                .unwrap()
                .as_deref(),
            Some("pending"),
            "a report that has not been delivered is still owed"
        );
        assert!(
            crate::store::deliveries::state(db.conn(), done)
                .unwrap()
                .is_none(),
            "a settled one from a week ago is only a record"
        );
    }

    /// The other half: a report nothing ever retried does not stay owed forever.
    ///
    /// `deliveries::failed` is reached only by a run that *tries* the report,
    /// so removing `[webhook]` from the configuration, or a walk that failed
    /// before the delivery step, must not leave rows owed indefinitely. Marked
    /// rather than deleted, so `status` can still say why.
    #[test]
    fn a_report_too_old_to_be_news_stops_being_owed() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        let now = snob_core::clock::now();
        let stale = now - Duration::from_secs(crate::store::deliveries::MAX_AGE_SECS as u64 + 1);
        let id = crate::store::deliveries::enqueue(
            db.conn(),
            "stale",
            Pk::new(7),
            "{}",
            stale,
            "https://receiver.example",
        )
        .unwrap();
        assert_eq!(crate::store::deliveries::pending(db.conn()).unwrap(), 1);

        prune(db.conn(), now).unwrap();

        assert_eq!(
            crate::store::deliveries::state(db.conn(), id)
                .unwrap()
                .as_deref(),
            Some("expired")
        );
        assert!(
            crate::store::deliveries::due(db.conn(), now, 10, "https://receiver.example")
                .unwrap()
                .is_empty(),
            "and it does not go out as news"
        );
    }

    /// Runs are a log. `--every 30m` over three accounts writes some fifty
    /// thousand rows a year, and they have to be swept.
    #[test]
    fn the_run_log_does_not_grow_without_end() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        let now = snob_core::clock::now();
        for age in [KEEP_RUNS_FOR_SECS as u64 + 1, 60] {
            record_run(
                db.conn(),
                &Run {
                    account_pk: Pk::new(7),
                    started_at: now - Duration::from_secs(age),
                    finished_at: Some(now - Duration::from_secs(age)),
                    requests: 1,
                    outcome: Some(RunOutcome::Ok.into()),
                    changes: 0,
                    posts_skipped: None,
                    due_at: None,
                    began_at: None,
                },
            )
            .unwrap();
        }

        prune(db.conn(), now).unwrap();

        let left: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM watch_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(left, 1, "the old one goes and the recent one stays");
        assert_eq!(
            one_of(&last_runs(db.conn()).unwrap(), Pk::new(7)).started_at,
            now - Duration::from_secs(60)
        );
    }

    /// A run belongs to an account, and `status` speaks per account.
    ///
    /// One query with no predicate always answered with whichever account came
    /// last in the configuration, so `status` said "found nothing" on a tick
    /// where an earlier account had found five.
    #[test]
    fn the_last_run_is_the_last_run_of_that_account() {
        let mut db = Store::in_memory().unwrap();
        account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);
        account_with_capture(&mut db, Pk::new(8), &[user(2, "two")]);

        let now = snob_core::clock::now();
        for (pk, changes) in [(Pk::new(7), 5u32), (Pk::new(8), 0)] {
            record_run(
                db.conn(),
                &Run {
                    account_pk: pk,
                    started_at: now,
                    finished_at: Some(now),
                    requests: 1,
                    outcome: Some(RunOutcome::Ok.into()),
                    changes,
                    posts_skipped: None,
                    due_at: None,
                    began_at: None,
                },
            )
            .unwrap();
        }

        assert_eq!(
            one_of(&last_runs(db.conn()).unwrap(), Pk::new(7)).changes,
            5
        );
        assert_eq!(
            one_of(&last_runs(db.conn()).unwrap(), Pk::new(8)).changes,
            0
        );
    }

    /// `username_history` holds everybody this tool has ever seen. A diff about
    /// one list may only speak about the people in it.
    #[test]
    fn somebody_outside_the_capture_is_not_reported() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "one")]);

        users::upsert(db.conn(), &user(2, "stranger")).unwrap();
        users::upsert(db.conn(), &user(2, "stranger_renamed")).unwrap();

        assert!(
            renames_since(db.conn(), id, 0, head_now(db.conn()))
                .unwrap()
                .is_empty()
        );
    }

    /// Somebody who changed their name twice moved once, from where they
    /// started. Reading the latest history row instead would report the middle
    /// name as the old one, which is a name the user never saw.
    #[test]
    fn two_renames_in_one_window_are_one_move_from_the_first_name() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "first")]);

        users::upsert(db.conn(), &user(1, "second")).unwrap();
        users::upsert(db.conn(), &user(1, "third")).unwrap();

        let found = renames_since(db.conn(), id, 0, head_now(db.conn())).unwrap();
        assert_eq!(found.len(), 1, "one account moved, so one line about it");
        assert_eq!(found[0].from, "first");
        assert_eq!(found[0].to, "third");
    }

    /// Renamed and renamed back is not news. The history has two rows and the
    /// current name equals the original, which would otherwise be announced as
    /// "@someone is now @someone".
    #[test]
    fn a_name_that_came_back_to_where_it_started_is_not_a_rename() {
        let mut db = Store::in_memory().unwrap();
        let id = account_with_capture(&mut db, Pk::new(7), &[user(1, "original")]);

        users::upsert(db.conn(), &user(1, "briefly")).unwrap();
        users::upsert(db.conn(), &user(1, "original")).unwrap();

        assert!(
            renames_since(db.conn(), id, 0, head_now(db.conn()))
                .unwrap()
                .is_empty()
        );
    }
}
