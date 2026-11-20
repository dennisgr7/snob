//! `users` is a metadata cache: being here means having been seen once.
//! Membership of a list lives in `snapshot_members`.

use rusqlite::{Connection, OptionalExtension, params};

use super::{StoreError, pk_from_sql, pk_to_sql};
use snob_core::Pk;
use snob_core::clock::now;
use snob_core::model::{USERNAME_MAX, User};

/// Records that an account exists without knowing its name: `accounts.pk`
/// references `users(pk)`, so the row has to exist first. The empty string
/// is "not known"; every lookup by name refuses it.
pub fn ensure(conn: &Connection, pk: Pk) -> Result<(), StoreError> {
    let now = now().get();
    conn.execute(
        "INSERT INTO users (pk, username, first_seen, last_seen)
         VALUES (?1, '', ?2, ?2)
         ON CONFLICT(pk) DO UPDATE SET last_seen = excluded.last_seen",
        params![pk_to_sql(pk), now],
    )?;
    Ok(())
}

/// Inserts or updates a user and returns the previous username when it
/// changed. An empty stored name ([`ensure`]) is not a rename.
///
/// `prepare_cached` because this runs once per account in
/// `snapshots::save_page`, the only per-account loop in the program.
pub fn upsert(conn: &Connection, u: &User) -> Result<Option<String>, StoreError> {
    let pk = pk_to_sql(u.pk);
    let now = now().get();

    let previous: Option<String> = conn
        .prepare_cached("SELECT username FROM users WHERE pk = ?1")?
        .query_row(params![pk], |row| row.get(0))
        .optional()?;

    conn.prepare_cached(
        "INSERT INTO users (pk, username, full_name, is_verified, is_private, pfp_url,
                            first_seen, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT(pk) DO UPDATE SET
             username    = excluded.username,
             -- Metadata is only overwritten when the new response carries it: a
             -- page arriving without full_name must not wipe the one we had.
             full_name   = coalesce(excluded.full_name,   users.full_name),
             is_verified = coalesce(excluded.is_verified, users.is_verified),
             is_private  = coalesce(excluded.is_private,  users.is_private),
             pfp_url     = coalesce(excluded.pfp_url,     users.pfp_url),
             last_seen   = excluded.last_seen",
    )?
    .execute(params![
        pk,
        u.username,
        u.full_name,
        u.is_verified,
        u.is_private,
        u.pfp_url,
        now,
    ])?;

    match previous {
        Some(old) if !old.is_empty() && old != u.username => {
            // Not cached: a rename is rare.
            conn.execute(
                "INSERT INTO username_history (pk, username, changed_at) VALUES (?1, ?2, ?3)",
                params![pk, old, now],
            )?;
            Ok(Some(old))
        }
        _ => Ok(None),
    }
}

/// The stored name, when one was ever learned.
pub fn name(conn: &Connection, pk: Pk) -> Result<Option<String>, StoreError> {
    Ok(find(conn, pk)?
        .map(|u| u.username)
        .filter(|name| !name.is_empty()))
}

/// The pk last seen under `name`, in any case, walked or not. Names are not
/// unique over time: the most recently seen wins, and the caller checks the
/// answer it gets by the pk against the name.
pub fn pk_named(conn: &Connection, name: &str) -> Result<Option<Pk>, StoreError> {
    if name.is_empty() {
        return Ok(None);
    }
    let pk = conn
        .query_row(PK_NAMED, params![name], |row| row.get(0))
        .optional()?;
    Ok(pk.map(pk_from_sql))
}

/// Sought in `users_username_nocase` (`021_lookup_indexes.sql`): the
/// comparison has to stay `COLLATE NOCASE` for the index to serve it.
const PK_NAMED: &str = "SELECT pk FROM users
             WHERE username = ?1 COLLATE NOCASE AND username <> ''
             ORDER BY last_seen DESC, pk LIMIT 1";

/// At most `limit` accounts whose name begins with `prefix` in any case, in
/// the order of their names, **once per name whatever its case**: the one
/// [`pk_named`] picks for it, so a name suggested resolves to the account
/// shown. A prefix no username can begin with names nobody.
pub fn named_from(conn: &Connection, prefix: &str, limit: usize) -> Result<Vec<User>, StoreError> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_';
    if prefix.is_empty() || prefix.len() > USERNAME_MAX || !prefix.chars().all(allowed) {
        return Ok(Vec::new());
    }
    // Every character a username may hold sorts below DEL, so the names that
    // begin with `prefix` are the range up to it: no pattern is matched.
    let below = format!("{prefix}\u{7f}");
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let names = conn
        .prepare(NAMED_FROM)?
        .query_map(params![prefix, below, limit], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut found = Vec::with_capacity(names.len());
    for name in names {
        if let Some(pk) = pk_named(conn, &name)?
            && let Some(user) = find(conn, pk)?
        {
            found.push(user);
        }
    }
    Ok(found)
}

/// A range in `users_username_nocase`, read in its order.
const NAMED_FROM: &str = "SELECT username FROM users
             WHERE username >= ?1 COLLATE NOCASE AND username < ?2 COLLATE NOCASE
               AND username <> ''
             GROUP BY username COLLATE NOCASE
             ORDER BY username COLLATE NOCASE
             LIMIT ?3";

/// The columns [`row_to_user`] reads, in its order: `is_private` and
/// `is_verified` are both `Option<bool>`, so a transposed pair compiles.
pub(crate) const USER_COLUMNS: &str = "pk, username, full_name, is_private, is_verified, pfp_url";

pub(crate) fn find(conn: &Connection, pk: Pk) -> Result<Option<User>, StoreError> {
    let u = conn
        .query_row(
            &format!("SELECT {USER_COLUMNS} FROM users WHERE pk = ?1"),
            params![pk_to_sql(pk)],
            row_to_user,
        )
        .optional()?;
    Ok(u)
}

pub(crate) fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    Ok(User {
        pk: pk_from_sql(row.get(0)?),
        username: row.get(1)?,
        full_name: row.get(2)?,
        is_private: row.get(3)?,
        is_verified: row.get(4)?,
        pfp_url: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    /// `users` holds a row for every account ever seen on any page: a name is
    /// looked up in the index, never by reading them all.
    #[test]
    fn a_name_is_sought_in_the_index_not_scanned_for() {
        let db = Store::in_memory().unwrap();
        let plan = crate::store::plan(db.conn(), PK_NAMED);
        assert!(plan.contains("users_username_nocase"), "{plan}");
        assert!(!plan.contains("SCAN users"), "{plan}");
    }

    /// The names filed against an account, oldest first.
    ///
    /// A test helper rather than public API, because it orders by
    /// `changed_at`, which the production reader deliberately does not:
    /// `watch::renames_since` orders by `id`, because two names filed in the
    /// same second have no order at all by the clock and one by arrival.
    fn previous_usernames(conn: &Connection, pk: Pk) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT username FROM username_history WHERE pk = ?1 ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map(params![pk_to_sql(pk)], |row| row.get::<_, String>(0))
            .unwrap();
        rows.collect::<Result<_, _>>().unwrap()
    }

    fn user(pk: u64, name: &str) -> User {
        User {
            pk: Pk::new(pk),
            username: name.into(),
            full_name: Some("Full Name".into()),
            is_private: Some(false),
            is_verified: Some(false),
            pfp_url: None,
        }
    }

    /// A sequence that must not corrupt a real name.
    ///
    /// A session stored without a username reaches `ensure`; a later run online
    /// learns the name; a run with `--cache` reaches `ensure` again. Writing
    /// anything over the name at the last step would leave the account unfound
    /// by it and file a rename that never happened.
    #[test]
    fn a_run_that_does_not_know_the_name_never_overwrites_one() {
        let db = Store::in_memory().unwrap();

        ensure(db.conn(), Pk::new(7)).unwrap();
        upsert(db.conn(), &user(7, "realname")).unwrap();
        ensure(db.conn(), Pk::new(7)).unwrap();

        assert_eq!(
            find(db.conn(), Pk::new(7)).unwrap().unwrap().username,
            "realname"
        );
        assert!(
            previous_usernames(db.conn(), Pk::new(7)).is_empty(),
            "nothing was renamed, so nothing may be filed as a rename"
        );
    }

    /// Learning the name for the first time is not a rename either: the empty
    /// string is what `ensure` writes for "never knew it".
    #[test]
    fn learning_a_name_for_the_first_time_is_not_a_rename() {
        let db = Store::in_memory().unwrap();
        ensure(db.conn(), Pk::new(7)).unwrap();

        assert_eq!(upsert(db.conn(), &user(7, "realname")).unwrap(), None);
        assert!(previous_usernames(db.conn(), Pk::new(7)).is_empty());

        // A genuine rename still is one.
        assert_eq!(
            upsert(db.conn(), &user(7, "newname")).unwrap().as_deref(),
            Some("realname")
        );
    }

    /// The empty name must never be findable: it is a placeholder, not a
    /// username, and matching it would hand back the wrong account.
    #[test]
    fn an_unnamed_account_cannot_be_looked_up_by_name() {
        let db = Store::in_memory().unwrap();
        ensure(db.conn(), Pk::new(7)).unwrap();
        crate::store::accounts::upsert(db.conn(), Pk::new(7), true).unwrap();

        let found = crate::store::accounts::find_pk_by_username(db.conn(), "").unwrap();
        assert_eq!(found, None);
    }

    #[test]
    fn inserting_twice_leaves_a_single_row() {
        let db = Store::in_memory().unwrap();
        upsert(db.conn(), &user(1, "one")).unwrap();
        upsert(db.conn(), &user(1, "one")).unwrap();

        let rows: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM users", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn a_rename_is_recorded() {
        let db = Store::in_memory().unwrap();
        upsert(db.conn(), &user(1, "old_name")).unwrap();

        let previous = upsert(db.conn(), &user(1, "new_name")).unwrap();
        assert_eq!(previous.as_deref(), Some("old_name"));
        assert_eq!(previous_usernames(db.conn(), Pk::new(1)), vec!["old_name"]);

        let unchanged = upsert(db.conn(), &user(1, "new_name")).unwrap();
        assert_eq!(unchanged, None);
    }

    #[test]
    fn a_partial_response_does_not_wipe_existing_metadata() {
        let db = Store::in_memory().unwrap();
        upsert(db.conn(), &user(1, "one")).unwrap();

        let sparse = User {
            pk: Pk::new(1),
            username: "one".into(),
            full_name: None,
            is_private: None,
            is_verified: None,
            pfp_url: None,
        };
        upsert(db.conn(), &sparse).unwrap();

        let stored = find(db.conn(), Pk::new(1)).unwrap().unwrap();
        assert_eq!(stored.full_name.as_deref(), Some("Full Name"));
        assert_eq!(stored.is_verified, Some(false));
    }

    #[test]
    fn first_seen_stays_put_and_last_seen_moves() {
        let db = Store::in_memory().unwrap();
        upsert(db.conn(), &user(1, "one")).unwrap();
        db.conn()
            .execute("UPDATE users SET first_seen = 100, last_seen = 100", [])
            .unwrap();

        upsert(db.conn(), &user(1, "one")).unwrap();

        let (first, last): (i64, i64) = db
            .conn()
            .query_row("SELECT first_seen, last_seen FROM users", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(first, 100, "first_seen should not move");
        assert!(last > 100, "last_seen should have been updated");
    }

    #[test]
    fn a_user_that_does_not_exist_returns_nothing() {
        let db = Store::in_memory().unwrap();
        assert!(find(db.conn(), Pk::new(999)).unwrap().is_none());
    }

    #[test]
    fn a_pk_is_named_in_any_case_by_the_latest_holder_of_the_name() {
        let db = Store::in_memory().unwrap();
        upsert(db.conn(), &user(7, "someone")).unwrap();
        upsert(db.conn(), &user(8, "Someone")).unwrap();
        db.conn()
            .execute("UPDATE users SET last_seen = 100 WHERE pk = 8", [])
            .unwrap();
        assert_eq!(pk_named(db.conn(), "SOMEONE").unwrap(), Some(Pk::new(7)));
        db.conn()
            .execute("UPDATE users SET last_seen = 100 WHERE pk = 7", [])
            .unwrap();
        db.conn()
            .execute("UPDATE users SET last_seen = 200 WHERE pk = 8", [])
            .unwrap();
        assert_eq!(pk_named(db.conn(), "someone").unwrap(), Some(Pk::new(8)));
        assert_eq!(pk_named(db.conn(), "nobody").unwrap(), None);
    }

    #[test]
    fn accounts_are_found_by_their_first_letters_in_any_case_once_each() {
        let db = Store::in_memory().unwrap();
        for (pk, name) in [
            (1, "someone"),
            (2, "Someone"),
            (3, "somebody"),
            (4, "some_one"),
            (5, "other"),
            (6, "SOMETHING"),
        ] {
            upsert(db.conn(), &user(pk, name)).unwrap();
        }
        ensure(db.conn(), Pk::new(7)).unwrap();
        db.conn()
            .execute("UPDATE users SET last_seen = 100 WHERE pk = 1", [])
            .unwrap();
        let pks = |prefix: &str, limit: usize| -> Vec<u64> {
            named_from(db.conn(), prefix, limit)
                .unwrap()
                .iter()
                .map(|u| u.pk.get())
                .collect()
        };

        assert_eq!(pks("SoMe", 10), [4, 3, 2, 6], "the latest holder of a name");
        assert_eq!(pks("some", 2), [4, 3]);
        assert_eq!(pks("someone", 10), [2]);
        assert_eq!(pks("x", 10), Vec::<u64>::new());
        // A wildcard of a pattern is a letter here: `s_` begins no name.
        assert_eq!(pks("s_", 10), Vec::<u64>::new());
        for nobody in ["", "@some", "s%", "some one", "some\u{7f}"] {
            assert_eq!(pks(nobody, 10), Vec::<u64>::new(), "{nobody:?}");
        }
        assert_eq!(pks("some", 0), Vec::<u64>::new());

        // Seen in the same second: the account the name resolves to.
        upsert(db.conn(), &user(9, "tied")).unwrap();
        upsert(db.conn(), &user(8, "TIED")).unwrap();
        db.conn()
            .execute("UPDATE users SET last_seen = 200 WHERE pk IN (8, 9)", [])
            .unwrap();
        assert_eq!(pks("ti", 10), [8]);
        assert_eq!(pk_named(db.conn(), "tied").unwrap(), Some(Pk::new(8)));
    }

    /// Typing a letter must not read every account ever seen.
    #[test]
    fn the_search_by_first_letters_uses_the_username_index() {
        let db = Store::in_memory().unwrap();
        let plan = crate::store::plan(db.conn(), NAMED_FROM);
        assert!(plan.contains("users_username_nocase"), "{plan}");
        assert!(!plan.contains("SCAN users"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }

    /// The empty name is "never learned", and names nobody.
    #[test]
    fn the_empty_name_names_nobody() {
        let db = Store::in_memory().unwrap();
        ensure(db.conn(), Pk::new(9)).unwrap();
        assert_eq!(pk_named(db.conn(), "").unwrap(), None);
    }
}
