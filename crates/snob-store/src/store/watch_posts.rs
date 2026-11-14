//! What the monitor has seen of a watched account's posts, and the receipt
//! that says a baseline exists and how far it reaches.
//!
//! **Both are written only by `watch::commit_report`**, in the transaction
//! that queues the report naming the posts, so a process killed between the
//! two reports a post again rather than never. A media pk is a `u64` that is
//! not a [`Pk`], stored as its decimal digits (`018_watch_posts.sql`).

use std::collections::HashSet;
use std::time::Duration;

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use super::{StoreError, pk_from_sql, pk_to_sql};
use snob_core::{Epoch, Pk};

/// A post as a run read it off the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub media_pk: u64,
    pub code: String,
    pub taken_at: Epoch,
    /// Pinned to the top of the grid. Not stored: it only decides which known
    /// posts a read may stop at, and a pin moves.
    pub pinned: bool,
}

/// The receipt for one account's posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub account_pk: Pk,
    /// When the baseline was laid: the first read that concluded.
    pub baseline_at: Epoch,
    /// The last read that concluded anything.
    pub read_at: Epoch,
    /// The last report that named a post, if one ever did.
    pub reported_at: Option<Epoch>,
    /// The floor: the newest `taken_at` seen of this account, 0 with no post.
    pub newest_taken_at: Epoch,
}

/// What one run writes about an account's posts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commit {
    /// Every post the run read. Inserted when not already there; one already
    /// there keeps when it was first seen.
    pub seen: Vec<Seen>,
    /// The posts the report names as new.
    pub news: Vec<u64>,
    /// The posts whose every file is now in the directory the entry saves to.
    pub saved: Vec<u64>,
    /// The floor after this run.
    pub floor: Epoch,
    /// Whether the entry keeps files. Without, a post that is news owes
    /// nothing and is written as saved.
    pub has_files: bool,
}

/// A media pk as the column holds it.
fn media_pk_to_sql(pk: u64) -> String {
    pk.to_string()
}

/// A media pk back out of the column. The column's CHECK only lets digits in,
/// so a failure here is a number past `u64`, which this build did not write.
fn media_pk_from_sql(column: usize, value: &str) -> rusqlite::Result<u64> {
    value
        .parse()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(e)))
}

/// The receipt for this account's posts, if a baseline was ever laid.
pub fn mark(conn: &Connection, account_pk: Pk) -> Result<Option<Mark>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT account_pk, baseline_at, read_at, reported_at, newest_taken_at
             FROM watch_post_marks WHERE account_pk = ?1",
            params![pk_to_sql(account_pk)],
            mark_from_row,
        )
        .optional()?)
}

/// Every receipt, the most recently read first. What `watch status` reads.
pub fn all_marks(conn: &Connection) -> Result<Vec<Mark>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT account_pk, baseline_at, read_at, reported_at, newest_taken_at
         FROM watch_post_marks ORDER BY read_at DESC, account_pk",
    )?;
    let rows = stmt.query_map([], mark_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn mark_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mark> {
    Ok(Mark {
        account_pk: pk_from_sql(row.get(0)?),
        baseline_at: Epoch::new(row.get(1)?),
        read_at: Epoch::new(row.get(2)?),
        reported_at: row.get::<_, Option<i64>>(3)?.map(Epoch::new),
        newest_taken_at: Epoch::new(row.get(4)?),
    })
}

/// Which of `media_pks` this account's rows already hold.
pub fn known(
    conn: &Connection,
    account_pk: Pk,
    media_pks: &[u64],
) -> Result<HashSet<u64>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT 1 FROM watch_posts WHERE account_pk = ?1 AND media_pk = ?2")?;
    let mut found = HashSet::new();
    for &pk in media_pks {
        if stmt.exists(params![pk_to_sql(account_pk), media_pk_to_sql(pk)])? {
            found.insert(pk);
        }
    }
    Ok(found)
}

/// The `limit` most recently taken posts seen of this account, newest first.
///
/// What a read may stop at. Whether one is pinned is asked by the caller of
/// the grid it holds now, since a pin is not stored.
pub fn newest(conn: &Connection, account_pk: Pk, limit: usize) -> Result<Vec<u64>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT media_pk FROM watch_posts WHERE account_pk = ?1
         ORDER BY taken_at DESC, media_pk DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        params![
            pk_to_sql(account_pk),
            i64::try_from(limit).unwrap_or(i64::MAX)
        ],
        |row| media_pk_from_sql(0, &row.get::<_, String>(0)?),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The posts that were news and still owe a file, with their codes, newest
/// first.
pub fn pending_saves(conn: &Connection, account_pk: Pk) -> Result<Vec<(u64, String)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT media_pk, code FROM watch_posts
         WHERE account_pk = ?1 AND reported_at IS NOT NULL AND saved_at IS NULL
         ORDER BY taken_at DESC, media_pk DESC",
    )?;
    let rows = stmt.query_map(params![pk_to_sql(account_pk)], |row| {
        Ok((
            media_pk_from_sql(0, &row.get::<_, String>(0)?)?,
            row.get(1)?,
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Writes what one run concluded about an account's posts. Called **only**
/// from `watch::commit_report`, inside its transaction.
///
/// The first commit of an account lays its baseline: the mark is made with
/// `baseline_at`, and nothing it saw is news (the caller names no news then).
pub(super) fn write(
    tx: &Transaction<'_>,
    account_pk: Pk,
    commit: &Commit,
    at: Epoch,
) -> Result<(), StoreError> {
    let account = pk_to_sql(account_pk);
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO watch_posts (account_pk, media_pk, code, taken_at, first_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_pk, media_pk) DO NOTHING",
        )?;
        for seen in &commit.seen {
            insert.execute(params![
                account,
                media_pk_to_sql(seen.media_pk),
                seen.code,
                seen.taken_at.get(),
                at.get()
            ])?;
        }

        // A post is reported once: a row already reported keeps its moment.
        // Without files to keep, nothing is owed, so it is saved as it is
        // reported.
        let mut news = tx.prepare_cached(
            "UPDATE watch_posts
             SET reported_at = ?3,
                 saved_at = CASE WHEN ?4 THEN saved_at ELSE ?3 END
             WHERE account_pk = ?1 AND media_pk = ?2 AND reported_at IS NULL",
        )?;
        for &pk in &commit.news {
            news.execute(params![
                account,
                media_pk_to_sql(pk),
                at.get(),
                commit.has_files
            ])?;
        }

        let mut saved = tx.prepare_cached(
            "UPDATE watch_posts SET saved_at = ?3
             WHERE account_pk = ?1 AND media_pk = ?2 AND saved_at IS NULL",
        )?;
        for &pk in &commit.saved {
            saved.execute(params![account, media_pk_to_sql(pk), at.get()])?;
        }
    }

    let reported = (!commit.news.is_empty()).then_some(at.get());
    tx.execute(
        "INSERT INTO watch_post_marks
             (account_pk, baseline_at, read_at, reported_at, newest_taken_at)
         VALUES (?1, ?2, ?2, ?3, ?4)
         ON CONFLICT(account_pk) DO UPDATE SET
             read_at         = excluded.read_at,
             reported_at     = coalesce(excluded.reported_at, watch_post_marks.reported_at),
             newest_taken_at = max(watch_post_marks.newest_taken_at, excluded.newest_taken_at)",
        params![account, at.get(), reported, commit.floor.get()],
    )?;
    Ok(())
}

/// How many of an account's newest posts are kept whatever their age: three
/// pages of the web app's twelve, the most one run reads (`posts::UNTIL_PAGES`
/// in `snob-cli`), so a read never meets a post it has forgotten and takes it
/// for news.
const NEWEST_KEPT: i64 = 3 * 12;

/// Forgets posts seen longer ago than the captures are kept, all but each
/// account's newest [`NEWEST_KEPT`]. The marks are never swept: a mark is
/// what says a baseline exists.
pub(super) fn prune(conn: &Connection, now: Epoch) -> Result<usize, StoreError> {
    let cutoff = now - Duration::from_secs(super::watch::KEEP_FOR_SECS as u64);
    Ok(conn.execute(
        "DELETE FROM watch_posts
         WHERE first_seen_at < ?1
           AND (account_pk, media_pk) NOT IN (
             SELECT account_pk, media_pk FROM (
               SELECT account_pk, media_pk, row_number() OVER (
                 PARTITION BY account_pk ORDER BY taken_at DESC, media_pk DESC
               ) AS rank
               FROM watch_posts
             ) WHERE rank <= ?2
           )",
        params![cutoff.get(), NEWEST_KEPT],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, accounts, users};

    const ACCOUNT: Pk = Pk::new(7);

    fn store() -> Store {
        let db = Store::in_memory().unwrap();
        users::ensure(db.conn(), ACCOUNT).unwrap();
        accounts::upsert(db.conn(), ACCOUNT, false).unwrap();
        db
    }

    fn seen(pk: u64, taken_at: i64) -> Seen {
        Seen {
            media_pk: pk,
            code: format!("C{pk}"),
            taken_at: Epoch::new(taken_at),
            pinned: false,
        }
    }

    fn commit(db: &mut Store, commit: &Commit, at: i64) {
        let tx = db.conn_mut().transaction().unwrap();
        write(&tx, ACCOUNT, commit, Epoch::new(at)).unwrap();
        tx.commit().unwrap();
    }

    fn first_seen(db: &Store, pk: u64) -> i64 {
        db.conn()
            .query_row(
                "SELECT first_seen_at FROM watch_posts WHERE media_pk = ?1",
                [pk.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn a_media_pk_past_i64_reads_back_as_it_was_written() {
        let mut db = store();
        let big = u64::MAX - 5;
        commit(
            &mut db,
            &Commit {
                seen: vec![seen(big, 100)],
                floor: Epoch::new(100),
                ..Commit::default()
            },
            1_000,
        );
        assert_eq!(newest(db.conn(), ACCOUNT, 12).unwrap(), vec![big]);
        assert_eq!(
            known(db.conn(), ACCOUNT, &[big, 3]).unwrap(),
            HashSet::from([big])
        );
        assert_eq!(media_pk_from_sql(0, &media_pk_to_sql(big)).unwrap(), big);
    }

    #[test]
    fn a_post_seen_twice_is_one_row_and_keeps_when_it_was_first_seen() {
        let mut db = store();
        let once = Commit {
            seen: vec![seen(1, 100)],
            floor: Epoch::new(100),
            ..Commit::default()
        };
        commit(&mut db, &once, 1_000);
        commit(&mut db, &once, 2_000);
        let rows: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM watch_posts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        assert_eq!(first_seen(&db, 1), 1_000);
        assert_eq!(
            mark(db.conn(), ACCOUNT).unwrap().unwrap().read_at,
            Epoch::new(2_000),
            "the mark moves with every read"
        );
    }

    #[test]
    fn a_baseline_marks_nothing_as_reported() {
        let mut db = store();
        commit(
            &mut db,
            &Commit {
                seen: vec![seen(1, 100), seen(2, 90)],
                floor: Epoch::new(100),
                has_files: true,
                ..Commit::default()
            },
            1_000,
        );
        let mark = mark(db.conn(), ACCOUNT).unwrap().unwrap();
        assert_eq!(mark.baseline_at, Epoch::new(1_000));
        assert_eq!(mark.reported_at, None);
        assert_eq!(mark.newest_taken_at, Epoch::new(100));
        assert!(
            pending_saves(db.conn(), ACCOUNT).unwrap().is_empty(),
            "nothing of a baseline is owed a file"
        );
    }

    #[test]
    fn an_account_with_no_posts_still_has_a_baseline() {
        let mut db = store();
        commit(&mut db, &Commit::default(), 1_000);
        let mark = mark(db.conn(), ACCOUNT).unwrap().expect("a baseline");
        assert_eq!(mark.newest_taken_at, Epoch::new(0));
        assert_eq!(all_marks(db.conn()).unwrap(), vec![mark]);
    }

    #[test]
    fn news_is_reported_once_and_owes_its_files_until_saved() {
        let mut db = store();
        commit(&mut db, &Commit::default(), 1_000);
        let news = Commit {
            seen: vec![seen(5, 200), seen(6, 210)],
            news: vec![5, 6],
            saved: vec![6],
            floor: Epoch::new(210),
            has_files: true,
        };
        commit(&mut db, &news, 2_000);
        assert_eq!(
            pending_saves(db.conn(), ACCOUNT).unwrap(),
            vec![(5, "C5".to_string())]
        );
        assert_eq!(
            mark(db.conn(), ACCOUNT).unwrap().unwrap().reported_at,
            Some(Epoch::new(2_000))
        );

        // A later run saves it, and names nothing new: the mark keeps when a
        // post was last reported, and the floor never moves back.
        commit(
            &mut db,
            &Commit {
                saved: vec![5],
                floor: Epoch::new(50),
                has_files: true,
                ..Commit::default()
            },
            3_000,
        );
        assert!(pending_saves(db.conn(), ACCOUNT).unwrap().is_empty());
        let mark = mark(db.conn(), ACCOUNT).unwrap().unwrap();
        assert_eq!(mark.reported_at, Some(Epoch::new(2_000)));
        assert_eq!(mark.newest_taken_at, Epoch::new(210));
    }

    #[test]
    fn news_on_an_entry_that_keeps_no_files_owes_none() {
        let mut db = store();
        commit(&mut db, &Commit::default(), 1_000);
        commit(
            &mut db,
            &Commit {
                seen: vec![seen(5, 200)],
                news: vec![5],
                floor: Epoch::new(200),
                ..Commit::default()
            },
            2_000,
        );
        assert!(pending_saves(db.conn(), ACCOUNT).unwrap().is_empty());
    }

    #[test]
    fn the_newest_three_pages_survive_the_sweep_however_old() {
        let mut db = store();
        let posts: Vec<Seen> = (1..=40).map(|n| seen(n, 100 + n as i64)).collect();
        commit(
            &mut db,
            &Commit {
                seen: posts,
                floor: Epoch::new(140),
                ..Commit::default()
            },
            1_000,
        );
        let long_after = Epoch::new(1_000 + super::super::watch::KEEP_FOR_SECS + 1);
        assert_eq!(prune(db.conn(), long_after).unwrap(), 4);
        let kept = newest(db.conn(), ACCOUNT, 100).unwrap();
        assert_eq!(kept.len(), 36);
        assert_eq!(kept.first(), Some(&40));
        assert_eq!(kept.last(), Some(&5));
        assert!(
            mark(db.conn(), ACCOUNT).unwrap().is_some(),
            "the mark stays"
        );

        // And nothing seen within the window goes.
        assert_eq!(prune(db.conn(), Epoch::new(2_000)).unwrap(), 0);
    }

    #[test]
    fn deleting_the_account_takes_its_posts_with_it() {
        let mut db = store();
        commit(
            &mut db,
            &Commit {
                seen: vec![seen(1, 100)],
                floor: Epoch::new(100),
                ..Commit::default()
            },
            1_000,
        );
        db.conn()
            .execute("DELETE FROM accounts WHERE pk = ?1", [7])
            .unwrap();
        assert!(newest(db.conn(), ACCOUNT, 12).unwrap().is_empty());
        assert_eq!(mark(db.conn(), ACCOUNT).unwrap(), None);
    }

    #[test]
    fn a_media_pk_that_is_not_digits_is_refused_by_the_table() {
        let db = store();
        let refused = db.conn().execute(
            "INSERT INTO watch_posts (account_pk, media_pk, code, taken_at, first_seen_at)
             VALUES (7, '12a', 'x', 1, 1)",
            [],
        );
        assert!(refused.is_err());
    }
}
