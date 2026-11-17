//! What a view shows of an account at once, while the requests it makes
//! anyway run (`snob_store::cache`): read here without a request, and kept
//! from the answers those requests bring.
//!
//! **The cache adds no request, and takes none away but one**: a post's
//! thumbnail already kept, which never changes, is not downloaded again
//! ([`Keeper::fetch_thumb`]). It is filled where an answer already arrives: a
//! profile read ([`crate::app::App::profile_named`], and its highlights'
//! tray in [`crate::profile::read`]), the first page of a grid or a tab
//! ([`crate::posts::first_page`]), a counter poll (`engine::freshness`), a
//! picture a view downloads from the CDN for itself
//! ([`Keeper::fetch_picture`], [`Keeper::fetch_drawing`]). Nothing else
//! reads it to decide whether to ask: a view shows it, then shows what the
//! request answered. There is no rule for when to refresh it, since nothing
//! refreshes it but those answers.

use std::path::PathBuf;

use snob_core::model::printable;
use snob_core::session::Session;
use snob_core::{Epoch, Pk};
use snob_ig::client::{IgClient, ProfileTab};
use snob_ig::model::{Counters, WebProfileInfo};
use snob_store::cache::{self, Shown, Tab};
pub use snob_store::cache::{Drawing, KeptGrid, KeptHighlight, KeptKind, KeptPost, KeptTray};
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::viewer::Card;

use crate::media::{MAX_LARGE_PICTURE, MAX_SMALL_PICTURE, picture_type};
use crate::posts::{Grid, Post, PostKind};
use crate::profile::{Highlight, Visibility};

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

/// The small picture of `pk` that `viewer` was last shown, when one is kept:
/// for the viewer's own account, its card's first (`viewer::picture`).
pub fn picture(paths: &AppPaths, viewer: Pk, pk: Pk) -> Option<PathBuf> {
    let account = paths.account(viewer);
    if pk == viewer {
        let own = account.viewer_picture();
        if own.is_file() {
            return Some(own);
        }
    }
    cache::picture(&account, pk)
}

/// The drawing named `key` that `viewer` was last shown, when one is kept.
pub fn drawing(paths: &AppPaths, viewer: Pk, drawing: Drawing, key: &str) -> Option<PathBuf> {
    cache::drawing(&paths.account(viewer), drawing, key)
}

/// The highlights of `pk` that `viewer` was last shown, none when nothing is
/// kept. **Reads one file and sends nothing.**
pub fn tray(paths: &AppPaths, viewer: Pk, pk: Pk) -> Option<KeptTray> {
    cache::load_tray(&paths.account(viewer), pk)
}

/// The first page of `pk`'s grid (`tab` `None`) or tab that `viewer` was
/// last shown, none when nothing is kept. **Reads one file and sends
/// nothing.**
pub fn grid(paths: &AppPaths, viewer: Pk, pk: Pk, tab: Option<ProfileTab>) -> Option<KeptGrid> {
    cache::load_grid(&paths.account(viewer), pk, tab_of(tab))
}

fn tab_of(tab: Option<ProfileTab>) -> Tab {
    match tab {
        None => Tab::Posts,
        Some(ProfileTab::Reels) => Tab::Reels,
        Some(ProfileTab::Reposts) => Tab::Reposts,
        Some(ProfileTab::Tagged) => Tab::Tagged,
    }
}

/// `post` as its tile shows it, its words made printable: what a grid keeps
/// of it.
pub fn kept_post(post: &Post) -> KeptPost {
    KeptPost {
        code: post.code.clone(),
        kind: Some(match post.kind {
            PostKind::Photo => KeptKind::Photo,
            PostKind::Video => KeptKind::Video,
            PostKind::Reel => KeptKind::Reel,
            PostKind::Carousel(items) => KeptKind::Carousel(items),
        }),
        caption: Some(printable(post.headline())).filter(|line| !line.trim().is_empty()),
        owner: Some(printable(&post.owner)).filter(|owner| !owner.is_empty()),
        taken_at: (post.taken_at != Epoch::default()).then_some(post.taken_at),
        // Never read under hidden counts: the number is then a placeholder.
        likes: post.likes.filter(|_| !post.likes_hidden),
        comments: post.comments,
        pinned: post.pinned,
    }
}

/// The largest picture of `pk` kept for `viewer`, else its small one.
pub fn largest(paths: &AppPaths, viewer: Pk, pk: Pk) -> Option<PathBuf> {
    drawing(paths, viewer, Drawing::Largest, &pk.to_string()).or_else(|| picture(paths, viewer, pk))
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

    /// Keeps the highlights a profile read showed of `pk`: **none when it
    /// showed none**, a private account's to a viewer who does not follow it
    /// among them.
    pub fn tray(&self, pk: Pk, highlights: &Visibility<Vec<Highlight>>) {
        if !self.may_keep() {
            return;
        }
        let highlights = match highlights {
            Visibility::Shown(tray) => tray
                .iter()
                .map(|highlight| KeptHighlight {
                    id: highlight.id.clone(),
                    title: Some(highlight.title.clone()).filter(|title| !title.is_empty()),
                })
                .collect(),
            Visibility::Hidden => Vec::new(),
        };
        if let Err(e) = cache::save_tray(&self.account, pk, &KeptTray { highlights }) {
            tracing::debug!(error = %e, "a tray could not be kept");
        }
    }

    /// Keeps the first page of `pk`'s grid or tab a read answered.
    pub fn grid(&self, pk: Pk, grid: &Grid) {
        if !self.may_keep() {
            return;
        }
        let kept = KeptGrid {
            posts: grid.posts.iter().map(kept_post).collect(),
            more: grid.next.is_some(),
        };
        if let Err(e) = cache::save_grid(&self.account, pk, tab_of(grid.tab), &kept) {
            tracing::debug!(error = %e, "a grid could not be kept");
        }
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

    /// The small picture of `pk` at `address`, downloaded for a view that
    /// shows it ([`Self::picture`]): the file kept, when there is one.
    pub async fn fetch_picture(&self, client: &IgClient, pk: Pk, address: &str) -> Option<PathBuf> {
        let bytes = downloaded(client, address, MAX_SMALL_PICTURE).await?;
        self.picture(pk, &bytes);
        cache::picture(&self.account, pk)
    }

    /// The drawing named `key` at `address`, downloaded for a view that
    /// draws it and kept as the small picture is: the file kept, when there
    /// is one.
    pub async fn fetch_drawing(
        &self,
        client: &IgClient,
        drawing: Drawing,
        key: &str,
        address: &str,
    ) -> Option<PathBuf> {
        let cap = match drawing {
            Drawing::Largest => MAX_LARGE_PICTURE,
            Drawing::Cover | Drawing::Thumb => MAX_SMALL_PICTURE,
        };
        let bytes = downloaded(client, address, cap).await?;
        self.drawing(drawing, key, &bytes)
    }

    /// The thumbnail of the post `code` at `address`: **the one kept, with
    /// nothing downloaded, when there is one**, since a post's picture never
    /// changes; else downloaded and kept as [`Self::fetch_drawing`] does.
    pub async fn fetch_thumb(
        &self,
        client: &IgClient,
        code: &str,
        address: &str,
    ) -> Option<PathBuf> {
        if let Some(kept) = cache::drawing(&self.account, Drawing::Thumb, code) {
            return Some(kept);
        }
        self.fetch_drawing(client, Drawing::Thumb, code, address)
            .await
    }

    /// Keeps the drawing named `key` **a view already downloaded**: what is
    /// not a picture is not kept. The file kept, when there is one.
    pub fn drawing(&self, drawing: Drawing, key: &str, bytes: &[u8]) -> Option<PathBuf> {
        if picture_type(bytes).is_none() || !self.may_keep() {
            return None;
        }
        match cache::keep_drawing(&self.account, drawing, key, bytes) {
            Ok(true) => cache::drawing(&self.account, drawing, key),
            Ok(false) => None,
            Err(e) => {
                tracing::debug!(error = %e, "a picture could not be kept");
                None
            }
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

/// The bytes at `address` on the CDN, at most `cap` of them: one download,
/// which Instagram's pace does not count.
async fn downloaded(client: &IgClient, address: &str, cap: usize) -> Option<Vec<u8>> {
    if address.is_empty() {
        return None;
    }
    match client.download_capped(address, cap).await {
        Ok(bytes) => Some(bytes),
        // Not the error's own words: they can hold the address.
        Err(_) => {
            tracing::debug!("a picture could not be fetched");
            None
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
    fn a_tray_is_kept_as_shown_and_kept_empty_once_hidden() {
        let root = tempfile::tempdir().unwrap();
        let (paths, session) = signed_in(root.path());
        let keeper = Keeper::new(paths.account(Pk::new(42)), &session);
        let shown = Visibility::Shown(vec![Highlight {
            id: "highlight:1".into(),
            title: String::new(),
            items: Some(3),
            updated_at: None,
            cover: Some("https://cdn.invalid/1.jpg".into()),
        }]);

        keeper.tray(Pk::new(7), &shown);
        assert_eq!(
            tray(&paths, Pk::new(42), Pk::new(7)),
            Some(KeptTray {
                highlights: vec![KeptHighlight {
                    id: "highlight:1".into(),
                    title: None,
                }],
            })
        );

        keeper.tray(Pk::new(7), &Visibility::Hidden);
        assert_eq!(
            tray(&paths, Pk::new(42), Pk::new(7)),
            Some(KeptTray::default())
        );
    }

    #[test]
    fn a_first_page_is_kept_by_its_tab_as_its_tiles_show_it() {
        let root = tempfile::tempdir().unwrap();
        let (paths, session) = signed_in(root.path());
        let keeper = Keeper::new(paths.account(Pk::new(42)), &session);
        let media = serde_json::from_str(
            r#"{"pk":"2","code":"AAAAAAAAAAC","media_type":8,"taken_at":1790000001,
            "caption":{"text":"first\u0007 line\nsecond"},"like_count":5,
            "like_and_view_counts_disabled":true,"comment_count":4,
            "carousel_media":[{"pk":"21","media_type":1},{"pk":"22","media_type":1}]}"#,
        )
        .unwrap();
        let mut grid = Grid::of("someone", Some(ProfileTab::Tagged), Some(Pk::new(7)));
        grid.posts = vec![Post::from_media(&media, "owner.two")];
        grid.next = Some("cursor".into());

        keeper.grid(Pk::new(7), &grid);

        assert_eq!(grid_of(&paths, None), None, "a tab each");
        assert_eq!(
            grid_of(&paths, Some(ProfileTab::Tagged)),
            Some(KeptGrid {
                posts: vec![KeptPost {
                    code: "AAAAAAAAAAC".into(),
                    kind: Some(KeptKind::Carousel(2)),
                    caption: Some("first line".into()),
                    owner: Some("owner.two".into()),
                    taken_at: Some(Epoch::new(1_790_000_001)),
                    likes: None,
                    comments: Some(4),
                    pinned: false,
                }],
                more: true,
            })
        );
    }

    fn grid_of(paths: &AppPaths, tab: Option<ProfileTab>) -> Option<KeptGrid> {
        grid(paths, Pk::new(42), Pk::new(7), tab)
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
