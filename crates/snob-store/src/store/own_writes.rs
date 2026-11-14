//! The follows and unfollows the account made through snob, so that a stored
//! following list can be handed out with them applied.
//!
//! **Applied and said, never passed off as a walk** (`engine::serve_stored`):
//! only to a stored list of the account's own following, only the writes
//! newer than its walk, and the run says how many it applied. One row per
//! write Instagram took that changed the relationship: a follow that became
//! a request, or one already so, changed no list.

use rusqlite::{Connection, OptionalExtension, params};

use super::{StoreError, pk_from_sql, pk_to_sql, users};
use snob_core::model::User;
use snob_core::{Epoch, Pk};

/// How long a row is kept. By then a newer walk has read the list, and a
/// list that old is not served without a warning about its age anyway.
const KEPT_SECS: i64 = 30 * 86_400;

/// Which of the two writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Follow,
    Unfollow,
}

impl Verb {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Unfollow => "unfollow",
        }
    }
}

/// A change applied to a stored list: what was done, and to whom, by the
/// name the store has for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub verb: Verb,
    pub pk: Pk,
    pub username: Option<String>,
}

/// Records a write Instagram took, at `at`, and forgets the rows past
/// their keeping. In a transaction of the caller's, or on its own.
pub fn record(conn: &Connection, target: Pk, verb: Verb, at: Epoch) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO own_writes (at, target_pk, verb) VALUES (?1, ?2, ?3)",
        params![at.get(), pk_to_sql(target), verb.as_str()],
    )?;
    conn.execute(
        "DELETE FROM own_writes WHERE at < ?1",
        params![at.get() - KEPT_SECS],
    )?;
    Ok(())
}

/// The last write to each account made after `after`, oldest first: a
/// follow and then an unfollow of one account is the unfollow.
fn since(conn: &Connection, after: Epoch) -> Result<Vec<(Pk, Verb)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT target_pk, verb FROM own_writes w
         WHERE at > ?1
           AND id = (SELECT max(id) FROM own_writes v
                     WHERE v.target_pk = w.target_pk AND v.at > ?1)
         ORDER BY at, id",
    )?;
    let rows = stmt.query_map(params![after.get()], |row| {
        let verb: String = row.get(1)?;
        Ok((
            pk_from_sql(row.get(0)?),
            if verb == "follow" {
                Verb::Follow
            } else {
                Verb::Unfollow
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// What the account's stored following says of one account ([`followed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Followed {
    /// No whole capture of the account's own following is recent enough to
    /// say.
    NoCapture,
    /// The capture taken at `taken_at`, with the writes made since, does not
    /// hold it.
    No { taken_at: Epoch },
    /// It holds it.
    Yes { user: User, taken_at: Epoch },
}

/// Whether `viewer`'s own following holds `target`: by its newest whole
/// capture, when that was taken at or after `not_before`, with the writes
/// made since applied as [`apply`] applies them.
///
/// **A seek for the capture and one for the member.** Reading the list to
/// look for one account read a row for every account followed.
pub fn followed(
    conn: &Connection,
    viewer: Pk,
    target: Pk,
    not_before: Epoch,
) -> Result<Followed, StoreError> {
    let newest: Option<(i64, bool)> = conn
        .query_row(
            HELD_IN_NEWEST,
            params![pk_to_sql(viewer), pk_to_sql(target)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((taken_at, in_the_capture)) =
        newest.filter(|(taken_at, _)| *taken_at >= not_before.get())
    else {
        return Ok(Followed::NoCapture);
    };
    let taken_at = Epoch::new(taken_at);

    let written: Option<String> = conn
        .query_row(
            LAST_WRITE_TO,
            params![pk_to_sql(target), taken_at.get()],
            |row| row.get(0),
        )
        .optional()?;
    let held = match written.as_deref() {
        Some("follow") => true,
        Some(_) => false,
        None => in_the_capture,
    };
    // A followed account the store has no row for is left out, as `apply`
    // leaves it out.
    if held && let Some(user) = users::find(conn, target)? {
        return Ok(Followed::Yes { user, taken_at });
    }
    Ok(Followed::No { taken_at })
}

/// `?1` the account whose following it is, `?2` the account asked about.
/// The newest whole capture is sought in `snapshots_lookup` and the member in
/// the primary key of `snapshot_members`.
const HELD_IN_NEWEST: &str = "SELECT s.taken_at, EXISTS (
           SELECT 1 FROM snapshot_members m
           WHERE m.snapshot_id = s.id AND m.user_pk = ?2)
         FROM usable_snapshots s
         WHERE s.account_pk = ?1 AND s.kind = 'following'
         ORDER BY s.taken_at DESC, s.id DESC LIMIT 1";

/// The last write to `?1` made after `?2`, as [`since`] picks it. Over a
/// table of a month of the account's own writes, a few hundred rows at most.
const LAST_WRITE_TO: &str = "SELECT verb FROM own_writes
         WHERE target_pk = ?1 AND at > ?2 ORDER BY id DESC LIMIT 1";

/// Applies to `members`, the account's own following as a walk that ended
/// at `taken_at` read it, the writes made since, and says which changed it:
/// an unfollowed account taken out, a followed one put first, where
/// Instagram lists the newest. A write the list already shows changes
/// nothing and is not said; a followed account the store has no row for is
/// left out, having no name to show.
pub fn apply(
    conn: &Connection,
    members: &mut Vec<User>,
    taken_at: Epoch,
) -> Result<Vec<Applied>, StoreError> {
    let mut applied = Vec::new();
    for (pk, verb) in since(conn, taken_at)? {
        let at = members.iter().position(|user| user.pk == pk);
        match (verb, at) {
            (Verb::Unfollow, Some(at)) => {
                let user = members.remove(at);
                applied.push(Applied {
                    verb,
                    pk,
                    username: Some(user.username),
                });
            }
            (Verb::Follow, None) => {
                if let Some(user) = users::find(conn, pk)? {
                    applied.push(Applied {
                        verb,
                        pk,
                        username: Some(user.username.clone()),
                    });
                    members.insert(0, user);
                }
            }
            _ => {}
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    fn user(pk: u64, name: &str) -> User {
        User::named(Pk::new(pk), name)
    }

    const ME: Pk = Pk::new(7);

    /// A whole capture of `ME`'s following holding `members`, taken now.
    fn following_of_mine(db: &mut Store, members: &[User]) -> Epoch {
        use crate::store::{accounts, snapshots};
        use snob_core::model::{ListKind, StopReason};

        users::ensure(db.conn(), ME).unwrap();
        accounts::upsert(db.conn(), ME, true).unwrap();
        let opened = snapshots::begin(db.conn(), ME, ListKind::Following, None).unwrap();
        snapshots::save_page(db, opened.id, members, None).unwrap();
        snapshots::close(db.conn(), opened.id, StopReason::Completed).unwrap();
        snapshots::find_usable(db.conn(), opened.id)
            .unwrap()
            .unwrap()
            .taken_at
            .unwrap()
    }

    #[test]
    fn one_account_is_asked_about_without_reading_the_list() {
        let mut db = Store::in_memory().unwrap();
        let hour = std::time::Duration::from_secs(3_600);
        assert_eq!(
            followed(db.conn(), ME, Pk::new(1), Epoch::new(0)).unwrap(),
            Followed::NoCapture
        );

        users::upsert(db.conn(), &user(3, "cy")).unwrap();
        let taken_at = following_of_mine(&mut db, &[user(1, "ana"), user(2, "bo")]);
        let asked = |db: &Store, pk: u64, not_before: Epoch| {
            followed(db.conn(), ME, Pk::new(pk), not_before).unwrap()
        };
        let recent = taken_at - hour;

        assert_eq!(
            asked(&db, 1, recent),
            Followed::Yes {
                user: user(1, "ana"),
                taken_at
            }
        );
        assert_eq!(asked(&db, 3, recent), Followed::No { taken_at });
        assert_eq!(asked(&db, 1, taken_at + hour), Followed::NoCapture);

        // The account's own writes since the walk, as `apply` applies them.
        record(db.conn(), Pk::new(1), Verb::Unfollow, taken_at + hour).unwrap();
        record(db.conn(), Pk::new(3), Verb::Follow, taken_at + hour).unwrap();
        assert_eq!(asked(&db, 1, recent), Followed::No { taken_at });
        assert!(matches!(asked(&db, 3, recent), Followed::Yes { .. }));
        assert_eq!(asked(&db, 2, recent).clone(), {
            let mut members = vec![user(1, "ana"), user(2, "bo")];
            apply(db.conn(), &mut members, taken_at).unwrap();
            let bo = members.into_iter().find(|u| u.pk == Pk::new(2)).unwrap();
            Followed::Yes { user: bo, taken_at }
        });
    }

    #[test]
    fn the_capture_and_the_member_are_sought_not_scanned_for() {
        let db = Store::in_memory().unwrap();
        let plan = crate::store::plan(db.conn(), HELD_IN_NEWEST);
        assert!(plan.contains("snapshots_lookup"), "{plan}");
        assert!(!plan.contains("SCAN"), "{plan}");
    }

    #[test]
    fn writes_after_the_walk_are_applied_and_earlier_ones_are_not() {
        let db = Store::in_memory().unwrap();
        for u in [user(1, "ana"), user(2, "bo"), user(3, "cy")] {
            users::upsert(db.conn(), &u).unwrap();
        }
        // Before the walk: the walk already shows it.
        record(db.conn(), Pk::new(1), Verb::Unfollow, Epoch::new(900)).unwrap();
        // After it: bo unfollowed, cy followed.
        record(db.conn(), Pk::new(2), Verb::Unfollow, Epoch::new(1_100)).unwrap();
        record(db.conn(), Pk::new(3), Verb::Follow, Epoch::new(1_200)).unwrap();

        let mut members = vec![user(1, "ana"), user(2, "bo")];
        let applied = apply(db.conn(), &mut members, Epoch::new(1_000)).unwrap();
        let names: Vec<&str> = members.iter().map(|u| u.username.as_str()).collect();
        assert_eq!(names, ["cy", "ana"]);
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0].verb, Verb::Unfollow);
        assert_eq!(applied[0].username.as_deref(), Some("bo"));
    }

    #[test]
    fn the_last_write_to_an_account_is_the_one_that_counts() {
        let db = Store::in_memory().unwrap();
        users::upsert(db.conn(), &user(2, "bo")).unwrap();
        record(db.conn(), Pk::new(2), Verb::Unfollow, Epoch::new(1_100)).unwrap();
        record(db.conn(), Pk::new(2), Verb::Follow, Epoch::new(1_200)).unwrap();

        let mut members = vec![user(2, "bo")];
        let applied = apply(db.conn(), &mut members, Epoch::new(1_000)).unwrap();
        assert_eq!(members.len(), 1, "followed again: still there");
        assert!(applied.is_empty(), "and nothing changed to say");
    }
}
