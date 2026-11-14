//! The outbox: reports owed to the address the user chose.
//!
//! A report is queued before the mark that makes it old news moves, a retry
//! sends the bytes that were signed, the wait between attempts grows, and an
//! attempt that will never succeed is given up on. Retrying is allowed here
//! (the user's own server), never against Instagram.

use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use super::{StoreError, pk_to_sql};
use snob_core::{Epoch, Pk};

/// How long to wait before the first retry. Each attempt doubles it.
const FIRST_BACKOFF_SECS: i64 = 60;

/// The longest wait between attempts, so a queue that has been failing for a
/// day still tries roughly hourly rather than settling into a week.
const MAX_BACKOFF_SECS: i64 = 3_600;

/// How many attempts before a report is given up on: the backstop, since
/// [`MAX_AGE_SECS`] is the bound meant to decide. Six doubling waits (about
/// an hour together) and then hourly pass that week (168 hours) before 180.
const MAX_ATTEMPTS: i64 = 180;

/// How old a report may get before it is given up on. **A week, not a day**:
/// the queue is drained only by a run, and with a daily schedule a day here
/// gave a report up just before the one run that could have sent it.
pub const MAX_AGE_SECS: i64 = 7 * 24 * 3_600;

/// A report waiting to be sent. `created_at` is not a field on purpose: its
/// age is [`MAX_AGE_SECS`]'s to judge, in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub id: i64,
    pub run_id: String,
    /// The exact bytes to send, and the ones the signature covers.
    pub body: String,
    pub attempts: i64,
    /// The event and the type it was queued with; `None` for a row queued
    /// before they were kept, which is snob's JSON (`017_delivery_form.sql`).
    pub form: Option<Form>,
}

/// What a body is, kept beside it because the body itself may not say:
/// the summary posted to Slack names no event, and plain text is no JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    /// What the `X-Snob-Event` header says.
    pub event: String,
    /// What the `Content-Type` header says.
    pub content_type: String,
}

/// Queues a report of snob's JSON; [`enqueue_as`] queues one with its form.
/// Runs inside the caller's transaction, with the mark the report makes
/// stale. `destination` is what [`due`] filters on.
pub fn enqueue(
    conn: &Connection,
    run_id: &str,
    account_pk: Pk,
    body: &str,
    at: Epoch,
    destination: &str,
) -> Result<i64, StoreError> {
    enqueue_as(conn, run_id, account_pk, body, at, destination, None)
}

/// Queues a report with the event and the type it is to be sent as.
pub fn enqueue_as(
    conn: &Connection,
    run_id: &str,
    account_pk: Pk,
    body: &str,
    at: Epoch,
    destination: &str,
    form: Option<&Form>,
) -> Result<i64, StoreError> {
    conn.execute(
        "INSERT INTO watch_deliveries
            (run_id, account_pk, created_at, body, next_try_at, destination,
             event, content_type)
         VALUES (?1, ?2, ?3, ?4, ?3, ?5, ?6, ?7)",
        params![
            run_id,
            pk_to_sql(account_pk),
            at.get(),
            body,
            destination,
            form.map(|form| form.event.as_str()),
            form.map(|form| form.content_type.as_str()),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The moment a report has to be newer than to still be news: one function
/// for [`due`], [`failed`] and [`expire_stale`]. Too old the moment it
/// reaches [`MAX_AGE_SECS`].
fn still_news_after(now: Epoch) -> Epoch {
    now - Duration::from_secs(MAX_AGE_SECS as u64)
}

/// Gives up on reports that have grown too old to be news, tried or not: a
/// removed `[webhook]` or a run that fails before delivering must not leave
/// rows owed for ever. Marked rather than deleted, so `status` can say what
/// became of them.
pub(super) fn expire_stale(conn: &Connection, now: Epoch) -> Result<usize, StoreError> {
    let expired = conn.execute(
        "UPDATE watch_deliveries
         SET state = 'expired', settled_at = ?1,
             next_try_at = NULL,
             last_error = coalesce(last_error, 'it grew too old to be news')
         WHERE state = 'pending' AND created_at <= ?2",
        params![now.get(), still_news_after(now).get()],
    )?;
    Ok(expired)
}

/// How long a settled delivery is kept, for `status`.
const KEEP_SETTLED_FOR_SECS: i64 = 7 * 24 * 3_600;

/// Forgets deliveries settled long ago. A pending row is never deleted here.
pub(super) fn forget_settled(conn: &Connection, now: Epoch) -> Result<usize, StoreError> {
    let removed = conn.execute(
        "DELETE FROM watch_deliveries
         WHERE state != 'pending' AND settled_at IS NOT NULL AND settled_at < ?1",
        params![(now - Duration::from_secs(KEEP_SETTLED_FOR_SECS as u64)).get()],
    )?;
    Ok(removed)
}

/// Reports that may be tried now, oldest first, as the events they are. One
/// past the age bound is not due; `store::watch::prune` settles it.
///
/// Only reports addressed to `destination` come back, compared as **the
/// whole address, not its origin**: n8n's `/webhook/name` and
/// `/webhook-test/name` differ in the path alone, and a `--webhook` typed to
/// look at a payload must not be sent another receiver's backlog.
pub fn due(
    conn: &Connection,
    now: Epoch,
    limit: usize,
    destination: &str,
) -> Result<Vec<Delivery>, StoreError> {
    owed_to(conn, now, limit, destination, false)
}

/// Every report still owed to `destination`, backoff ended or not: for `snob
/// watch deliver`. It is one more attempt, counted and aged like any other.
pub fn owed_now(
    conn: &Connection,
    now: Epoch,
    limit: usize,
    destination: &str,
) -> Result<Vec<Delivery>, StoreError> {
    owed_to(conn, now, limit, destination, true)
}

fn owed_to(
    conn: &Connection,
    now: Epoch,
    limit: usize,
    destination: &str,
    at_once: bool,
) -> Result<Vec<Delivery>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, run_id, body, attempts, event, content_type
         FROM watch_deliveries
         WHERE state = 'pending' AND next_try_at IS NOT NULL
           AND (?5 OR next_try_at <= ?1)
           AND created_at > ?3
           AND destination = ?4
         ORDER BY created_at, id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        params![
            now.get(),
            limit as i64,
            still_news_after(now).get(),
            destination,
            at_once,
        ],
        |row| {
            Ok(Delivery {
                id: row.get(0)?,
                run_id: row.get(1)?,
                body: row.get(2)?,
                attempts: row.get(3)?,
                form: match (row.get(4)?, row.get(5)?) {
                    (Some(event), Some(content_type)) => Some(Form {
                        event,
                        content_type,
                    }),
                    _ => None,
                },
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Records that a report arrived.
pub fn delivered(conn: &Connection, id: i64, status: u16, at: Epoch) -> Result<(), StoreError> {
    conn.execute(
        // `state = 'pending'`: a `watch deliver` and a run's drain can try one
        // row at once, and the second must not undo what the first settled.
        "UPDATE watch_deliveries
         SET state = 'delivered', settled_at = ?2, next_try_at = NULL,
             attempts = attempts + 1, last_status = ?3, last_error = NULL
         WHERE id = ?1 AND state = 'pending'",
        params![id, at.get(), status as i64],
    )?;
    Ok(())
}

/// What happened to a report that did not arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It will be tried again, at this moment.
    Retrying(Epoch),
    /// It will not. Either it was refused in a way waiting cannot fix, or it
    /// ran out of attempts, or it got too old to be news.
    GaveUp(GaveUp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaveUp {
    /// There was no request to send: a header that cannot be built from what
    /// was configured. Not "the far end said no" (see [`failed`]).
    Refused,
    OutOfAttempts,
    TooOld,
}

/// Records a failed attempt and decides what happens next.
///
/// `permanent` is the caller's reading, and narrow on purpose: a 401 or a 404
/// from the user's own server is often transient, and the mark has already
/// moved, so giving up on one would lose the change. [`MAX_ATTEMPTS`] and
/// [`MAX_AGE_SECS`] bound the retrying.
pub fn failed(
    conn: &Connection,
    id: i64,
    status: Option<u16>,
    error: &str,
    permanent: bool,
    now: Epoch,
) -> Result<Outcome, StoreError> {
    // **One transaction, begun by the write that counts the attempt**: a
    // `watch deliver` and a run's drain trying one row at once each read the
    // count the other left, never the same one twice. Inside a transaction
    // of the caller's, that one is it.
    let alone = if conn.is_autocommit() {
        Some(conn.unchecked_transaction()?)
    } else {
        None
    };
    let counted: Option<(i64, Epoch)> = conn
        .query_row(
            "UPDATE watch_deliveries
             SET attempts = attempts + 1, last_status = ?2, last_error = ?3
             WHERE id = ?1 AND state = 'pending'
             RETURNING attempts, created_at",
            params![id, status.map(i64::from), error],
            |row| Ok((row.get(0)?, Epoch::new(row.get(1)?))),
        )
        .optional()?;
    // Settled meanwhile by whoever else was trying it: nothing is written,
    // and the answer is what this attempt would have come to.
    let (attempts, created_at) = match counted {
        Some(counted) => counted,
        None => conn.query_row(
            "SELECT attempts + 1, created_at FROM watch_deliveries WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, Epoch::new(row.get(1)?))),
        )?,
    };

    let gave_up = if permanent {
        Some(GaveUp::Refused)
    } else if attempts >= MAX_ATTEMPTS {
        Some(GaveUp::OutOfAttempts)
    } else if created_at <= still_news_after(now) {
        Some(GaveUp::TooOld)
    } else {
        None
    };

    let outcome = match gave_up {
        Some(reason) => {
            conn.execute(
                "UPDATE watch_deliveries
                 SET state = 'expired', settled_at = ?2, next_try_at = NULL
                 WHERE id = ?1 AND state = 'pending'",
                params![id, now.get()],
            )?;
            Outcome::GaveUp(reason)
        }
        None => {
            let next = now + Duration::from_secs(backoff(attempts) as u64);
            conn.execute(
                "UPDATE watch_deliveries SET next_try_at = ?2
                 WHERE id = ?1 AND state = 'pending'",
                params![id, next.get()],
            )?;
            Outcome::Retrying(next)
        }
    };
    if let Some(alone) = alone {
        alone.commit()?;
    }
    Ok(outcome)
}

/// How long to wait before attempt number `attempts + 1`: doubling, capped.
fn backoff(attempts: i64) -> i64 {
    let shift = (attempts.max(1) - 1).min(16) as u32;
    FIRST_BACKOFF_SECS
        .checked_shl(shift)
        .unwrap_or(MAX_BACKOFF_SECS)
        .min(MAX_BACKOFF_SECS)
}

/// A count of the queue, split by whether this configuration could post it:
/// after `[webhook] url` moves, rows addressed to the old one are not owed
/// here, and `status` must not fail its verdict over them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Owed {
    /// Reports this destination could post: the ones addressed here.
    pub waiting: usize,
    /// Reports addressed elsewhere, which expire where they are.
    pub elsewhere: usize,
    /// Reports given up on, kept for `KEEP_SETTLED_FOR_SECS` so that `status`
    /// can say what became of them.
    pub given_up: usize,
}

/// What is queued, for `snob watch status`. `destination` is the address the
/// caller could post to, as [`enqueue`] recorded it; `None` can post nothing,
/// so nothing is owed to it. The age bound is not applied: `prune` expires a
/// row past it, and filtering here would print 0 over rows still pending.
pub fn owed(conn: &Connection, destination: Option<&str>) -> Result<Owed, StoreError> {
    // The `state` test is in the filters, not the `WHERE`: a report given up
    // on has left `pending` and still has to be counted.
    let (waiting, elsewhere, given_up): (i64, i64, i64) = conn.query_row(
        "SELECT
           count(*) FILTER (WHERE state = 'pending' AND destination = ?1),
           count(*) FILTER (WHERE state = 'pending'
             AND (?1 IS NULL OR destination <> ?1)),
           -- Every way a report is settled without arriving: too old, out of
           -- attempts, or refused before it was sent (`GaveUp`).
           count(*) FILTER (WHERE state = 'expired')
         FROM watch_deliveries",
        params![destination],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(Owed {
        waiting: waiting as usize,
        elsewhere: elsewhere as usize,
        given_up: given_up as usize,
    })
}

/// The whole queue, whoever it belongs to. For tests; people are told [`owed`].
pub fn pending(conn: &Connection) -> Result<usize, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM watch_deliveries WHERE state = 'pending'",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

/// The state of one report, for tests.
pub fn state(conn: &Connection, id: i64) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT state FROM watch_deliveries WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, accounts, users};

    const ME: Pk = Pk::new(42);
    /// The address these reports are addressed to.
    const HERE: &str = "https://receiver.example";

    fn store() -> Store {
        let db = Store::in_memory().unwrap();
        users::ensure(db.conn(), ME).unwrap();
        accounts::upsert(db.conn(), ME, true).unwrap();
        db
    }

    /// How old a report may get before it stops being news, written out.
    ///
    /// Every reference to it in the tree is relative — a fixture ages a row by
    /// `MAX_AGE_SECS + 1` and asserts it is no longer due — so the constant can
    /// be changed to anything at all and the suite still passes, while reports
    /// are either given up on within the hour or kept for a year.
    #[test]
    fn the_age_a_report_stops_being_news_at_is_the_documented_one() {
        assert_eq!(MAX_AGE_SECS, 7 * 24 * 3_600);
    }

    /// A `watch deliver` typed while a run drains the queue tries the same
    /// row from another process. Each attempt is one, however they fall:
    /// read and written back apart, two that read the same count wrote the
    /// same one, and the report was tried once more than it said.
    #[test]
    fn two_processes_failing_one_report_each_count_their_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("snob.db");
        let db = Store::open_at(&path).unwrap();
        users::ensure(db.conn(), ME).unwrap();
        accounts::upsert(db.conn(), ME, true).unwrap();
        let id = queued(&db, "run-1", Epoch::new(1_000));
        let each = 20;

        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    let other = Store::open_at(&path).unwrap();
                    for _ in 0..each {
                        failed(
                            other.conn(),
                            id,
                            Some(503),
                            "busy",
                            false,
                            Epoch::new(1_001),
                        )
                        .unwrap();
                    }
                });
            }
        });

        let attempts: i64 = db
            .conn()
            .query_row(
                "SELECT attempts FROM watch_deliveries WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 2 * each);
    }

    fn queued(db: &Store, run_id: &str, at: Epoch) -> i64 {
        enqueue(db.conn(), run_id, ME, r#"{"a":1}"#, at, HERE).unwrap()
    }

    /// The property the signature depends on. A retry sends the string that was
    /// signed, not a fresh rendering of the events.
    #[test]
    fn the_body_that_comes_back_is_the_body_that_went_in() {
        let db = store();
        let body = r#"{"schema":1,"events":{"followers_gained":[]}}"#;
        enqueue(db.conn(), "run-1", ME, body, Epoch::new(1_000), HERE).unwrap();

        let waiting = due(db.conn(), Epoch::new(1_000), 10, HERE).unwrap();
        assert_eq!(waiting[0].body, body);
    }

    /// A summary posted as plain text cannot say what it is, so the row does.
    #[test]
    fn a_report_queued_with_its_form_comes_back_with_it_and_an_old_one_without() {
        let db = store();
        let form = Form {
            event: "watch.changes".into(),
            content_type: "text/plain; charset=utf-8".into(),
        };
        enqueue_as(
            db.conn(),
            "run-1",
            ME,
            "Changes for @me",
            Epoch::new(1_000),
            HERE,
            Some(&form),
        )
        .unwrap();
        queued(&db, "run-2", Epoch::new(1_001));

        let waiting = due(db.conn(), Epoch::new(1_001), 10, HERE).unwrap();
        assert_eq!(waiting[0].form.as_ref(), Some(&form));
        assert_eq!(waiting[1].form, None, "queued as snob's JSON");
    }

    #[test]
    fn a_report_waiting_out_its_backoff_is_owed_now_but_not_due() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));
        failed(db.conn(), id, Some(500), "down", false, Epoch::new(1_000)).unwrap();
        assert!(
            due(db.conn(), Epoch::new(1_001), 10, HERE)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            owed_now(db.conn(), Epoch::new(1_001), 10, HERE)
                .unwrap()
                .len(),
            1
        );
        assert!(
            owed_now(
                db.conn(),
                Epoch::new(1_001),
                10,
                "https://elsewhere.example"
            )
            .unwrap()
            .is_empty(),
            "still only to the address it was made for"
        );
    }

    #[test]
    fn a_queued_report_is_due_at_once() {
        let db = store();
        queued(&db, "run-1", Epoch::new(1_000));
        assert_eq!(
            due(db.conn(), Epoch::new(1_000), 10, HERE).unwrap().len(),
            1
        );
        assert_eq!(pending(db.conn()).unwrap(), 1);
    }

    #[test]
    fn a_delivered_report_is_not_due_again() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));

        delivered(db.conn(), id, 200, Epoch::new(1_010)).unwrap();
        assert!(
            due(db.conn(), Epoch::new(2_000), 10, HERE)
                .unwrap()
                .is_empty()
        );
        assert_eq!(state(db.conn(), id).unwrap().unwrap(), "delivered");
        assert_eq!(pending(db.conn()).unwrap(), 0);
    }

    /// A failure that waiting can fix is waited on, and it is not tried again
    /// in the meantime.
    /// Two tries of one row at once, a `watch deliver` and a run's drain: the
    /// one that failed, finishing second, does not undo the one that
    /// arrived.
    #[test]
    fn a_failure_finishing_after_a_delivery_leaves_it_delivered() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));
        delivered(db.conn(), id, 200, Epoch::new(1_001)).unwrap();
        failed(db.conn(), id, Some(500), "down", false, Epoch::new(1_002)).unwrap();
        assert_eq!(state(db.conn(), id).unwrap().as_deref(), Some("delivered"));
        failed(db.conn(), id, None, "no request", true, Epoch::new(1_003)).unwrap();
        assert_eq!(state(db.conn(), id).unwrap().as_deref(), Some("delivered"));
    }

    #[test]
    fn a_temporary_failure_is_retried_later_and_not_sooner() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));

        let Outcome::Retrying(next) =
            failed(db.conn(), id, Some(503), "busy", false, Epoch::new(1_000)).unwrap()
        else {
            panic!("a 503 is worth another try");
        };
        assert_eq!(next, Epoch::new(1_000 + FIRST_BACKOFF_SECS));
        assert!(
            due(db.conn(), next - Duration::from_secs(1), 10, HERE)
                .unwrap()
                .is_empty()
        );
        assert_eq!(due(db.conn(), next, 10, HERE).unwrap().len(), 1);
    }

    /// A request that could not be sent at all is not sent again: "there is
    /// no request to retry" — a header the builder cannot construct — is the
    /// only failure retrying genuinely cannot change, and `failed` says why a
    /// 4xx is not one.
    #[test]
    fn a_request_that_cannot_be_sent_is_not_retried_at_all() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));

        assert_eq!(
            // `None`, not `Some(0)`: `Attempt::Refused` carries no status
            // because no server answered, and 0 is not a code one can send.
            failed(db.conn(), id, None, "not a header", true, Epoch::new(1_000)).unwrap(),
            Outcome::GaveUp(GaveUp::Refused)
        );
        assert!(
            due(db.conn(), Epoch::new(999_999), 10, HERE)
                .unwrap()
                .is_empty()
        );
        assert_eq!(state(db.conn(), id).unwrap().unwrap(), "expired");
    }

    /// The wait grows, and it stops growing. Without the cap, the eighth
    /// attempt would be hours out and the queue would look stuck.
    #[test]
    fn the_wait_grows_and_then_stops_growing() {
        let waits: Vec<i64> = (1..=6).map(backoff).collect();
        assert_eq!(waits, vec![60, 120, 240, 480, 960, 1_920]);
        assert_eq!(backoff(20), MAX_BACKOFF_SECS, "and it is capped");
        assert!(backoff(i64::MAX) > 0, "a shift that big must not wrap");
    }

    /// A receiver that is down for an afternoon does not cost the reports queued
    /// in it.
    ///
    /// It measures the ladder in time rather than counting iterations, which
    /// would pass at any length; `MAX_ATTEMPTS` says why the age has to win.
    #[test]
    fn a_receiver_down_for_hours_still_has_its_report_retried() {
        let db = store();
        let queued_at = Epoch::new(1_000);
        let id = queued(&db, "run-1", queued_at);

        let mut now = queued_at;
        loop {
            match failed(db.conn(), id, Some(503), "restarting", false, now).unwrap() {
                Outcome::Retrying(next) => now = next,
                Outcome::GaveUp(reason) => {
                    assert_eq!(
                        reason,
                        GaveUp::TooOld,
                        "the age is what decides in ordinary running"
                    );
                    break;
                }
            }
        }

        assert!(
            now - queued_at >= MAX_AGE_SECS,
            "it gave up after {}s, and a week is what the age bound names",
            now - queued_at
        );
        assert!(
            due(db.conn(), now + Duration::from_secs(999_999), 10, HERE)
                .unwrap()
                .is_empty()
        );
    }

    /// And the attempt count is still there, for a queue that is somehow tried
    /// without the clock moving. A queue that never drains is a queue that grows
    /// forever.
    #[test]
    fn the_attempt_count_is_the_backstop_when_the_clock_does_not_move() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));

        for _ in 1..MAX_ATTEMPTS {
            assert!(
                matches!(
                    failed(db.conn(), id, Some(500), "boom", false, Epoch::new(1_000)).unwrap(),
                    Outcome::Retrying(_)
                ),
                "not out of attempts yet"
            );
        }

        assert_eq!(
            failed(db.conn(), id, Some(500), "boom", false, Epoch::new(1_000)).unwrap(),
            Outcome::GaveUp(GaveUp::OutOfAttempts)
        );
    }

    /// The other bound, and it catches a different failure: a process stopped
    /// for a week has attempted nothing, and news about last Tuesday is not
    /// news.
    #[test]
    fn a_report_too_old_to_be_news_is_given_up_on() {
        let db = store();
        let id = queued(&db, "run-1", Epoch::new(1_000));

        assert_eq!(
            failed(
                db.conn(),
                id,
                None,
                "connection refused",
                false,
                Epoch::new(1_000 + MAX_AGE_SECS)
            )
            .unwrap(),
            Outcome::GaveUp(GaveUp::TooOld)
        );
    }

    /// They are a sequence of events about one account, so they go out in the
    /// order they happened.
    #[test]
    fn reports_come_out_oldest_first() {
        let db = store();
        queued(&db, "later", Epoch::new(2_000));
        queued(&db, "earlier", Epoch::new(1_000));

        let order: Vec<String> = due(db.conn(), Epoch::new(3_000), 10, HERE)
            .unwrap()
            .into_iter()
            .map(|d| d.run_id)
            .collect();
        assert_eq!(order, vec!["earlier", "later"]);
    }

    /// A report belongs to the address it was addressed to; `due` says why.
    #[test]
    fn a_report_is_only_due_at_the_address_it_was_addressed_to() {
        let db = store();
        let mine = enqueue(db.conn(), "run-1", ME, "{}", Epoch::new(1_000), HERE).unwrap();
        let elsewhere = enqueue(
            db.conn(),
            "run-2",
            ME,
            "{}",
            Epoch::new(1_000),
            "https://bin.example",
        )
        .unwrap();
        let here: Vec<i64> = due(db.conn(), Epoch::new(1_000), 10, HERE)
            .unwrap()
            .iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(here, vec![mine]);

        let there: Vec<i64> = due(db.conn(), Epoch::new(1_000), 10, "https://bin.example")
            .unwrap()
            .iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(there, vec![elsewhere]);
    }

    /// Two workflows on one host are two addresses.
    ///
    /// The test above uses two different *hosts*, which an origin filter
    /// would pass too: an origin tells `bin.example` from `receiver.example`
    /// and cannot tell `/webhook/snob` from `/webhook-test/snob`, which is how
    /// n8n publishes a workflow and its test run.
    #[test]
    fn a_report_is_not_drained_to_a_different_path_on_the_same_host() {
        const PUBLISHED: &str = "https://n8n.local/webhook/snob";
        const TEST_RUN: &str = "https://n8n.local/webhook-test/snob";

        let db = store();
        let owed = enqueue(db.conn(), "run-1", ME, "{}", Epoch::new(1_000), PUBLISHED).unwrap();

        let to_the_test_run: Vec<i64> = due(db.conn(), Epoch::new(1_000), 10, TEST_RUN)
            .unwrap()
            .iter()
            .map(|d| d.id)
            .collect();
        assert!(
            to_the_test_run.is_empty(),
            "a report addressed to {PUBLISHED} was drained to {TEST_RUN}"
        );

        let to_where_it_belongs: Vec<i64> = due(db.conn(), Epoch::new(1_000), 10, PUBLISHED)
            .unwrap()
            .iter()
            .map(|d| d.id)
            .collect();
        assert_eq!(
            to_where_it_belongs,
            vec![owed],
            "and it is still owed at the address it was addressed to"
        );
    }

    /// What a person is told is owed is what a run could actually post.
    ///
    /// `Owed` says why `pending` is not that number.
    #[test]
    fn what_is_owed_is_split_from_what_is_addressed_elsewhere() {
        let db = store();
        enqueue(db.conn(), "here", ME, "{}", Epoch::new(1_000), HERE).unwrap();
        enqueue(
            db.conn(),
            "there",
            ME,
            "{}",
            Epoch::new(1_000),
            "https://bin.example",
        )
        .unwrap();

        let here = owed(db.conn(), Some(HERE)).unwrap();
        assert_eq!(
            here,
            Owed {
                waiting: 1,
                elsewhere: 1,
                given_up: 0
            },
            "the row for {HERE} is owed here; the other is not"
        );
        assert_eq!(
            due(db.conn(), Epoch::new(1_000), 10, HERE).unwrap().len(),
            here.waiting,
            "what is reported as owed has to be what a run would be handed"
        );

        // A configuration naming no address can post nothing at all, so nothing
        // is owed to it -- and it cannot tell a row queued by a `--webhook` run
        // from one whose `[webhook]` was deleted, which is why that is a
        // separate number rather than a verdict.
        assert_eq!(
            owed(db.conn(), None).unwrap(),
            Owed {
                waiting: 0,
                elsewhere: 2,
                given_up: 0
            }
        );

        // However it is split, nothing falls out of the total.
        assert_eq!(here.waiting + here.elsewhere, pending(db.conn()).unwrap());
    }

    /// A report nothing will ever deliver has to be visible for as long as the
    /// row survives.
    ///
    /// `expire_stale` marks rather than deletes for this, and `owed` is what
    /// reads the mark.
    #[test]
    fn a_report_given_up_on_is_still_counted_while_the_row_is_kept() {
        let db = store();
        enqueue(db.conn(), "old", ME, "{}", Epoch::new(1_000), HERE).unwrap();

        let before = owed(db.conn(), Some(HERE)).unwrap();
        assert_eq!(before.waiting, 1);
        assert_eq!(before.given_up, 0);

        // Far enough past the age bound that it is no longer news.
        let much_later = Epoch::new(1_000 + MAX_AGE_SECS + 1);
        assert_eq!(expire_stale(db.conn(), much_later).unwrap(), 1);

        let after = owed(db.conn(), Some(HERE)).unwrap();
        assert_eq!(after.waiting, 0, "it is not owed any more");
        assert_eq!(
            after.given_up, 1,
            "and that is exactly when somebody has to be told"
        );
    }

    /// A day old is one answer, not three: `due`, `failed` and `prune` agree
    /// about a report at exactly `MAX_AGE_SECS`.
    #[test]
    fn a_report_exactly_a_day_old_is_the_same_answer_to_everyone() {
        let queued_at = Epoch::new(1_000);
        let now = queued_at + Duration::from_secs(MAX_AGE_SECS as u64);

        let db = store();
        let id = queued(&db, "run-1", queued_at);
        assert!(
            due(db.conn(), now, 10, HERE).unwrap().is_empty(),
            "a report the age bound has caught must not go out as news"
        );
        assert_eq!(
            failed(db.conn(), id, None, "connection refused", false, now).unwrap(),
            Outcome::GaveUp(GaveUp::TooOld),
            "and the run that tries it gives up on it"
        );

        // And the sweep that runs whether or not anything tried it agrees, which
        // is the reading that was the odd one out.
        let db = store();
        let swept = queued(&db, "run-2", queued_at);
        expire_stale(db.conn(), now).unwrap();
        assert_eq!(
            state(db.conn(), swept).unwrap().as_deref(),
            Some("expired"),
            "a report nothing ever retried is expired at the same age"
        );
    }

    /// And how long the record of a delivered one is kept, for the same reason
    /// the age bound is written down: `prune`'s fixture ages a row by a multiple
    /// of this, so it agreed with anything.
    #[test]
    fn how_long_a_settled_report_is_kept_is_the_documented_one() {
        assert_eq!(KEEP_SETTLED_FOR_SECS, 7 * 24 * 3_600);
    }

    /// The id the receiver deduplicates on has to be unique, or two runs could
    /// hand it the same one and it would drop a real report.
    #[test]
    fn two_reports_cannot_share_a_run_id() {
        let db = store();
        queued(&db, "run-1", Epoch::new(1_000));
        assert!(
            enqueue(db.conn(), "run-1", ME, "{}", Epoch::new(2_000), HERE).is_err(),
            "the id a receiver deduplicates on must be unique"
        );
    }
}
