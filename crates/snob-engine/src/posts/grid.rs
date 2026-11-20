//! A profile's grid and its tabs, a page of twelve at a time: the profile,
//! then each page a step after the one before, as far as the reader asked
//! or to a post it already has; or, for a view that reads on as it is
//! scrolled, one page at a time from the cursor the last one handed out
//! ([`After`], [`page_after`]).

use std::time::{Duration, Instant};

use anyhow::Result;
use snob_ig::client::{IgClient, ProfileTab};
use snob_ig::error::IgError;
use snob_ig::pace::Pace;
use snob_ig::shortcode::MediaPk;

use super::{MOST_PAGES, Post};
use crate::engine::{rest_before_a_page, rest_before_a_page_telling};

/// A profile's grid as far as it was read.
#[derive(Debug, Clone)]
pub struct Grid {
    /// As Instagram spells it.
    pub username: String,
    pub posts: Vec<Post>,
    /// Where the next page starts; `None` once the grid has ended.
    pub next: Option<String>,
    /// How many posts the profile says it has.
    pub declared: Option<u64>,
    /// The tab it is, `None` for the grid itself.
    pub tab: Option<ProfileTab>,
    /// The account's pk: what a tab is asked by.
    pub pk: Option<snob_core::Pk>,
    /// Whether the app was navigated to the tab, which it is once, before
    /// the tab's second page.
    pub tab_opened: bool,
    /// How many pages of it were read.
    pages: u32,
}

impl Grid {
    /// An empty grid of `username`'s, of `tab` when it is one.
    pub fn of(username: &str, tab: Option<ProfileTab>, pk: Option<snob_core::Pk>) -> Self {
        Self {
            username: username.to_string(),
            posts: Vec::new(),
            next: None,
            declared: None,
            tab,
            pk,
            tab_opened: false,
            pages: 0,
        }
    }

    /// What it is, for a sentence: "posts", "reels", "tagged posts",
    /// "reposts".
    pub fn what(&self) -> &'static str {
        match self.tab {
            None => "posts",
            Some(ProfileTab::Reels) => "reels",
            Some(ProfileTab::Tagged) => "tagged posts",
            Some(ProfileTab::Reposts) => "reposts",
        }
    }

    /// Where the page after the last one read starts: `None` when no page
    /// was read, the grid has ended, its account is not known by pk, or
    /// [`MOST_PAGES`] were read.
    pub fn after(&self) -> Option<After> {
        if self.pages == 0 || self.pages >= MOST_PAGES {
            return None;
        }
        Some(After {
            username: self.username.clone(),
            pk: self.pk?,
            tab: self.tab,
            cursor: self.next.clone()?,
            tab_opened: self.tab_opened,
            asked_at: Instant::now(),
            pages: self.pages,
        })
    }
}

/// Where a grid or a tab reads on: the cursor its last page handed out, how
/// many pages were read and whether the app was navigated to the tab. It has
/// no public field and no constructor: [`Grid::after`] makes it from a grid
/// read and [`page_after`] moves it on, so a view reading on as it is
/// scrolled stops at [`MOST_PAGES`] and sends back the cursor a page handed
/// out (`snob_ig` refuses a tab's cursor of another shape all the same).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct After {
    username: String,
    pk: snob_core::Pk,
    tab: Option<ProfileTab>,
    cursor: String,
    /// Whether the app was navigated to the tab, so it is never sent twice.
    tab_opened: bool,
    /// When this read last reached the account's browser.
    asked_at: Instant,
    /// How many pages were read, the one that handed out `cursor` included.
    pages: u32,
}

impl After {
    /// Whether the tab can still be taken as opened: **a browser idle for
    /// [`IDLE`](crate::browser::headless::IDLE) may have been closed**, and
    /// the tab of the one that replaces it was never navigated to.
    fn tab_still_opened(&self) -> bool {
        self.tab_opened && self.asked_at.elapsed() < crate::browser::headless::IDLE
    }
}

/// The page `after` points at, past any rest the account's sitting owes
/// (`resting` hears how long before it begins) and a step after the page
/// before, as [`more`] reads it; and where the page after it starts. A wait
/// stopped is [`IgError::Canceled`].
///
/// **`after` keeps a navigation to the tab once sent**, whether or not its
/// page came, so reading on from it again does not send another.
pub async fn page_after(
    app: &crate::app::App,
    after: &mut After,
    resting: impl FnOnce(Duration),
) -> Result<(Grid, Option<After>)> {
    let client = app.client();
    let mut grid = Grid::of(&after.username, after.tab, Some(after.pk));
    grid.next = Some(after.cursor.clone());
    grid.pages = after.pages;
    grid.tab_opened = after.tab_still_opened();
    let opened_before = grid.tab_opened;
    let of = format!("the {}", grid.what());
    let read = match rest_before_a_page_telling(client, &of, resting).await {
        Ok(true) => Ok(false),
        Ok(false) => more(client, &mut grid).await,
        Err(e) => Err(e),
    };
    after.tab_opened = grid.tab_opened;
    if grid.tab_opened && !opened_before {
        after.asked_at = Instant::now();
    }
    match read {
        Ok(true) => {
            let next = grid.after();
            Ok((grid, next))
        }
        Ok(false) => Err(IgError::Canceled.into()),
        Err(e) => Err(e),
    }
}

/// What a profile's grid could show.
pub enum Fetched {
    Grid(Grid),
    /// Private, and the viewer does not follow it: "no posts" and "posts you
    /// may not see" are different sentences, as `profile` keeps them.
    Hidden {
        username: String,
    },
}

/// The profile `typed` names and the first `pages` pages of its grid, more if
/// `at_least` posts take more, up to [`MOST_PAGES`].
pub async fn fetch_grid(
    client: &IgClient,
    typed: &str,
    viewer: snob_core::Pk,
    known: Option<snob_core::Pk>,
    pages: u32,
    at_least: usize,
) -> Result<Fetched> {
    let (fetched, _) = read_grid(client, typed, viewer, known, pages, at_least, &[]).await?;
    Ok(fetched)
}

/// How many pages a grid read with `--until` reads at most when `--pages` does
/// not say: three, 36 posts, which is days of most accounts' posting.
pub const UNTIL_PAGES: u32 = 3;

/// How a grid's read went: what it cost, and whether it reached a post the
/// reader already had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridRead {
    /// How many pages of the grid were asked for, one request each.
    pub pages: u32,
    /// The first of the posts named to stop at that the pages read hold,
    /// when one was named and found.
    pub reached: Option<MediaPk>,
}

/// The first post of `posts` that is one of `until` and is not pinned.
///
/// **A pinned post never stops a read.**
pub fn reached(posts: &[Post], until: &[MediaPk]) -> Option<MediaPk> {
    posts
        .iter()
        .find(|post| !post.pinned && until.contains(&post.pk))
        .map(|post| post.pk)
}

/// The profile a grid or a tab is read from, or the username of one the
/// viewer is not shown ([`crate::engine::target::shown_to`]).
enum Opened {
    Shown(Box<snob_ig::model::WebProfileInfo>),
    Hidden(String),
}

async fn opened(
    client: &IgClient,
    typed: &str,
    viewer: snob_core::Pk,
    known: Option<snob_core::Pk>,
) -> Result<Opened> {
    let info = client
        .profile_named(crate::engine::target::clean(typed), known)
        .await?;
    Ok(if crate::engine::target::shown_to(&info, viewer) {
        Opened::Shown(Box::new(info))
    } else {
        Opened::Hidden(info.username)
    })
}

fn not_shown(username: String) -> (Fetched, GridRead) {
    (Fetched::Hidden { username }, GridRead::default())
}

/// [`fetch_grid`], stopping early at a post the reader already has: no page is
/// asked for once the pages read hold one of `until` ([`reached`]), so a reader
/// that comes back each day pays for the new posts and not for the grid.
pub async fn read_grid(
    client: &IgClient,
    typed: &str,
    viewer: snob_core::Pk,
    known: Option<snob_core::Pk>,
    pages: u32,
    at_least: usize,
    until: &[MediaPk],
) -> Result<(Fetched, GridRead)> {
    let info = match opened(client, typed, viewer, known).await? {
        Opened::Hidden(username) => return Ok(not_shown(username)),
        Opened::Shown(info) => *info,
    };
    let declared = info.posts.map(|p| p.count);
    // Zero is an answer the profile already gave, as in `highlights`.
    if declared == Some(0) {
        let mut grid = Grid::of(&info.username, None, Some(info.id));
        grid.declared = declared;
        return Ok((Fetched::Grid(grid), GridRead::default()));
    }
    let (mut grid, how) = read_pages(client, &info.username, pages, at_least, until).await?;
    grid.declared = declared;
    Ok((Fetched::Grid(grid), how))
}

/// The pages of `username`'s grid, once its profile is open: the first, then
/// each next one a step after the page before and past any rest the account's
/// sitting owes, while fewer than `at_least` posts are held, or while fewer
/// than `pages` were read and none of `until` has come ([`reached`]); at most
/// [`MOST_PAGES`].
pub async fn read_pages(
    client: &IgClient,
    username: &str,
    pages: u32,
    at_least: usize,
    until: &[MediaPk],
) -> Result<(Grid, GridRead)> {
    let mut grid = Grid::of(username, None, None);
    let first = client.profile_posts(&grid.username, None).await?;
    grid.take(first);
    let how = read_on(client, &mut grid, pages, at_least, until).await?;
    Ok((grid, how))
}

/// Tab `tab` of the profile `typed` names, as [`read_grid`] reads its grid: the
/// profile, then the tab's first page, and its next ones as far as `pages`,
/// `at_least` and `until` say.
///
/// **No shortcut on a profile that says it has no posts**: its header counts
/// the grid alone, and an account with none had a reel, reposts and tagged
/// posts over more than a page in the capture of 2026-11-14.
#[expect(clippy::too_many_arguments, reason = "read_grid's, and the tab")]
pub async fn read_tab(
    client: &IgClient,
    typed: &str,
    viewer: snob_core::Pk,
    known: Option<snob_core::Pk>,
    tab: ProfileTab,
    pages: u32,
    at_least: usize,
    until: &[MediaPk],
) -> Result<(Fetched, GridRead)> {
    let info = match opened(client, typed, viewer, known).await? {
        Opened::Hidden(username) => return Ok(not_shown(username)),
        Opened::Shown(info) => *info,
    };
    let mut grid = Grid::of(&info.username, Some(tab), Some(info.id));
    let first = client
        .profile_tab(tab, info.id, &grid.username, None)
        .await?;
    grid.take(first);
    let how = read_on(client, &mut grid, pages, at_least, until).await?;
    Ok((Fetched::Grid(grid), how))
}

/// The first page of the grid (`tab` `None`) or of a tab of `pk`, an
/// account already resolved: what a profile page shows once the tab is
/// shown, with no resolve of its own. Kept in the account's cache for a view
/// to show at once next time (`cache::Keeper::grid`).
pub async fn first_page(
    app: &crate::app::App,
    username: &str,
    pk: snob_core::Pk,
    tab: Option<ProfileTab>,
) -> Result<Grid> {
    let client = app.client();
    let mut grid = Grid::of(username, tab, Some(pk));
    let first = match tab {
        None => client.profile_posts(username, None).await?,
        Some(tab) => client.profile_tab(tab, pk, username, None).await?,
    };
    grid.take(first);
    if let Some(keeper) = app.cache() {
        keeper.grid(pk, &grid);
    }
    Ok(grid)
}

/// Reads `grid` on from its first page, each next one a step after the
/// page before and past any rest the account's sitting owes, while fewer
/// than `at_least` posts are held, or while fewer than `pages` were read and
/// none of `until` has come ([`reached`]); at most [`MOST_PAGES`].
async fn read_on(
    client: &IgClient,
    grid: &mut Grid,
    pages: u32,
    at_least: usize,
    until: &[MediaPk],
) -> Result<GridRead> {
    let mut read = 1;
    // A post's number asked for is read up to, whatever `until` says; past
    // it, a page is asked for only while none of `until` has come.
    while read < MOST_PAGES
        && (grid.posts.len() < at_least || (read < pages && reached(&grid.posts, until).is_none()))
    {
        if rest_before_a_page(client, "the grid").await? || !more(client, grid).await? {
            break;
        }
        read += 1;
    }
    Ok(GridRead {
        pages: read,
        reached: reached(&grid.posts, until),
    })
}

impl Grid {
    fn take(&mut self, page: snob_ig::client::PostsPage) {
        // **A tagged post or a repost is its author's**, never the
        // profile's whose tab it is on (12 of 12 in the capture of
        // 2026-11-14): the answer names the owner, and where it does not,
        // the owner is not known rather than taken to be the profile.
        let owner = match self.tab {
            None | Some(ProfileTab::Reels) => self.username.clone(),
            Some(ProfileTab::Tagged | ProfileTab::Reposts) => String::new(),
        };
        self.posts
            .extend(page.posts.iter().map(|m| Post::from_media(m, &owner)));
        self.next = page.next;
        self.pages += 1;
    }
}

/// The grid's next page, a step after the page before as the pages of an
/// action are. `false` when the grid had ended, or Ctrl+C stopped the wait.
pub async fn more(client: &IgClient, grid: &mut Grid) -> Result<bool> {
    let Some(after) = grid.next.clone() else {
        return Ok(false);
    };
    if client.step_between(&Pace::default()).await {
        return Ok(false);
    }
    let page = match (grid.tab, grid.pk) {
        (Some(tab), Some(pk)) => {
            // The click that opens the tab, then a step, then its next
            // page from it, as the app scrolls a tab it has opened.
            if !grid.tab_opened {
                client.open_tab(&grid.username, tab).await?;
                grid.tab_opened = true;
                if client.step_between(&Pace::default()).await {
                    return Ok(false);
                }
            }
            client
                .profile_tab(tab, pk, &grid.username, Some(&after))
                .await?
        }
        _ => client.profile_posts(&grid.username, Some(&after)).await?,
    };
    grid.take(page);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read stops at a post the reader has, the first in the grid's order,
    /// and never at one its owner pinned, which comes first however old.
    #[test]
    fn a_pinned_post_never_stops_a_read() {
        let post = |pk: u64, pinned: bool| {
            let ids = if pinned { "[42]" } else { "[]" };
            let json =
                format!(r#"{{"pk":"{pk}","user":{{"pk":"42"}},"timeline_pinned_user_ids":{ids}}}"#);
            Post::from_media(&serde_json::from_str(&json).unwrap(), "x")
        };
        let grid = [post(9, true), post(5, false), post(4, false)];
        let pk = MediaPk::new;
        assert_eq!(reached(&grid, &[pk(9)]), None);
        assert_eq!(reached(&grid, &[pk(9), pk(4)]), Some(pk(4)));
        assert_eq!(reached(&grid, &[pk(4), pk(5)]), Some(pk(5)));
        assert_eq!(reached(&grid, &[]), None);
    }

    /// A reels tab of the account `pk` read `pages` pages, the last of which
    /// handed out `next`.
    fn read(next: Option<&str>, pk: Option<u64>, pages: u32) -> Grid {
        let mut grid = Grid::of(
            "someone",
            Some(ProfileTab::Reels),
            pk.map(snob_core::Pk::new),
        );
        grid.next = next.map(str::to_string);
        grid.pages = pages;
        grid
    }

    #[test]
    fn a_grid_unread_ended_or_whose_account_is_not_known_by_pk_reads_on_from_nowhere() {
        assert_eq!(read(Some("reels:12"), Some(7), 0).after(), None);
        assert_eq!(read(None, Some(7), 1).after(), None);
        assert_eq!(read(Some("reels:12"), None, 1).after(), None);
        assert!(read(Some("reels:12"), Some(7), 1).after().is_some());
    }

    #[test]
    fn no_page_is_read_on_past_the_most_pages_of_a_run() {
        assert_eq!(read(Some("reels:12"), Some(7), MOST_PAGES).after(), None);
        assert_eq!(
            read(Some("reels:12"), Some(7), MOST_PAGES + 1).after(),
            None
        );
        let last = read(Some("reels:12"), Some(7), MOST_PAGES - 1)
            .after()
            .expect("one page left");
        assert_eq!(last.pages, MOST_PAGES - 1);
    }

    #[test]
    fn reading_on_carries_the_cursor_and_whether_the_tab_was_opened() {
        let mut grid = read(Some("reels:15"), Some(7), 2);
        let after = grid.after().unwrap();
        assert_eq!(after.cursor, "reels:15");
        assert_eq!(after.tab, Some(ProfileTab::Reels));
        assert_eq!(after.pk, snob_core::Pk::new(7));
        assert_eq!(after.pages, 2);
        assert!(!after.tab_opened);
        grid.tab_opened = true;
        assert!(
            grid.after().unwrap().tab_still_opened(),
            "the tab is opened once"
        );
    }

    #[test]
    fn a_tab_opened_in_a_browser_idle_since_is_opened_again() {
        let mut grid = read(Some("reels:15"), Some(7), 2);
        grid.tab_opened = true;
        let mut after = grid.after().unwrap();
        let Some(long_ago) = Instant::now().checked_sub(crate::browser::headless::IDLE) else {
            return; // a clock too young to have been idle that long
        };
        after.asked_at = long_ago;
        assert!(!after.tab_still_opened());
    }
}
