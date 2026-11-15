//! What a view shows of an account at once, while the requests it makes
//! anyway run (`snob_store::cache`): read here without a request, and kept
//! from the answers those requests bring.
//!
//! **The cache adds no request and takes none away.** It is filled where an
//! answer already arrives: a profile read ([`crate::app::App::profile_named`]),
//! a counter poll (`engine::freshness`), a picture a view already downloaded
//! ([`Keeper::picture`]). Nothing reads it to decide whether to ask: a view
//! shows it, then shows what the request answered. There is no rule for when
//! to refresh it, since nothing refreshes it but those answers.

use std::path::PathBuf;

use snob_core::model::printable;
use snob_core::session::Session;
use snob_core::{Epoch, Pk};
use snob_ig::model::{Counters, WebProfileInfo};
use snob_store::cache::{self, Shown};
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::store::Store;
use snob_store::viewer::Card;

use crate::app::Viewer;
use crate::media::{MAX_SMALL_PICTURE, picture_type};

/// What `viewer` was last shown of account `pk`, at once: none when nothing is
/// kept. For the viewer's own account, what its card says fills what the
/// cache lacks. **Reads two small files and sends nothing.**
pub fn shown(paths: &AppPaths, viewer: Pk, pk: Pk) -> Option<Shown> {
    let account = paths.account(viewer);
    let kept = cache::load(&account, pk);
    if pk != viewer {
        return kept;
    }
    let card = Card::load(&account).map(|card| Shown {
        full_name: card.full_name,
        biography: card.biography,
        seen_at: card.seen_at,
        ..Shown::default()
    });
    match (card, kept) {
        (Some(card), Some(kept)) => Some(card.updated_by(kept)),
        (card, kept) => kept.or(card),
    }
}

/// The small picture of `pk` that `viewer` was last shown, when one is kept.
pub fn picture(paths: &AppPaths, viewer: Pk, pk: Pk) -> Option<PathBuf> {
    cache::picture(&paths.account(viewer), pk)
}

/// The pk `viewer` last saw under the name `typed`, from what is stored here:
/// the session's own name, then the account's database, **opened read-only**
/// (`Store::read_existing`). For a view entered by a name, which finds what
/// the cache holds by pk.
pub fn known(paths: &AppPaths, viewer: &Viewer, typed: &str) -> Option<Pk> {
    let account = paths.account(viewer.pk);
    let db = match Store::read_existing(&account) {
        Ok(Some(db)) => db,
        Ok(None) => Store::in_memory().ok()?,
        Err(e) => {
            tracing::debug!(error = %e, "the account's database could not be read");
            return None;
        }
    };
    crate::engine::target::known_pk_in(viewer, &db, typed)
        .ok()
        .flatten()
}

/// Keeps what answers showed one account, for as long as its session may
/// still be sent as here. **Never an error**: the cache is cosmetic, and no
/// read fails for it.
#[derive(Debug, Clone)]
pub struct Keeper {
    account: AccountPaths,
    /// When the session the answers came with was made.
    made: Epoch,
}

impl Keeper {
    /// The keeper of `account`, whose answers come with `session`.
    pub fn new(account: AccountPaths, session: &Session) -> Self {
        Self {
            account,
            made: session.created_at,
        }
    }

    /// Whether anything may still be kept: **nothing after a sign-out**,
    /// which removes the cache, or it would come back.
    fn may_keep(&self) -> bool {
        !self.account.refuses_session(self.made)
    }

    /// Keeps what a profile read answered.
    pub fn profile(&self, info: &WebProfileInfo) {
        let own = info.id == self.account.pk();
        let said = |text: Option<&str>| text.map(printable).filter(|text| !text.is_empty());
        let counted = info.counters_are_knowable();
        self.merge(
            info.id,
            Shown {
                username: said(Some(&info.username)),
                full_name: said(info.full_name.as_deref()),
                // A line at a time: a bio's line breaks are its author's.
                biography: info
                    .biography
                    .as_deref()
                    .map(|bio| bio.lines().map(printable).collect::<Vec<_>>().join("\n"))
                    .filter(|bio| !bio.trim().is_empty()),
                external_url: said(info.external_url.as_deref()),
                category: said(info.category_name.as_deref()),
                is_private: info.is_private,
                is_verified: info.is_verified,
                // The search route counts nothing: unknown, not zero.
                followers: info.follower_count().filter(|_| counted),
                following: info.following_count().filter(|_| counted),
                posts: info.posts.map(|posts| posts.count).filter(|_| counted),
                you_follow: info.followed_by_viewer.filter(|_| !own),
                follows_you: info.follows_viewer.filter(|_| !own),
                seen_at: None,
            },
        );
    }

    /// Keeps the two counters a poll of account `pk` answered.
    pub fn counters(&self, pk: Pk, counters: Counters) {
        self.merge(
            pk,
            Shown {
                followers: counters.followers,
                following: counters.following,
                ..Shown::default()
            },
        );
    }

    /// Keeps the small picture of `pk` **a view already downloaded**: what
    /// is not a picture, or weighs more than a small one may, is not kept.
    pub fn picture(&self, pk: Pk, bytes: &[u8]) {
        if bytes.len() > MAX_SMALL_PICTURE || picture_type(bytes).is_none() || !self.may_keep() {
            return;
        }
        if let Err(e) = cache::keep_picture(&self.account, pk, bytes) {
            tracing::debug!(error = %e, "a picture could not be kept");
        }
    }

    fn merge(&self, pk: Pk, newer: Shown) {
        if !self.may_keep() {
            return;
        }
        let newer = Shown {
            seen_at: Some(snob_core::clock::now()),
            ..newer
        };
        let kept = cache::load(&self.account, pk)
            .unwrap_or_default()
            .updated_by(newer);
        if let Err(e) = cache::save(&self.account, pk, &kept) {
            tracing::debug!(error = %e, "what an answer showed could not be kept");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::session::SessionOrigin;

    /// A sandbox's account 42, signed in with the session returned.
    fn signed_in(root: &std::path::Path) -> (AppPaths, Session) {
        let paths = AppPaths::rooted_at(root);
        let session =
            Session::from_sessionid("42%3Aabc%3A1", "Mozilla/5.0", SessionOrigin::Paste).unwrap();
        paths
            .account(Pk::new(42))
            .admit_session(session.created_at)
            .unwrap();
        (paths, session)
    }

    #[test]
    fn a_poll_adds_its_counters_to_what_a_profile_showed() {
        let root = tempfile::tempdir().unwrap();
        let (paths, session) = signed_in(root.path());
        let keeper = Keeper::new(paths.account(Pk::new(42)), &session);
        cache::save(
            &paths.account(Pk::new(42)),
            Pk::new(7),
            &Shown {
                username: Some("someone".into()),
                posts: Some(3),
                ..Shown::default()
            },
        )
        .unwrap();

        keeper.counters(
            Pk::new(7),
            Counters {
                followers: Some(25),
                following: Some(11),
            },
        );

        let shown = shown(&paths, Pk::new(42), Pk::new(7)).unwrap();
        assert_eq!(shown.username.as_deref(), Some("someone"));
        assert_eq!(
            (shown.followers, shown.following, shown.posts),
            (Some(25), Some(11), Some(3))
        );
        assert!(shown.seen_at.is_some());
    }

    #[test]
    fn nothing_is_kept_once_the_account_is_signed_out() {
        let root = tempfile::tempdir().unwrap();
        let (paths, session) = signed_in(root.path());
        let account = paths.account(Pk::new(42));
        let keeper = Keeper::new(account.clone(), &session);

        account.sign_out().unwrap();
        keeper.counters(
            Pk::new(7),
            Counters {
                followers: Some(1),
                following: Some(1),
            },
        );
        keeper.picture(Pk::new(7), b"\xff\xd8\xff");

        assert!(!account.cache_dir().exists());
    }

    #[test]
    fn what_is_not_a_small_picture_is_not_kept() {
        let root = tempfile::tempdir().unwrap();
        let (paths, session) = signed_in(root.path());
        let keeper = Keeper::new(paths.account(Pk::new(42)), &session);

        keeper.picture(Pk::new(7), b"<html>");
        keeper.picture(Pk::new(8), &[0xff; MAX_SMALL_PICTURE + 1]);
        keeper.picture(Pk::new(9), b"\xff\xd8\xff");

        assert_eq!(picture(&paths, Pk::new(42), Pk::new(7)), None);
        assert_eq!(picture(&paths, Pk::new(42), Pk::new(8)), None);
        assert!(picture(&paths, Pk::new(42), Pk::new(9)).is_some());
    }

    #[test]
    fn the_own_account_is_shown_from_its_card_where_the_cache_says_nothing() {
        let root = tempfile::tempdir().unwrap();
        let (paths, session) = signed_in(root.path());
        let account = paths.account(Pk::new(42));
        Card {
            full_name: Some("Invented Name".into()),
            ..Card::default()
        }
        .save(&account)
        .unwrap();
        Keeper::new(account, &session).counters(
            Pk::new(42),
            Counters {
                followers: Some(10),
                following: Some(10),
            },
        );

        let own = shown(&paths, Pk::new(42), Pk::new(42)).unwrap();
        assert_eq!(own.full_name.as_deref(), Some("Invented Name"));
        assert_eq!(own.followers, Some(10));
    }
}
