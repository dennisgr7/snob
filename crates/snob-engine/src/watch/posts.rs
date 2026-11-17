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
use crate::exit::ExitCode;
use crate::posts::{self, Post};
use crate::watch::Watched;

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

/// How far past this machine's clock a `taken_at` may be and still become the
/// floor: a day.
///
/// The floor is Instagram's time and only rises, so one moment far ahead (a
/// unit changed on their side, a value misread) would stand later than every
/// post taken after it, and none would be news again. The widest honest
/// disagreement between two clocks is a time zone (a hardware clock holding
/// local time read as UTC: 14 h at most), which a day covers.
const AHEAD_AT_MOST: Duration = Duration::from_secs(24 * 60 * 60);

/// [`domain::compare`], with the floor on both sides of it held to
/// [`AHEAD_AT_MOST`] past `now`.
///
/// **A post dated later than that is still compared at the time it carries**,
/// so it is news once, as any post never seen and later than the floor; only
/// the floor does not follow it.
fn compare(
    known: &std::collections::HashSet<u64>,
    mark: Option<&watch_posts::Mark>,
    read: &domain::Read<'_>,
    now: Epoch,
) -> domain::Compared {
    let latest = now + AHEAD_AT_MOST;
    // A floor a version before this rule left past it says nothing: the
    // posts were all read when they were last read, by this clock.
    let before = mark.map(|mark| match mark.newest_taken_at {
        floor if floor > latest => mark.read_at.min(now),
        floor => floor,
    });
    let mut compared = domain::compare(known, before, read);
    compared.floor = read
        .posts
        .iter()
        .map(|post| post.taken_at)
        .filter(|taken_at| *taken_at <= latest)
        .chain(before)
        .max()
        .unwrap_or_default();
    compared
}

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
    // Asked as the lists asked, the own account (no name) included: what
    // their resolve remembered is reused for both.
    let opened = app.resolved_target(watched.name());

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
                .or_else(|| watched.name().map(|name| target::clean(name).to_string()))
                .or_else(|| app.viewer().username.clone())
                .unwrap_or_default();
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
            let info = app.profile_named(&typed, known).await?;
            let is_self = info.id == app.viewer().pk;
            // The account's row, which the report's and the run's rows hang
            // off, written as every read of an account writes it.
            users::upsert(
                app.db().conn(),
                &User::named(info.id, info.username.clone()),
            )?;
            accounts::upsert(app.db().conn(), info.id, is_self)?;
            if !target::shown_to(&info, app.viewer().pk) {
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
    let compared = compare(
        &known_posts,
        mark.as_ref(),
        &domain::Read {
            posts: &seen,
            ended,
            stopped_at_known: how.reached.is_some(),
        },
        snob_core::clock::now(),
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
    use crate::watch::PostsWatch;

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

    fn seen(pk: u64, taken_at: i64) -> domain::Seen {
        domain::Seen {
            pk,
            taken_at: Epoch::new(taken_at),
            pinned: false,
        }
    }

    fn mark(floor: i64, read_at: i64) -> watch_posts::Mark {
        watch_posts::Mark {
            account_pk: Pk::new(7),
            baseline_at: Epoch::new(0),
            read_at: Epoch::new(read_at),
            reported_at: None,
            newest_taken_at: Epoch::new(floor),
        }
    }

    fn read(posts: &[domain::Seen]) -> domain::Read<'_> {
        domain::Read {
            posts,
            ended: false,
            stopped_at_known: true,
        }
    }

    /// One post dated far ahead is reported once, as any new post is, and
    /// the floor does not follow it there: before, it became the floor, and
    /// no post taken after it was ever news again.
    #[test]
    fn a_post_dated_far_ahead_is_news_once_and_never_the_floor() {
        const NOW: i64 = 1_800_000_000;
        const FAR: i64 = 4_000_000_000;
        let now = Epoch::new(NOW);
        let known = |pks: &[u64]| pks.iter().copied().collect();

        let posts = [seen(9, FAR), seen(8, NOW - 50), seen(7, NOW - 100)];
        let first = compare(
            &known(&[7]),
            Some(&mark(NOW - 100, NOW - 3_600)),
            &read(&posts),
            now,
        );
        assert_eq!(first.news, vec![9, 8], "the post itself is not lost");
        assert_eq!(first.floor, Epoch::new(NOW - 50));

        // The next read, an hour on: the far one is known, and a post taken
        // since is news, which it is not under a floor that far ahead.
        let later = Epoch::new(NOW + 3_600);
        let posts = [seen(10, NOW + 1_800), seen(9, FAR), seen(8, NOW - 50)];
        let second = compare(
            &known(&[9, 8, 7]),
            Some(&mark(first.floor.get(), NOW)),
            &read(&posts),
            later,
        );
        assert_eq!(second.news, vec![10]);
        assert_eq!(second.floor, Epoch::new(NOW + 1_800));

        // A baseline does not take it for its floor either.
        let baseline = compare(
            &known(&[]),
            None,
            &read(&[seen(9, FAR), seen(8, NOW - 50)]),
            now,
        );
        assert!(baseline.news.is_empty());
        assert_eq!(baseline.floor, Epoch::new(NOW - 50));
        let nothing_else = compare(&known(&[]), None, &read(&[seen(9, FAR)]), now);
        assert_eq!(nothing_else.floor, Epoch::default());
    }

    /// A clock a time zone behind Instagram's is an honest disagreement: a
    /// post dated within a day ahead moves the floor as before.
    #[test]
    fn a_post_dated_within_a_day_ahead_still_moves_the_floor() {
        const NOW: i64 = 1_800_000_000;
        let ahead = NOW + 14 * 3_600;
        let compared = compare(
            &[7].into_iter().collect(),
            Some(&mark(NOW - 100, NOW - 3_600)),
            &read(&[seen(8, ahead), seen(7, NOW - 100)]),
            Epoch::new(NOW),
        );
        assert_eq!(compared.news, vec![8]);
        assert_eq!(compared.floor, Epoch::new(ahead));
        assert_eq!(AHEAD_AT_MOST, Duration::from_secs(86_400));
    }

    /// A floor stored far ahead by a version before the rule silences
    /// nothing more: the comparison starts from when the posts were last
    /// read instead.
    #[test]
    fn a_floor_stored_far_ahead_is_not_compared_against() {
        const NOW: i64 = 1_800_000_000;
        let stored = mark(4_000_000_000, NOW - 3_600);
        let posts = [seen(10, NOW - 60), seen(6, NOW - 7_200)];
        let compared = compare(
            &[5].into_iter().collect(),
            Some(&stored),
            &read(&posts),
            Epoch::new(NOW),
        );
        assert_eq!(compared.news, vec![10], "taken since the last read");
        assert_eq!(compared.floor, Epoch::new(NOW - 60));
    }

    /// The hour between two reads, and the page a read may stop at, as the
    /// header and their own comments say.
    #[test]
    fn the_posts_of_an_account_are_read_at_most_once_an_hour() {
        assert_eq!(READ_AT_MOST_EVERY, Duration::from_secs(3_600));
        assert_eq!(KNOWN_TO_STOP_AT, 12);
    }
}
