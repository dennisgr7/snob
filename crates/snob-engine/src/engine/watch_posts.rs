//! The monitor's look at a watched account's posts: what one run reads of the
//! grid, and what it may conclude from it.
//!
//! **The order is the rule**, and each step is asked before anything is sent:
//!
//! 1. An entry that does not watch posts reads nothing.
//! 2. A canceled run, or a critical battery, reads nothing.
//! 3. In a cooldown, or under the brake on every account, nothing is sent, not
//!    even the profile (`App::held`).
//! 4. Without a browser nothing is sent: the grid is a Relay query only the tab
//!    builds (`IgClient::profile_posts`), and the lists go on without it.
//! 5. An account whose posts were read less than [`READ_AT_MOST_EVERY`] ago is
//!    not read again.
//! 6. The profile, as the app opens one with the grid: reused when the lists of
//!    this run already opened it (`target::resolve`, through the memo
//!    `App::resolved_target`), a step after their last request; otherwise read
//!    here (`profile_named`).
//! 7. The grid: one page to lay a baseline; afterwards up to
//!    [`posts::UNTIL_PAGES`], stopping at the first page that holds a post seen
//!    before and not pinned (`posts::reached`), each page a step after the one
//!    before and past any rest the account's sitting owes, as `snob posts
//!    --until` reads.
//! 8. The comparison (`snob_core::watch::posts::compare`).
//!
//! Reading somebody's posts asks no consent (AGENTS.md, "Consent"): the consent
//! covers walking their followers or following.

use std::time::Duration;

use anyhow::Result;
use snob_core::model::{StopReason, User};
use snob_core::watch::posts::{self as domain, Basis};
use snob_core::{Epoch, Pk};
use snob_ig::pace::Pace;
use snob_ig::shortcode::MediaPk;
use snob_store::store::{accounts, users, watch_posts};

use crate::app::App;
use crate::engine::target;
use crate::engine::watch::Watched;
use crate::exit::ExitCode;
use crate::posts::{self, Post};

/// The shortest time between two reads of one account's grid: an hour.
///
/// The budget would allow one every quarter of an hour, which a timer asking
/// that often reaches, and that is 480 requests a day for one account; a
/// person who opens the same profile every fifteen minutes all day is not a
/// reader. Conservative and not measured: slower is always acceptable here.
pub const READ_AT_MOST_EVERY: Duration = Duration::from_secs(60 * 60);

/// How many of the posts seen before a read may stop at: the newest twelve, one
/// page.
pub const KNOWN_TO_STOP_AT: usize = 12;

/// Why a run drew no conclusion about an account's posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostsSkipped {
    /// A cooldown, or the brake on every account: nothing was sent.
    Cooldown,
    /// Read less than [`READ_AT_MOST_EVERY`] ago. Nothing is wrong.
    ReadRecently,
    /// The read stopped short: canceled, a critical battery, or a push-back,
    /// with what Instagram said when it said something.
    Incomplete(StopReason, Option<ExitCode>),
    /// A private account the viewer does not follow.
    Hidden,
    /// No browser to read the grid from (`SNOB_NO_BROWSER`).
    NoBrowser,
}

impl PostsSkipped {
    /// The word a report says it with (`run.posts.skipped`). Protocol.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cooldown | Self::ReadRecently => "not_verified",
            Self::Incomplete(..) => "incomplete",
            Self::Hidden => "hidden",
            Self::NoBrowser => "no_browser",
        }
    }
}

/// What a run concluded about an account's posts.
#[derive(Debug, Clone)]
pub struct PostsReport {
    pub basis: Basis,
    /// When the posts were last read and concluded on; `None` before the
    /// baseline.
    pub since: Option<Epoch>,
    /// When this read concluded.
    pub until: Epoch,
    /// The new posts, as the grid lists them, newest first.
    pub news: Vec<Post>,
    /// How many pages of the grid were read.
    pub pages: u32,
    /// Whether the read reached back to what was known (`domain::Compared`).
    pub complete: bool,
    /// What was saved of each new post, when the entry saves; filled in by
    /// the command that saves them, before the report is made.
    pub saved: Vec<SavedPost>,
}

/// What was saved of one post, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPost {
    pub pk: MediaPk,
    pub directory: String,
    /// The names of its files that are in the directory.
    pub files: Vec<String>,
    pub failed: usize,
    pub left: usize,
}

impl PostsReport {
    /// What was saved of `pk`, when anything was tried.
    pub fn saved_of(&self, pk: MediaPk) -> Option<&SavedPost> {
        self.saved.iter().find(|saved| saved.pk == pk)
    }
}

/// One run's look at an account's posts.
#[derive(Debug)]
pub struct PostsLook {
    /// Whose posts, when the profile said.
    pub pk: Option<Pk>,
    pub skipped: Option<PostsSkipped>,
    pub report: Option<PostsReport>,
    /// Every post read, for the saves owed to posts still on the page.
    pub read: Vec<Post>,
    /// What committing this look writes.
    pub(crate) commit: Option<watch_posts::Commit>,
}

impl PostsLook {
    fn skipped(pk: Option<Pk>, why: PostsSkipped) -> Self {
        Self {
            pk,
            skipped: Some(why),
            report: None,
            read: Vec::new(),
            commit: None,
        }
    }
}

/// Looks at `watched`'s posts, if it watches them, in the order the module
/// header gives.
pub async fn look(
    app: &mut App,
    watched: &Watched,
    after_lists: bool,
    battery_said: bool,
) -> Result<Option<PostsLook>> {
    let Some(entry) = watched.posts() else {
        return Ok(None);
    };
    let has_files = entry.save_to.is_some();

    let power_stop =
        !app.cancel().is_canceled() && app.stop_between_pages().is_some_and(|stop| stop());
    if app.cancel().is_canceled() || power_stop {
        if power_stop && !battery_said {
            app.warn(crate::sentences::BATTERY_CRITICAL_SKIP);
        }
        return Ok(Some(PostsLook::skipped(
            None,
            PostsSkipped::Incomplete(StopReason::Canceled, None),
        )));
    }
    if app.held()?.is_some() {
        return Ok(Some(PostsLook::skipped(None, PostsSkipped::Cooldown)));
    }
    if !app.client().has_page() {
        return Ok(Some(PostsLook::skipped(None, PostsSkipped::NoBrowser)));
    }

    let known = match watched.name() {
        None => Some(app.viewer().pk),
        Some(name) => target::known_pk(app, name)?,
    };
    let now = snob_core::clock::now();
    if let Some(pk) = known
        && let Some(mark) = watch_posts::mark(app.db().conn(), pk)?
        && now - mark.read_at < READ_AT_MOST_EVERY.as_secs() as i64
        && now >= mark.read_at
    {
        return Ok(Some(PostsLook::skipped(
            Some(pk),
            PostsSkipped::ReadRecently,
        )));
    }

    match read(app, watched, known, after_lists, has_files).await {
        Ok(look) => Ok(Some(look)),
        // A push-back ends the posts of this run, as it ends a list: the
        // transport has written the cooldown, and what the lists concluded is
        // still reported. Anything else is the run's.
        Err(e) if crate::exit::exit_code_for(&e) == ExitCode::RateLimited => {
            Ok(Some(PostsLook::skipped(
                known,
                PostsSkipped::Incomplete(StopReason::RateLimit, Some(ExitCode::RateLimited)),
            )))
        }
        Err(e) => Err(e),
    }
}

/// Steps 6 to 8: the profile, the grid, the comparison.
async fn read(
    app: &mut App,
    watched: &Watched,
    known: Option<Pk>,
    after_lists: bool,
    has_files: bool,
) -> Result<PostsLook> {
    let canceled = || PostsSkipped::Incomplete(StopReason::Canceled, None);
    let opened = watched
        .name()
        .and_then(|name| app.resolved_target(Some(name)));

    let (pk, username, declared) = match opened {
        // The lists of this run opened the profile: the grid follows it, a
        // step after their last request, as the app sends it with the
        // profile.
        Some(target) => {
            if app.client().step_between(&Pace::default()).await {
                return Ok(PostsLook::skipped(Some(target.pk), canceled()));
            }
            let name = target
                .username
                .unwrap_or_else(|| target::clean(watched.name().unwrap_or_default()).to_string());
            (target.pk, name, None)
        }
        None => {
            if after_lists && app.client().step_between(&Pace::default()).await {
                return Ok(PostsLook::skipped(known, canceled()));
            }
            let typed = match watched.name() {
                Some(name) => target::clean(name).to_string(),
                None => match app.viewer().username.clone() {
                    Some(own) => own,
                    None => app
                        .client()
                        .resolve_username(app.viewer().pk)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("your account's name is not known"))?,
                },
            };
            let info = app.client().profile_named(&typed, known).await?;
            let is_self = info.id == app.viewer().pk;
            // The account's row, which the report's and the run's rows hang
            // off, written as every read of an account writes it.
            users::upsert(
                app.db().conn(),
                &User::named(info.id, info.username.clone()),
            )?;
            accounts::upsert(app.db().conn(), info.id, is_self)?;
            let is_private = info.is_private.unwrap_or(false);
            let you_follow = info.followed_by_viewer.unwrap_or(false);
            if !(is_self || !is_private || you_follow) {
                return Ok(PostsLook::skipped(Some(info.id), PostsSkipped::Hidden));
            }
            (info.id, info.username, info.posts.map(|posts| posts.count))
        }
    };

    let conn = app.db().conn();
    let mark = watch_posts::mark(conn, pk)?;
    let until: Vec<MediaPk> = match mark {
        Some(_) => watch_posts::newest(conn, pk, KNOWN_TO_STOP_AT)?
            .into_iter()
            .map(MediaPk::new)
            .collect(),
        None => Vec::new(),
    };
    // One page lays a baseline; nothing to compare it with makes more a waste.
    let pages = if mark.is_some() {
        posts::UNTIL_PAGES
    } else {
        1
    };

    let (grid, how, ended) = if declared == Some(0) {
        // Zero is an answer the profile already gave.
        (Vec::new(), posts::GridRead::default(), true)
    } else {
        let (grid, how) = posts::read_pages(app.client(), &username, pages, 0, &until).await?;
        // A read cut short by Ctrl+C reached nowhere in particular, and the
        // floor must not move past what it did not read.
        if app.cancel().is_canceled() {
            return Ok(PostsLook::skipped(Some(pk), canceled()));
        }
        let ended = grid.next.is_none();
        (grid.posts, how, ended)
    };
    let conn = app.db().conn();

    let seen: Vec<domain::Seen> = grid
        .iter()
        .map(|post| domain::Seen {
            pk: post.pk.get(),
            taken_at: post.taken_at,
            pinned: post.pinned,
        })
        .collect();
    let pks: Vec<u64> = seen.iter().map(|post| post.pk).collect();
    let known_posts = watch_posts::known(conn, pk, &pks)?;
    let compared = domain::compare(
        &known_posts,
        mark.map(|mark| mark.newest_taken_at),
        &domain::Read {
            posts: &seen,
            ended,
            stopped_at_known: how.reached.is_some(),
        },
    );

    let news: Vec<Post> = grid
        .iter()
        .filter(|post| compared.news.contains(&post.pk.get()))
        .cloned()
        .collect();
    let commit = watch_posts::Commit {
        seen: grid
            .iter()
            .map(|post| watch_posts::Seen {
                media_pk: post.pk.get(),
                code: post.code.clone(),
                taken_at: post.taken_at,
                pinned: post.pinned,
            })
            .collect(),
        news: compared.news.clone(),
        saved: Vec::new(),
        floor: compared.floor,
        has_files,
    };

    Ok(PostsLook {
        pk: Some(pk),
        skipped: None,
        report: Some(PostsReport {
            basis: compared.basis,
            since: mark.map(|mark| mark.read_at),
            until: snob_core::clock::now(),
            news,
            pages: how.pages,
            complete: compared.complete,
            saved: Vec::new(),
        }),
        read: grid,
        commit: Some(commit),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::watch::PostsWatch;

    /// Without a browser the grid cannot be read, so the posts are skipped
    /// and said to be, and nothing at all is sent for them.
    #[tokio::test]
    async fn without_a_browser_posts_are_skipped_and_nothing_is_sent() {
        let server = wiremock::MockServer::start().await;
        let session = snob_core::session::Session::from_sessionid(
            "42%3AAbCdEfGh%3A20",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        let client = snob_ig::client::IgClient::new(session, snob_ig::pace::Pacer::unlimited())
            .unwrap()
            .with_base_url(url::Url::parse(&server.uri()).unwrap());
        let mut app = App::for_test(
            client,
            snob_store::store::Store::in_memory().unwrap(),
            crate::app::Viewer {
                pk: Pk::new(42),
                username: Some("me".into()),
            },
        );
        let watched =
            Watched::asking("someone".into()).watching(false, Some(PostsWatch::default()));

        let looked = look(&mut app, &watched, false, false)
            .await
            .unwrap()
            .expect("the posts are watched");
        assert_eq!(looked.skipped, Some(PostsSkipped::NoBrowser));
        assert!(looked.report.is_none());
        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty()
        );

        // And an entry that does not watch posts is not looked at at all.
        assert!(
            look(&mut app, &Watched::own(), false, false)
                .await
                .unwrap()
                .is_none()
        );
    }

    /// The hour between two reads, and the page a read may stop at, as the
    /// header and their own comments say.
    #[test]
    fn the_posts_of_an_account_are_read_at_most_once_an_hour() {
        assert_eq!(READ_AT_MOST_EVERY, Duration::from_secs(3_600));
        assert_eq!(KNOWN_TO_STOP_AT, 12);
    }
}
