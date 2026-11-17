//! A profile's grid and its tabs, a page of twelve at a time: the profile,
//! then each page a step after the one before, as far as the reader asked
//! or to a post it already has.

use anyhow::Result;
use snob_ig::client::{IgClient, ProfileTab};
use snob_ig::pace::Pace;
use snob_ig::shortcode::MediaPk;

use super::{MOST_PAGES, Post, rest_before_a_page};

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
    let info = client
        .profile_named(crate::engine::target::clean(typed), known)
        .await?;
    if !crate::engine::target::shown_to(&info, viewer) {
        return Ok((
            Fetched::Hidden {
                username: info.username,
            },
            GridRead::default(),
        ));
    }
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
    let info = client
        .profile_named(crate::engine::target::clean(typed), known)
        .await?;
    if !crate::engine::target::shown_to(&info, viewer) {
        return Ok((
            Fetched::Hidden {
                username: info.username,
            },
            GridRead::default(),
        ));
    }
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
/// shown, with no resolve of its own.
pub async fn first_page(
    client: &IgClient,
    username: &str,
    pk: snob_core::Pk,
    tab: Option<ProfileTab>,
) -> Result<Grid> {
    let mut grid = Grid::of(username, tab, Some(pk));
    let first = match tab {
        None => client.profile_posts(username, None).await?,
        Some(tab) => client.profile_tab(tab, pk, username, None).await?,
    };
    grid.take(first);
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
}
