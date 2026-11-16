//! Snapshots: a follower or following list captured at one moment.
//!
//! Opened when the walk begins, filled page by page, closed at the end. Each
//! page commits in its own transaction, so stopping is saving.

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{Store, StoreError, pk_from_sql, pk_to_sql};
use snob_core::clock::{now, now_ms};
use snob_core::model::{ListKind, StopReason, User};
use snob_core::{Epoch, Pk};

/// How long an interrupted walk may still be resumed: past it, the two halves
/// would describe no single moment. A day and a half, since a list larger
/// than what is left of the day's accounts spans two days by design.
pub const RESUME_WINDOW_SECS: i64 = 36 * 3600;

/// How long a claim on a walk outlives the last page it saved: how long a
/// killed walk goes on looking busy. Strictly shorter than
/// [`RESUME_WINDOW_SECS`], or no process could ever adopt one. Forty-five
/// minutes covers the longest quiet stretch, a sitting's rest of up to fifteen
/// (`pace::SITTING_PAUSE_MS`), plus a backoff; a walk asleep on the day's
/// accounts refreshes its claim with [`keep_claim`].
pub const CLAIM_TTL_SECS: i64 = 45 * 60;

const _: () = assert!(CLAIM_TTL_SECS < RESUME_WINDOW_SECS);

/// Who this process is: the pid and the moment it was first asked, unique
/// among the processes that can be running at once.
fn this_process() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| format!("{}-{}", std::process::id(), now_ms()))
}

/// A capture, as much of it as anything reads. `complete` is not a field on
/// purpose: the check is the `usable_snapshots` view, not whoever remembers
/// to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub id: i64,
    pub account_pk: Pk,
    pub kind: ListKind,
    pub started_at: Epoch,
    pub taken_at: Option<Epoch>,
    pub member_count: u64,
    pub declared_count: Option<u64>,
    pub next_cursor: Option<String>,
}

/// The columns [`row_to_snapshot`] reads **by position**: every statement it
/// reads from selects this list.
const SNAPSHOT_COLUMNS: &str = concat!(
    "id, account_pk, kind, started_at, taken_at, member_count, ",
    "declared_count, next_cursor"
);

/// What changed when a page was saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedPage {
    /// How many users were not already in the snapshot. Instagram's pagination
    /// repeats accounts across pages, so this rarely matches what arrived.
    pub added: usize,
    /// Accounts that have renamed themselves since we last saw them.
    pub renamed: Vec<(Pk, String)>,
}

/// The snapshot a walk has just opened, with the `started_at` the row holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opened {
    pub id: i64,
    pub started_at: Epoch,
}

/// Starts a new snapshot, claimed by this process.
pub fn begin(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
    declared_count: Option<u64>,
) -> Result<Opened, StoreError> {
    let started_at = now();
    conn.execute(
        "INSERT INTO snapshots
            (account_pk, kind, source, started_at, declared_count, claimed_by, claimed_at)
         VALUES (?1, ?2, 'live', ?3, ?4, ?5, ?3)",
        params![
            pk_to_sql(account_pk),
            kind.as_str(),
            started_at.get(),
            declared_count.map(super::count_to_sql),
            this_process(),
        ],
    )?;
    Ok(Opened {
        id: conn.last_insert_rowid(),
        started_at,
    })
}

/// What makes a partial capture resumable, shared by [`resumable`], which
/// takes it, and [`is_resumable`], which only asks.
///
/// `?1` the account, `?2` the list, `?3` the oldest `started_at` inside the
/// resume window, `?4` the newest `claimed_at` that counts as abandoned, `?5`
/// the process whose own claim does not disqualify a row: this process when
/// taking, NULL (which makes the clause inert) when asking, since the run
/// that continues the walk is never the one asking.
const RESUMABLE: &str = concat!(
    "account_pk = ?1 AND kind = ?2 AND complete = 0
     AND next_cursor IS NOT NULL AND started_at >= ?3
     AND ",
    claim_available!("?4", "?5")
);

/// Whether a claim leaves a row available, spelled once for [`RESUMABLE`],
/// [`delete_partials`] and [`walked_elsewhere`]. `$stale` is the newest
/// `claimed_at` that counts as abandoned, `$process` the one whose own claim
/// does not count. A macro because the statements number their parameters
/// differently.
macro_rules! claim_available {
    ($stale:literal, $process:literal) => {
        concat!(
            "(claimed_by IS NULL OR claimed_at IS NULL OR claimed_at < ",
            $stale,
            " OR claimed_by = ",
            $process,
            ")"
        )
    };
}
use claim_available;

/// The oldest `started_at` still resumable, and the newest `claimed_at` that
/// counts as abandoned.
fn resume_bounds(now: snob_core::Epoch) -> (i64, i64) {
    (
        (now - Duration::from_secs(RESUME_WINDOW_SECS as u64)).get(),
        (now - Duration::from_secs(CLAIM_TTL_SECS as u64)).get(),
    )
}

/// Looks for an interrupted walk this process may continue, and takes it,
/// counting the resume. **The claim is taken in the statement that finds the
/// row**, so two processes racing for one partial cannot both win.
pub fn resumable(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<Option<Snapshot>, StoreError> {
    let now = now();

    // `RETURNING`, so what comes back is the row that was claimed: a read-back
    // by this process's claim could return a newer claimed row with no cursor.
    let snapshot = conn
        .query_row(
            &format!(
                "UPDATE snapshots
                 SET claimed_by = ?5, claimed_at = ?6, resumes = resumes + 1
                 WHERE id = (
                   SELECT id FROM snapshots
                   WHERE {RESUMABLE}
                   ORDER BY started_at DESC, id DESC LIMIT 1
                 )
                 RETURNING {SNAPSHOT_COLUMNS}"
            ),
            params![
                pk_to_sql(account_pk),
                kind.as_str(),
                resume_bounds(now).0,
                resume_bounds(now).1,
                this_process(),
                now.get(),
            ],
            row_to_snapshot,
        )
        .optional()?;
    Ok(snapshot)
}

/// Whether an interrupted walk could be continued by the **next** process:
/// asked, never taken, and "claimed by me" does not count as available.
pub fn is_resumable(conn: &Connection, account_pk: Pk, kind: ListKind) -> Result<bool, StoreError> {
    Ok(resumable_progress(conn, account_pk, kind)?.is_some())
}

/// How many accounts the walk the next run would continue has stored so far,
/// asked as [`is_resumable`] asks.
pub fn resumable_progress(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<Option<u64>, StoreError> {
    let now = now();
    let found: Option<i64> = conn
        .query_row(
            &format!(
                "SELECT member_count FROM snapshots WHERE {RESUMABLE}
                 ORDER BY started_at DESC, id DESC LIMIT 1"
            ),
            params![
                pk_to_sql(account_pk),
                kind.as_str(),
                resume_bounds(now).0,
                resume_bounds(now).1,
                None::<&str>,
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.map(|members| super::count_from_sql(members).unwrap_or(0)))
}

/// Whether another process is walking this list right now: a capture not yet
/// complete whose claim is somebody else's and still fresh.
pub fn walked_elsewhere(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<bool, StoreError> {
    let found: Option<i64> = conn
        .query_row(
            concat!(
                "SELECT 1 FROM snapshots
                 WHERE account_pk = ?1 AND kind = ?2 AND complete = 0 AND NOT ",
                claim_available!("?3", "?4"),
                " LIMIT 1"
            ),
            params![
                pk_to_sql(account_pk),
                kind.as_str(),
                resume_bounds(now()).1,
                this_process(),
            ],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Refreshes the claim of a walk that waits rather than saves (asleep on the
/// day's accounts). `false` means the claim is somebody else's now, and the
/// sleeper should stop.
pub fn keep_claim(store: &Store, id: i64) -> Result<bool, StoreError> {
    let held = store.conn().execute(
        "UPDATE snapshots SET claimed_at = ?3
         WHERE id = ?1 AND claimed_by = ?2 AND complete = 0",
        params![id, this_process(), now().get()],
    )?;
    Ok(held > 0)
}

/// Lets go of a walk that stopped on an error, so the next run resumes it at
/// once instead of starting over while the claim ages. The cursor and the
/// pages are kept.
pub fn release(conn: &Connection, id: i64) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE snapshots SET claimed_by = NULL, claimed_at = NULL
         WHERE id = ?1 AND claimed_by = ?2 AND complete = 0",
        params![id, this_process()],
    )?;
    Ok(())
}

/// Saves a whole page in one transaction: the users, their membership and the
/// cursor. Committing per page is what makes partial progress durable.
pub fn save_page(
    store: &mut Store,
    id: i64,
    users: &[User],
    cursor: Option<&str>,
) -> Result<SavedPage, StoreError> {
    // `BEGIN IMMEDIATE`: this reads before it writes, and upgrading a deferred
    // transaction can fail with `SQLITE_BUSY_SNAPSHOT`, which the busy handler
    // does not cover (see `SqliteRateBudget::reserve`).
    let tx = store
        .conn_mut()
        .transaction_with_behavior(TransactionBehavior::Immediate)?;

    let first_ordinal: i64 = tx.query_row(
        "SELECT member_count FROM snapshots WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    let mut result = SavedPage::default();

    {
        let mut member = tx.prepare_cached(
            "INSERT OR IGNORE INTO snapshot_members (snapshot_id, user_pk, ordinal)
             VALUES (?1, ?2, ?3)",
        )?;

        for u in users {
            if let Some(old) = super::users::upsert(&tx, u)? {
                result.renamed.push((u.pk, old));
            }

            // The ordinal advances with `added` alone: Instagram repeats
            // accounts across pages, and counting a repeat would put a later
            // page's ordinals below an earlier one's.
            let inserted = member.execute(params![
                id,
                pk_to_sql(u.pk),
                first_ordinal + result.added as i64
            ])?;
            if inserted > 0 {
                result.added += 1;
            }
        }
    }

    // `claimed_at` moves with every page: progress is the evidence somebody
    // is still here. `claimed_by = ?4` leaves one writer per snapshot: a walk
    // whose row was adopted must not interleave its pages with the adopter's.
    let held = tx.execute(
        "UPDATE snapshots
         SET member_count = member_count + ?2,
             pages        = pages + 1,
             requests     = requests + 1,
             next_cursor  = ?3,
             claimed_at   = ?5
         WHERE id = ?1 AND claimed_by = ?4",
        params![id, result.added as i64, cursor, this_process(), now().get()],
    )?;
    if held == 0 {
        // Dropping the transaction rolls the page back.
        return Err(StoreError::ClaimTaken);
    }

    tx.commit()?;
    Ok(result)
}

/// Closes the snapshot, releasing the claim. Only a full walk leaves it
/// usable for comparison.
///
/// A finished capture is never unfinished again (`complete = 0`), and only
/// the process holding the claim may end it; a refusal is `ClaimTaken`, so a
/// walk whose lease was adopted never reports a capture it does not own.
pub fn close(conn: &Connection, id: i64, reason: StopReason) -> Result<(), StoreError> {
    let complete = reason.yields_complete_list();
    let closed = conn.execute(
        "UPDATE snapshots
         SET complete = ?2, taken_at = ?3, stopped_by = ?4,
             next_cursor = CASE WHEN ?2 = 1 THEN NULL ELSE next_cursor END,
             claimed_by = NULL, claimed_at = NULL
         WHERE id = ?1 AND complete = 0 AND claimed_by = ?5",
        params![id, complete, now().get(), reason.as_str(), this_process()],
    )?;
    if closed == 0 {
        // Already finished is not a refusal: the capture is whole, whoever
        // closed it.
        let finished: Option<bool> = conn
            .query_row(
                "SELECT complete = 1 FROM snapshots WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if finished != Some(true) {
            return Err(StoreError::ClaimTaken);
        }
    }
    Ok(())
}

/// The most recent usable snapshot, read from the view so that it cannot be
/// an incomplete one. `id DESC` breaks the tie between two walks closed in
/// the same second: the monitor compares this id against the one it reported.
pub fn latest_complete(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<Option<Snapshot>, StoreError> {
    let snapshot = conn
        .query_row(
            &format!(
                "SELECT {SNAPSHOT_COLUMNS}
                 FROM usable_snapshots
                 WHERE account_pk = ?1 AND kind = ?2
                 ORDER BY taken_at DESC, id DESC LIMIT 1"
            ),
            params![pk_to_sql(account_pk), kind.as_str()],
            row_to_snapshot,
        )
        .optional()?;
    Ok(snapshot)
}

/// Whether anything was ever captured of this list, finished or not: a walk
/// that stopped short still filed `username_history` rows for the people it
/// saw, which the rename window must not close over.
pub fn any_capture(conn: &Connection, account_pk: Pk, kind: ListKind) -> Result<bool, StoreError> {
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM snapshots WHERE account_pk = ?1 AND kind = ?2 LIMIT 1",
            params![pk_to_sql(account_pk), kind.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// One usable snapshot by id; `None` for a walk that stopped short.
pub fn find_usable(conn: &Connection, id: i64) -> Result<Option<Snapshot>, StoreError> {
    let snapshot = conn
        .query_row(
            &format!("SELECT {SNAPSHOT_COLUMNS} FROM usable_snapshots WHERE id = ?1"),
            params![id],
            row_to_snapshot,
        )
        .optional()?;
    Ok(snapshot)
}

/// The users in a snapshot, in the order Instagram served them.
pub fn members(conn: &Connection, id: i64) -> Result<Vec<User>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM snapshot_members m JOIN users u ON u.pk = m.user_pk
         WHERE m.snapshot_id = ?1
         ORDER BY m.ordinal",
        super::users::USER_COLUMNS
    ))?;
    let rows = stmt.query_map(params![id], super::users::row_to_user)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The users the walk of `account_pk`'s `kind` **this process** is making
/// has saved so far, in the order Instagram served them, and how many the
/// account declared: for a view that shows a list as it is read, from a
/// connection of its own while the walk commits its pages. Empty when this
/// process walks no such list.
pub fn walking_here(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<(Vec<User>, Option<u64>), StoreError> {
    let walking: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, declared_count FROM snapshots
             WHERE account_pk = ?1 AND kind = ?2 AND complete = 0 AND claimed_by = ?3
             ORDER BY started_at DESC, id DESC LIMIT 1",
            params![pk_to_sql(account_pk), kind.as_str(), this_process()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((id, declared)) = walking else {
        return Ok((Vec::new(), None));
    };
    let declared = declared.and_then(super::count_from_sql);
    Ok((members(conn, id)?, declared))
}

/// Discards half-finished walks for this account and list. Called when starting
/// a new one, so partials that no longer serve do not pile up.
pub fn delete_partials(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<usize, StoreError> {
    // A walk another process is writing to is left alone; a stale claim
    // protects nothing. The resume window is not asked about: an expired or
    // cursor-less partial is exactly what this clears.
    let deleted = conn.execute(
        concat!(
            "DELETE FROM snapshots
         WHERE account_pk = ?1 AND kind = ?2 AND complete = 0
           AND ",
            claim_available!("?4", "?3")
        ),
        params![
            pk_to_sql(account_pk),
            kind.as_str(),
            this_process(),
            resume_bounds(now()).1,
        ],
    )?;
    Ok(deleted)
}

/// How many of a list's newest whole captures are kept whatever their age:
/// the newest is what the cache serves, and the one before it is what `snob
/// changes` compares it with when asked nothing else.
const NEWEST_KEPT: i64 = 2;

/// Removes the whole captures nothing needs any more, and returns how many:
/// those taken longer ago than `watch::KEEP_FOR_SECS`, but for the capture
/// each of the monitor's marks points at (the baseline of its next diff;
/// deleting it is one silently missed report) and each list's newest
/// [`NEWEST_KEPT`]. A capture not finished is never one: a resume may be under
/// way, and [`delete_partials`] clears those.
///
/// **One rule for the monitor (`watch::prune`) and for a walk that has just
/// finished**, so that somebody who never runs the monitor does not keep
/// every capture for ever. It touches no report: the outbox is the monitor's
/// to sweep, since only its run can say one was given up on.
pub fn prune(conn: &Connection, now: Epoch) -> Result<usize, StoreError> {
    let cutoff = now - Duration::from_secs(super::watch::KEEP_FOR_SECS as u64);
    Ok(conn.execute(
        "DELETE FROM snapshots
         WHERE complete = 1
           AND taken_at IS NOT NULL
           AND taken_at < ?1
           AND id NOT IN (SELECT snapshot_id FROM watch_marks WHERE snapshot_id IS NOT NULL)
           AND id NOT IN (
             SELECT id FROM (
               SELECT id, row_number() OVER (
                 PARTITION BY account_pk, kind ORDER BY taken_at DESC, id DESC
               ) AS rank
               FROM usable_snapshots
             ) WHERE rank <= ?2
           )",
        params![cutoff.get(), NEWEST_KEPT],
    )?)
}

/// A capture as `snob snapshots` lists it: every row, finished or not, with
/// what the capture itself recorded about how it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub snapshot: Snapshot,
    /// `live` for a walk, `dyi` for Instagram's own export (`import dyi`).
    pub source: String,
    /// Whether it is a whole list, the only kind anything is compared with.
    pub complete: bool,
    /// How the walk ended (`StopReason::as_str`); `None` while it is open.
    pub stopped_by: Option<String>,
}

/// Every capture of `account_pk`'s lists, of `kind` or of both, newest first.
/// **Reads only**, and the partial ones too: the listing is where somebody
/// finds out a walk stopped short, and why.
pub fn listed(
    conn: &Connection,
    account_pk: Pk,
    kind: Option<ListKind>,
) -> Result<Vec<Listed>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SNAPSHOT_COLUMNS}, source, complete, stopped_by
         FROM snapshots
         WHERE account_pk = ?1 AND (?2 IS NULL OR kind = ?2)
         ORDER BY coalesce(taken_at, started_at) DESC, id DESC"
    ))?;
    let rows = stmt.query_map(
        params![pk_to_sql(account_pk), kind.map(ListKind::as_str)],
        |row| {
            Ok(Listed {
                snapshot: row_to_snapshot(row)?,
                source: row.get(8)?,
                complete: row.get::<_, i64>(9)? == 1,
                stopped_by: row.get(10)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every whole capture of `account_pk`'s list of `kind`, newest first: what
/// `snob changes` picks the two it compares from. Read from
/// `usable_snapshots`, so a walk that stopped short is never one of them.
pub fn usable(
    conn: &Connection,
    account_pk: Pk,
    kind: ListKind,
) -> Result<Vec<Snapshot>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SNAPSHOT_COLUMNS}
         FROM usable_snapshots
         WHERE account_pk = ?1 AND kind = ?2
         ORDER BY taken_at DESC, id DESC"
    ))?;
    let rows = stmt.query_map(
        params![pk_to_sql(account_pk), kind.as_str()],
        row_to_snapshot,
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Reads one capture, by position, from [`SNAPSHOT_COLUMNS`]. The `kind` is
/// parsed, never defaulted: a default would hand back the wrong list.
fn row_to_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<Snapshot> {
    let kind: String = row.get(2)?;
    let kind = kind.parse::<ListKind>().map_err(|()| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            format!("\"{kind}\" is not a list this version knows").into(),
        )
    })?;
    Ok(Snapshot {
        id: row.get(0)?,
        account_pk: pk_from_sql(row.get(1)?),
        kind,
        started_at: Epoch::new(row.get(3)?),
        taken_at: row.get::<_, Option<i64>>(4)?.map(Epoch::new),
        // Nothing stored where the count cannot be read; `members` is what
        // says who is in the capture.
        member_count: super::count_from_sql(row.get(5)?).unwrap_or(0),
        declared_count: row
            .get::<_, Option<i64>>(6)?
            .and_then(super::count_from_sql),
        next_cursor: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{accounts, users};

    fn user(pk: u64) -> User {
        User {
            pk: Pk::new(pk),
            username: format!("user{pk}"),
            full_name: None,
            is_private: None,
            is_verified: None,
            pfp_url: None,
        }
    }

    /// The one term the two resume questions do not share, pinned from both
    /// sides.
    ///
    /// They share one string with one bound value between them, which makes
    /// the asymmetry easy to read and easy to "simplify" away -- so it is
    /// written down here as behavior rather than as a comment.
    ///
    /// `resumable` counts this process's own claim as available, because
    /// resuming its own work after a restart inside the window is the point.
    /// `is_resumable` does not, because the run that continues a walk is never
    /// the one asking: bind `this_process()` there and the advice on screen
    /// promises a continuation the next invocation will refuse.
    #[test]
    fn asking_whether_a_walk_can_be_continued_answers_for_the_next_process() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("next")).unwrap();

        // Claimed by this process, which is what `begin` and `save_page` leave
        // behind.
        let held: Option<String> = db
            .conn()
            .query_row("SELECT claimed_by FROM snapshots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            held.as_deref(),
            Some(this_process()),
            "the fixture depends on this process holding the claim"
        );

        assert!(
            !is_resumable(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            "the next process will find this claim fresh and refuse it, so the advice \
             must not promise a continuation"
        );
        assert_eq!(
            resumable(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .map(|s| s.id),
            Some(id),
            "and this process picks up its own work, which is what the window is for"
        );
    }

    #[test]
    fn a_walk_this_process_is_making_is_read_as_far_as_it_went_and_no_other_is() {
        let mut db = base();
        assert_eq!(
            walking_here(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            (Vec::new(), None)
        );
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(30))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10), user(11)], Some("next")).unwrap();
        let (users, declared) = walking_here(db.conn(), Pk::new(1), ListKind::Followers).unwrap();
        assert_eq!(users, [user(10), user(11)]);
        assert_eq!(declared, Some(30));
        assert!(
            walking_here(db.conn(), Pk::new(1), ListKind::Following)
                .unwrap()
                .0
                .is_empty(),
            "another list"
        );
        claimed_by_somebody_else(&db, id, now());
        assert!(
            walking_here(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .0
                .is_empty(),
            "another process's walk"
        );
    }

    /// A capture whose kind this version cannot read is an error, not a
    /// following capture; `row_to_snapshot` says why.
    ///
    /// Asked of the projection directly, because a row like this is precisely
    /// what the table will not hold -- the guard has to hold for a database this
    /// program did not write.
    #[test]
    fn a_capture_whose_kind_is_unreadable_is_refused_rather_than_guessed_at() {
        let db = base();
        let read = db.conn().query_row(
            "SELECT 1, 1, 'sideways', 0, NULL, 0, NULL, NULL",
            [],
            row_to_snapshot,
        );
        assert!(
            matches!(
                &read,
                Err(rusqlite::Error::FromSqlConversionFailure(2, _, _))
            ),
            "a kind this version cannot read answered {read:?}"
        );
    }

    /// The two windows are numbers somebody chose, and every other test in the
    /// tree refers to them by name -- so `RESUME_WINDOW_SECS = 6 * 60` passes
    /// the whole suite while every walk paused for longer than six minutes
    /// silently begins again at page one, and `CLAIM_TTL_SECS = 60` makes a
    /// walk that pauses for a minute look abandoned to any other process.
    ///
    /// Written out, the way `the_default_pace_is_the_documented_one` writes out
    /// the pacing: changing one has to be deliberate and has to say so here.
    #[test]
    fn the_two_windows_are_the_documented_ones() {
        assert_eq!(RESUME_WINDOW_SECS, 36 * 3600);
        assert_eq!(CLAIM_TTL_SECS, 45 * 60);
    }

    fn base() -> Store {
        let db = Store::in_memory().unwrap();
        users::upsert(db.conn(), &user(1)).unwrap();
        accounts::upsert(db.conn(), Pk::new(1), true).unwrap();
        db
    }

    /// Pretends to be another process by writing its claim directly.
    ///
    /// `this_process` is a per-process constant, so a test cannot be a second
    /// process — but every read of a claim compares against that constant, and
    /// a row claimed by anything else is exactly what the other process leaves
    /// behind. `at` is when that process last saved a page.
    fn claimed_by_somebody_else(db: &Store, id: i64, at: Epoch) {
        db.conn()
            .execute(
                "UPDATE snapshots SET claimed_by = 'another-process', claimed_at = ?2
                 WHERE id = ?1",
                params![id, at.get()],
            )
            .unwrap();
    }

    /// A walk asleep on the day's accounts keeps its claim, and only its own.
    #[test]
    fn a_sleeping_walk_keeps_its_claim_and_nobody_elses() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        assert!(keep_claim(&db, id).unwrap(), "the walk's own claim");

        claimed_by_somebody_else(&db, id, now());
        assert!(
            !keep_claim(&db, id).unwrap(),
            "a claim another process took is not refreshed from here"
        );
    }

    /// A walk somebody else is in the middle of is not adopted.
    ///
    /// This is the whole point of the claim: one `snob watch` running while
    /// somebody types `snob followers` must not have both processes continue
    /// the same partial and write into one capture.
    #[test]
    fn a_walk_another_process_is_working_on_is_left_alone() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        claimed_by_somebody_else(&db, id, now());

        assert!(
            resumable(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_none(),
            "somebody else is walking this one"
        );
        assert!(walked_elsewhere(db.conn(), Pk::new(1), ListKind::Followers).unwrap());
        assert!(
            !walked_elsewhere(db.conn(), Pk::new(1), ListKind::Following).unwrap(),
            "only that list"
        );
    }

    /// A walk is not somebody else's while it is this process's own, or once
    /// its claim has gone stale.
    #[test]
    fn only_a_live_claim_of_another_process_is_a_walk_elsewhere() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        assert!(
            !walked_elsewhere(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            "this process's own"
        );
        claimed_by_somebody_else(
            &db,
            id,
            now() - Duration::from_secs(CLAIM_TTL_SECS as u64 + 1),
        );
        assert!(
            !walked_elsewhere(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            "abandoned"
        );
    }

    /// And one they abandoned is. Nothing can tell us a process died, so the
    /// evidence is that it has stopped saving pages.
    #[test]
    fn a_walk_abandoned_long_enough_is_adopted() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        claimed_by_somebody_else(
            &db,
            id,
            now() - Duration::from_secs(CLAIM_TTL_SECS as u64 + 1),
        );

        let adopted = resumable(db.conn(), Pk::new(1), ListKind::Followers)
            .unwrap()
            .expect("whoever held it is gone");
        assert_eq!(adopted.id, id);
    }

    /// A walk that failed lets go, and the next run — a different process as
    /// far as the claim can tell — picks it up where it stopped.
    #[test]
    fn a_failed_walk_is_resumable_at_once() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        assert!(
            !is_resumable(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            "held by the walk that is still running"
        );

        release(db.conn(), id).unwrap();
        assert!(is_resumable(db.conn(), Pk::new(1), ListKind::Followers).unwrap());
    }

    /// Taking it is what `resumable` does, not something the caller remembers
    /// to do afterwards — so asking twice from two places cannot hand it out
    /// twice.
    #[test]
    fn resuming_takes_the_claim_in_the_same_breath() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        // Free for anybody.
        db.conn()
            .execute(
                "UPDATE snapshots SET claimed_by = NULL, claimed_at = NULL WHERE id = ?1",
                params![id],
            )
            .unwrap();

        assert!(
            resumable(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_some()
        );

        let holder: Option<String> = db
            .conn()
            .query_row(
                "SELECT claimed_by FROM snapshots WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(holder.as_deref(), Some(this_process()));
    }

    /// Asking whether a walk could be continued does not continue it;
    /// `is_resumable` says what claiming on the way past would cost.
    #[test]
    fn asking_whether_a_walk_is_resumable_does_not_claim_it() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        close(db.conn(), id, StopReason::Canceled).unwrap();

        assert!(
            is_resumable(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            "the walk stopped with a cursor, inside the window"
        );

        let holder: Option<String> = db
            .conn()
            .query_row(
                "SELECT claimed_by FROM snapshots WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(holder, None, "asking is not taking");
    }

    /// There is a window in which another process can adopt a killed walk.
    ///
    /// A hard kill never reaches `close`, so the claim `save_page` last wrote
    /// stands. The ordering of the two constants is held by the `const`
    /// assertion beside them, and `CLAIM_TTL_SECS` says why; this is the state
    /// it makes reachable.
    #[test]
    fn a_claim_goes_stale_while_the_partial_is_still_worth_resuming() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();

        // A walk that started well inside the resume window and whose process
        // was killed a moment after its last page: no `close`, so the claim is
        // still there.
        let started = now() - Duration::from_secs(CLAIM_TTL_SECS as u64 + 60);
        db.conn()
            .execute(
                "UPDATE snapshots SET started_at = ?2 WHERE id = ?1",
                params![id, started.get()],
            )
            .unwrap();
        claimed_by_somebody_else(&db, id, started + Duration::from_secs(30));

        let adopted = resumable(db.conn(), Pk::new(1), ListKind::Followers)
            .unwrap()
            .expect("the claim is stale and the partial is young");
        assert_eq!(adopted.id, id);
    }

    /// A page for a walk this process no longer holds is refused, not written.
    ///
    /// The claim only bounds how long an abandoned walk looks busy; it cannot
    /// stop the walk that was abandoned from waking up, and its next page must
    /// not interleave with the adopter's.
    #[test]
    fn a_page_for_a_walk_somebody_else_took_over_is_refused() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        claimed_by_somebody_else(&db, id, now());

        let refused = save_page(&mut db, id, &[user(11)], Some("further"));
        assert!(matches!(refused, Err(StoreError::ClaimTaken)));

        let (count, cursor): (i64, Option<String>) = db
            .conn()
            .query_row(
                "SELECT member_count, next_cursor FROM snapshots WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "the refused page rolled back");
        assert_eq!(cursor.as_deref(), Some("cursor"), "the cursor is untouched");
    }

    /// Closing a walk this process no longer holds is refused, like saving one;
    /// `close` says why.
    #[test]
    fn closing_a_walk_somebody_else_took_over_is_refused() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("cursor")).unwrap();
        claimed_by_somebody_else(&db, id, now());

        let refused = close(db.conn(), id, StopReason::Completed);
        assert!(
            matches!(refused, Err(StoreError::ClaimTaken)),
            "a close that changed nothing has to say so, or the caller reports a \
             finished capture it does not own: {refused:?}"
        );

        let (complete, claimed): (i64, Option<String>) = db
            .conn()
            .query_row(
                "SELECT complete, claimed_by FROM snapshots WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(complete, 0, "the adopter's walk is still in progress");
        assert_eq!(
            claimed.as_deref(),
            Some("another-process"),
            "and it still holds the claim it took"
        );
    }

    /// The row that comes back is the row that was claimed, not a newer
    /// cursor-less row this process left behind: `walk::fetch` leaves exactly
    /// that when a save or the budget fails, since it returns without calling
    /// `close`. The wrong row would restart the walk from page one under the
    /// wrong `started_at` and abandon the real partial still holding this
    /// process's claim.
    #[test]
    fn resuming_returns_the_partial_it_claimed_and_not_a_newer_dead_one() {
        let mut db = base();

        // The partial worth resuming: it has a cursor.
        let wanted = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, wanted, &[user(10)], Some("cursor")).unwrap();

        // And a newer row this process opened and walked away from without
        // closing, so it is claimed, incomplete and has no cursor at all.
        let abandoned = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;

        // Forced apart in the fixture rather than left to the clock. `begin`
        // stamps `started_at` in whole seconds, so two rows opened in one test
        // share it and `ORDER BY started_at DESC` is a tie SQLite may break
        // either way — which would make this pass or fail on timing rather than
        // on the thing it is about.
        db.conn()
            .execute(
                "UPDATE snapshots SET started_at = started_at - 10 WHERE id = ?1",
                params![wanted],
            )
            .unwrap();
        assert!(abandoned > wanted, "the dead one has to be the newer row");

        let adopted = resumable(db.conn(), Pk::new(1), ListKind::Followers)
            .unwrap()
            .expect("there is a partial with a cursor to continue");
        assert_eq!(
            adopted.id, wanted,
            "the row handed back must be the row the claim was taken on"
        );
        assert_eq!(adopted.next_cursor.as_deref(), Some("cursor"));
    }

    /// A capture that stopped short is not found by id either.
    ///
    /// `find_usable` reads the view rather than the table: the monitor holds an
    /// id and asks for it back, and a half-walked list handed over there is two
    /// hundred people reported as having left. A capture that is complete, and
    /// a row that was deleted, pass with `FROM snapshots` just as well, so this
    /// asks about the two kinds of incomplete row.
    #[test]
    fn a_capture_that_stopped_short_is_never_found_by_id() {
        let mut db = base();

        let cut_short = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, cut_short, &[user(10)], Some("cursor")).unwrap();
        close(db.conn(), cut_short, StopReason::RateLimit).unwrap();
        assert!(
            find_usable(db.conn(), cut_short).unwrap().is_none(),
            "a walk that stopped short is not something to compare against"
        );

        // The other half of the view: begun, never closed, so no `taken_at`.
        let still_open = begin(db.conn(), Pk::new(1), ListKind::Following, Some(10))
            .unwrap()
            .id;
        assert!(find_usable(db.conn(), still_open).unwrap().is_none());

        // And one it does answer for, so this cannot pass by refusing
        // everything.
        let whole = begin(db.conn(), Pk::new(1), ListKind::Following, Some(1))
            .unwrap()
            .id;
        save_page(&mut db, whole, &[user(11)], None).unwrap();
        close(db.conn(), whole, StopReason::Completed).unwrap();
        assert!(find_usable(db.conn(), whole).unwrap().is_some());
    }

    /// A finished capture is never unfinished again; `close` says why.
    #[test]
    fn closing_a_finished_capture_again_cannot_unfinish_it() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], None).unwrap();
        close(db.conn(), id, StopReason::Completed).unwrap();

        close(db.conn(), id, StopReason::RateLimit).unwrap();

        assert!(
            find_usable(db.conn(), id).unwrap().is_some(),
            "a capture that was finished stopped being usable"
        );
    }

    /// Starting a walk clears abandoned partials and leaves live ones alone.
    #[test]
    fn clearing_partials_spares_the_one_somebody_is_writing_to() {
        let mut db = base();

        let live = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, live, &[user(10)], Some("cursor")).unwrap();
        claimed_by_somebody_else(&db, live, now());

        let abandoned = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        save_page(&mut db, abandoned, &[user(11)], Some("cursor")).unwrap();
        claimed_by_somebody_else(
            &db,
            abandoned,
            now() - Duration::from_secs(CLAIM_TTL_SECS as u64 + 1),
        );

        assert_eq!(
            delete_partials(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            1
        );

        let left: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM snapshots WHERE id = ?1",
                params![live],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 1, "the walk somebody is in the middle of was deleted");
    }

    #[test]
    fn a_fresh_snapshot_is_not_complete() {
        let db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(10))
            .unwrap()
            .id;
        assert!(
            latest_complete(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_none()
        );
        assert_eq!(members(db.conn(), id).unwrap().len(), 0);
    }

    #[test]
    fn saving_pages_accumulates_members_in_order() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, Some(4))
            .unwrap()
            .id;

        save_page(&mut db, id, &[user(10), user(11)], Some("c1")).unwrap();
        save_page(&mut db, id, &[user(12), user(13)], None).unwrap();

        let pks: Vec<_> = members(db.conn(), id)
            .unwrap()
            .into_iter()
            .map(|u| u.pk.get())
            .collect();
        assert_eq!(pks, vec![10, 11, 12, 13]);
    }

    #[test]
    fn repeats_across_pages_are_not_counted_twice() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;

        let first = save_page(&mut db, id, &[user(10), user(11)], Some("c")).unwrap();
        // Instagram repeats accounts across pages when the list shifts.
        let second = save_page(&mut db, id, &[user(11), user(12)], None).unwrap();

        assert_eq!(first.added, 2);
        assert_eq!(second.added, 1);
        assert_eq!(members(db.conn(), id).unwrap().len(), 3);
    }

    /// Instagram repeats accounts across pages when the list shifts under it.
    /// A repeat must not push the ordinal forward, or the accounts after it
    /// come back in a different order than they were served in — and that
    /// order is the chronological one the user reads.
    #[test]
    fn repeats_do_not_disturb_the_order_of_what_follows() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;

        save_page(&mut db, id, &[user(10), user(11)], Some("c1")).unwrap();
        // The first two come round again, with one genuinely new behind them.
        save_page(&mut db, id, &[user(10), user(11), user(12)], Some("c2")).unwrap();
        save_page(&mut db, id, &[user(13)], None).unwrap();

        let pks: Vec<_> = members(db.conn(), id)
            .unwrap()
            .into_iter()
            .map(|u| u.pk.get())
            .collect();
        assert_eq!(pks, vec![10, 11, 12, 13]);
    }

    #[test]
    fn saving_a_page_advances_the_cursor() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("next")).unwrap();

        let pending = resumable(db.conn(), Pk::new(1), ListKind::Followers)
            .unwrap()
            .unwrap();
        assert_eq!(pending.id, id);
        assert_eq!(pending.next_cursor.as_deref(), Some("next"));

        // `pages` is a column and not a field, so this asks the column: it is
        // what asserts that `pages = pages + 1` in `save_page` ever runs.
        let pages: i64 = db
            .conn()
            .query_row("SELECT pages FROM snapshots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(pages, 1, "a saved page is a page the capture has paid for");
    }

    #[test]
    fn renames_are_reported_when_saving() {
        let mut db = base();
        users::upsert(db.conn(), &user(10)).unwrap();

        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        let mut renamed = user(10);
        renamed.username = "brand_new_name".into();

        let saved = save_page(&mut db, id, &[renamed], None).unwrap();
        assert_eq!(saved.renamed, vec![(Pk::new(10), "user10".to_string())]);
    }

    /// The most important rule of the store: a half-finished snapshot can never
    /// be the basis of a comparison, because everything missing from it would
    /// show up as a departure.
    #[test]
    fn latest_complete_never_returns_a_partial_one() {
        let mut db = base();

        for reason in [
            StopReason::Canceled,
            StopReason::Truncated,
            StopReason::RateLimit,
            StopReason::Network,
            StopReason::SessionInvalid,
            StopReason::PageLimit,
        ] {
            let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
                .unwrap()
                .id;
            save_page(&mut db, id, &[user(10)], None).unwrap();
            close(db.conn(), id, reason).unwrap();

            assert!(
                latest_complete(db.conn(), Pk::new(1), ListKind::Followers)
                    .unwrap()
                    .is_none(),
                "a snapshot cut short by {reason:?} should not be usable"
            );
        }

        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], None).unwrap();
        close(db.conn(), id, StopReason::Completed).unwrap();
        assert!(
            latest_complete(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_some()
        );
    }

    /// "The newest capture" has to have one answer, not either of two;
    /// `latest_complete` says why.
    #[test]
    fn the_newest_of_two_captures_taken_in_one_second_is_the_later_one() {
        let mut db = base();

        let mut ids = Vec::new();
        for _ in 0..2 {
            let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
                .unwrap()
                .id;
            save_page(&mut db, id, &[user(10)], None).unwrap();
            close(db.conn(), id, StopReason::Completed).unwrap();
            ids.push(id);
        }

        // Forced, not hoped for. `close` stamps `taken_at` from the clock and
        // nothing here freezes it, and a runner loaded enough to be preempted
        // between the two closes would put them in different seconds.
        db.conn()
            .execute(
                "UPDATE snapshots SET taken_at = 1000 WHERE complete = 1",
                [],
            )
            .unwrap();

        assert_eq!(
            latest_complete(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .unwrap()
                .id,
            ids[1]
        );
    }

    /// Guards against the enum and the schema drifting apart: every variant has
    /// to be accepted by the CHECK constraint.
    #[test]
    fn every_stop_reason_is_accepted_by_the_schema() {
        let mut db = base();
        for reason in StopReason::ALL {
            let id = begin(db.conn(), Pk::new(1), ListKind::Following, None)
                .unwrap()
                .id;
            save_page(&mut db, id, &[user(10)], None).unwrap();
            close(db.conn(), id, reason)
                .unwrap_or_else(|e| panic!("the schema rejected {reason:?}: {e}"));
        }
    }

    #[test]
    fn closing_cleanly_clears_the_pending_cursor() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("leftover")).unwrap();
        close(db.conn(), id, StopReason::Completed).unwrap();

        let snapshot = latest_complete(db.conn(), Pk::new(1), ListKind::Followers)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.next_cursor, None);
        assert!(
            resumable(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_interrupted_one_keeps_its_cursor_to_resume() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("from_here")).unwrap();
        close(db.conn(), id, StopReason::Canceled).unwrap();

        let pending = resumable(db.conn(), Pk::new(1), ListKind::Followers)
            .unwrap()
            .unwrap();
        assert_eq!(pending.next_cursor.as_deref(), Some("from_here"));
    }

    #[test]
    fn something_too_old_is_not_resumed() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10)], Some("stale")).unwrap();

        let long_ago = now() - Duration::from_secs(RESUME_WINDOW_SECS as u64 + 1);
        db.conn()
            .execute(
                "UPDATE snapshots SET started_at = ?1 WHERE id = ?2",
                params![long_ago.get(), id],
            )
            .unwrap();

        assert!(
            resumable(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_two_lists_do_not_mix() {
        let mut db = base();
        let followers = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, followers, &[user(10)], None).unwrap();
        close(db.conn(), followers, StopReason::Completed).unwrap();

        assert!(
            latest_complete(db.conn(), Pk::new(1), ListKind::Following)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn deleting_a_snapshot_deletes_its_members() {
        let mut db = base();
        let id = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, id, &[user(10), user(11)], None).unwrap();

        db.conn()
            .execute("DELETE FROM snapshots WHERE id = ?1", params![id])
            .unwrap();

        let left: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM snapshot_members", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(left, 0, "the foreign key cascade did not fire");
    }

    #[test]
    fn deleting_partials_spares_the_complete_ones() {
        let mut db = base();
        let good = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, good, &[user(10)], None).unwrap();
        close(db.conn(), good, StopReason::Completed).unwrap();

        let bad = begin(db.conn(), Pk::new(1), ListKind::Followers, None)
            .unwrap()
            .id;
        save_page(&mut db, bad, &[user(11)], Some("c")).unwrap();

        assert_eq!(
            delete_partials(db.conn(), Pk::new(1), ListKind::Followers).unwrap(),
            1
        );
        assert!(
            latest_complete(db.conn(), Pk::new(1), ListKind::Followers)
                .unwrap()
                .is_some()
        );
    }
}
