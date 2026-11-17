//! The monitor: what a run found of an account it watches, and what it was
//! allowed to conclude from it.
//!
//! A run ([`tick`]) reads the account's lists as any command does, through
//! `engine::list`, and its posts ([`posts`]), compares them (`stored`) against
//! what was last reported, and is recorded once its report is queued ([`commit`]).
//! [`check`] says what a run would find, writing nothing; [`webhook`] gets a
//! report to wherever the person wants it.
//!
//! **The lists know nothing of the monitor**: `engine` names nothing here.

pub mod check;
pub mod posts;
mod stored;
mod tick;
pub mod webhook;

use anyhow::Result;
use snob_core::model::{ListKind, StopReason};
use snob_core::watch::{Basis, Changes, ListDiff, Rename};
use snob_core::{Epoch, Pk};
use snob_store::store::{accounts, snapshots, users, watch as store};

use self::posts::{PostsReport, PostsSkipped};
use crate::app::{App, Viewer};
use crate::engine::{self, ListOutcome, ListQuery, Provenance, target};
use crate::exit::ExitCode;

use stored::compare;
pub use stored::from_store;
#[doc(hidden)]
pub use stored::record_from_store;
#[cfg(test)]
use tick::refusal;
use tick::refused_outcome;
pub use tick::tick;

/// What one list has to report.
#[derive(Debug, Clone)]
pub struct ListReport {
    pub kind: ListKind,
    pub basis: Basis,
    /// When the last report about this list was made.
    pub since: Option<Epoch>,
    /// When the newest capture was taken.
    pub until: Epoch,
    pub diff: ListDiff,
    /// How many accounts the newest capture holds, so a report can say "three
    /// left, of a hundred and forty" without the caller counting again.
    pub total: usize,
}

impl ListReport {
    /// Whether this run established that the list is still true, which is what
    /// makes a rename among its members worth reporting.
    pub fn verified(&self) -> bool {
        matches!(self.basis, Basis::Compare { .. } | Basis::Unchanged { .. })
    }
}

/// Everything storage can say about one account.
#[derive(Debug, Clone)]
pub struct WatchReport {
    pub account_pk: Pk,
    /// The name, when one was ever learned.
    pub username: Option<String>,
    pub is_self: bool,
    /// `None` when nothing of that list has ever been walked to completion.
    pub followers: Option<ListReport>,
    pub following: Option<ListReport>,
    pub renamed: Vec<Rename>,
    /// What this run concluded about the account's posts; `None` when it
    /// does not watch them or concluded nothing.
    pub posts: Option<PostsReport>,
}

impl WatchReport {
    /// Whether the tick found anything, without cloning the diffs.
    pub fn has_changes(&self) -> bool {
        self.change_count() > 0
    }

    /// How many individual changes, counted the way `Changes::len` counts --
    /// both lists plus the renames -- without building a `Changes`, and the
    /// new posts beside them.
    pub fn change_count(&self) -> usize {
        self.list_change_count() + self.new_posts()
    }

    /// The changes of the lists and the renames alone: what a report's
    /// `counts.total` has always counted, and still does.
    pub fn list_change_count(&self) -> usize {
        let of = |kind| self.report(kind).map_or(0, |r| r.diff.len());
        of(ListKind::Followers) + of(ListKind::Following) + self.renamed.len()
    }

    /// How many new posts this run found.
    pub fn new_posts(&self) -> usize {
        self.posts.as_ref().map_or(0, |posts| posts.news.len())
    }

    /// The changes, in the shape the payload and the printer both want.
    pub fn changes(&self) -> Changes {
        Changes {
            followers: self.diff_of(ListKind::Followers),
            following: self.diff_of(ListKind::Following),
            renamed: self.renamed.clone(),
        }
    }

    fn diff_of(&self, kind: ListKind) -> ListDiff {
        self.report(kind)
            .map(|r| r.diff.clone())
            .unwrap_or_default()
    }

    pub fn report(&self, kind: ListKind) -> Option<&ListReport> {
        match kind {
            ListKind::Followers => self.followers.as_ref(),
            ListKind::Following => self.following.as_ref(),
        }
    }

    /// Whether anything at all has ever been walked for this account.
    pub fn has_anything_stored(&self) -> bool {
        self.followers.is_some() || self.following.is_some() || self.posts.is_some()
    }
}

/// How old a stored capture may be and still be served when the counter polled
/// this run says it has not moved, and the shortest gap between two walks of
/// one list when it has: a counter that moves is walked by the first run once
/// the day is up, not at once.
///
/// **It has to be longer than the time between two runs, or it never serves
/// anything**: the next run is due one interval after the previous one
/// finished, plus a jitter that only moves it later. A monitor that re-walks
/// lists on a timer is the most expensive shape this tool has, and Meta's paper
/// on its anti-scraping system (arXiv 2502.17693) weighs a request by the
/// accounts it returns.
const REUSE_WINDOW: std::time::Duration = engine::DEFAULT_MAX_AGE;

/// An account this monitor watches.
#[derive(Debug, Clone)]
pub struct Watched {
    /// The signed-in account this entry is read as.
    viewer: Option<Pk>,
    /// `None` is the viewer's own account: nothing to agree to.
    target: Option<String>,
    consent: Option<Consent>,
    /// Whether its followers and following are watched.
    lists: bool,
    /// Whether its new posts are, and what is done with them.
    posts: Option<PostsWatch>,
}

/// What is done with an account's new posts: `[account.posts]` in
/// `watch.toml`, the only place it comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostsWatch {
    /// Where their photos and videos are saved; `None`, nowhere.
    pub save_to: Option<std::path::PathBuf>,
    /// What a saved video is; `None`, the best ffmpeg can join when found.
    pub quality: Option<snob_store::config::VideoQuality>,
}

/// A recorded answer to "may this walk somebody else's lists?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Consent;

impl Watched {
    /// The account the session belongs to.
    pub fn own() -> Self {
        Self {
            viewer: None,
            target: None,
            consent: None,
            lists: true,
            posts: None,
        }
    }

    /// Somebody else, with the question still to be asked.
    pub fn asking(username: String) -> Self {
        Self {
            viewer: None,
            target: Some(username),
            consent: None,
            lists: true,
            posts: None,
        }
    }

    /// Somebody else, with an answer already on record.
    pub fn consented(username: String, consent: Consent) -> Self {
        Self {
            viewer: None,
            target: Some(username),
            consent: Some(consent),
            lists: true,
            posts: None,
        }
    }

    /// The same entry, read as `viewer`.
    #[must_use]
    pub fn read_as(self, viewer: Option<Pk>) -> Self {
        Self { viewer, ..self }
    }

    /// The same entry, watching its lists or not, and its posts or not: what
    /// its entry in `watch.toml` says.
    #[must_use]
    pub fn watching(self, lists: bool, posts: Option<PostsWatch>) -> Self {
        Self {
            lists,
            posts,
            ..self
        }
    }

    /// Whether its followers and following are watched.
    pub fn lists(&self) -> bool {
        self.lists
    }

    /// What is done with its new posts, when they are watched.
    pub fn posts(&self) -> Option<&PostsWatch> {
        self.posts.as_ref()
    }

    /// The signed-in account this is read as; `None` is the account in use.
    pub fn viewer(&self) -> Option<Pk> {
        self.viewer
    }

    /// Whose account this is, when it is not the viewer's.
    pub fn name(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// Whether this may run with nobody there to answer a question.
    ///
    /// **Or when its lists are not read at all**: the consent covers walking
    /// somebody's followers or following, and reading their posts asks for
    /// none, as `snob posts` does not (AGENTS.md, "Consent").
    pub fn may_run_unattended(&self) -> bool {
        self.target.is_none() || self.consent.is_some() || !self.lists
    }

    /// The arguments a tick runs a list command with.
    fn list_args(&self) -> ListQuery {
        ListQuery {
            target: self.target.clone(),
            yes: self.consent.is_some(),
            refresh: false,
            cache: false,
            // A stored capture whose counter has not moved is still current, so
            // reusing it is right and costs nothing.
            max_age: REUSE_WINDOW,
            no_resume: false,
            max_pages: None,
            // Pause without the warning: nobody is at a terminal to read it,
            // and a monitor never goes past the day's ceiling.
            over_budget: Some(snob_ig::pager::OverBudget::Pause),
            // One walk a day per list at most.
            walk_at_most_every: Some(REUSE_WINDOW),
        }
    }
}

/// The entries each viewer reads, in the order the file first names that viewer
/// and, within one, in the file's order.
pub fn group_by_viewer(watched: &[Watched]) -> Vec<(Option<Pk>, Vec<Watched>)> {
    let mut groups: Vec<(Option<Pk>, Vec<Watched>)> = Vec::new();
    for entry in watched {
        match groups
            .iter_mut()
            .find(|(viewer, _)| *viewer == entry.viewer)
        {
            Some((_, group)) => group.push(entry.clone()),
            None => groups.push((entry.viewer, vec![entry.clone()])),
        }
    }
    groups
}

/// One list, as this run found it.
#[derive(Debug)]
pub struct TickList {
    pub kind: ListKind,
    /// Why this list could not be compared, when it could not.
    pub skipped: Option<Skipped>,
}

/// When a run was due, began and ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunTiming {
    /// The moment the schedule said the run was for; `None` for one started
    /// by hand, which was due for nothing.
    pub due_at: Option<Epoch>,
    /// When the run began, before any of its work.
    pub began_at: Epoch,
    /// When this account's part of it ended, once it has.
    pub ended_at: Option<Epoch>,
    /// How late the schedule itself may start it: the jitter the loop rolls.
    pub allowed: std::time::Duration,
}

/// How far past its moment, beyond the jitter, a run may start and still be on
/// time: half an hour.
pub const LATE_AFTER: std::time::Duration = std::time::Duration::from_secs(30 * 60);

impl RunTiming {
    /// A run started by hand: due for nothing, never late.
    pub fn by_hand(began_at: Epoch) -> Self {
        Self {
            due_at: None,
            began_at,
            ended_at: None,
            allowed: std::time::Duration::ZERO,
        }
    }

    /// A run the schedule said was due at `due_at`, the jitter allowed.
    pub fn due(due_at: Option<Epoch>, began_at: Epoch, allowed: std::time::Duration) -> Self {
        Self {
            due_at,
            began_at,
            ended_at: None,
            allowed,
        }
    }

    /// How long after its moment it began, in seconds; 0 for a run that began
    /// on it or before, and `None` for one due for nothing.
    pub fn late_by(&self) -> Option<i64> {
        self.due_at.map(|due| (self.began_at - due).max(0))
    }

    /// Whether it began later than [`LATE_AFTER`] past its moment, beyond the
    /// jitter.
    pub fn late(&self) -> bool {
        let allowed = LATE_AFTER.saturating_add(self.allowed).as_secs();
        let allowed = i64::try_from(allowed).unwrap_or(i64::MAX);
        self.late_by().is_some_and(|late| late > allowed)
    }
}

/// The posts, as this run found them: why it drew no conclusion about them,
/// when it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickPosts {
    pub skipped: Option<PostsSkipped>,
}

/// Why a run declined to draw a conclusion from a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    /// Nothing in this run established that the list still describes the
    /// account.
    NobodyLooked(Provenance),
    /// The walk did not finish, so accounts are missing from it — and every one
    /// of them would be reported as somebody who left.
    Incomplete(StopReason, Option<ExitCode>),
}

/// What a tick did.
#[derive(Debug)]
pub struct TickReport {
    /// The signed-in account this run read as.
    pub viewer: Viewer,
    pub report: WatchReport,
    pub requests: u32,
    pub lists: Vec<TickList>,
    /// The posts, when the account's are watched.
    pub posts: Option<TickPosts>,
    /// When the run was due, began and ended, as whoever started it said.
    pub timing: Option<RunTiming>,
    /// Every post this run read off the grid, for the saves still owed to
    /// one that is on it.
    pub read_posts: Vec<crate::posts::Post>,
    /// What committing the posts writes, when the run concluded on them.
    posts_commit: Option<snob_store::store::watch_posts::Commit>,
    /// The captures this report spoke about, and the moment it covers.
    committable: Vec<(ListKind, i64)>,
    at: Epoch,
    /// How far along the rename history this report has covered, when it read a
    /// window at all. `None` means the cursor must not move.
    rename_cursor: Option<i64>,
    /// The `username_history` rows this tick is announcing, so no later one
    /// repeats them.
    renames_sent: Vec<i64>,
}

/// Records that this report has been reported.
pub fn commit(
    app: &mut App,
    tick: &TickReport,
    delivery: Option<store::Queued<'_>>,
) -> Result<Option<i64>> {
    let pk = tick.report.account_pk;
    let at = tick.at;
    let (_, db, _) = app.parts();

    let queued = store::commit_report(
        db,
        pk,
        &tick.committable,
        at,
        tick.rename_cursor,
        &tick.renames_sent,
        tick.posts_commit.as_ref(),
        // The store's own type, not a copy mapped across: the compiler would
        // say nothing about a field added here that a mapper drops.
        delivery,
    )?;

    // Recorded whatever came of it, including a run that concluded nothing.
    let record = store::record_run(
        db.conn(),
        &store::Run {
            account_pk: pk,
            started_at: at,
            finished_at: Some(snob_core::clock::now()),
            requests: tick.requests,
            outcome: Some(tick.outcome().into()),
            changes: tick.report.change_count() as u32,
            posts_skipped: tick
                .posts_skipped()
                .map(|skipped| skipped.as_str().to_string()),
            due_at: tick.timing.and_then(|timing| timing.due_at),
            began_at: tick.timing.map(|timing| timing.began_at),
        },
    );
    if let Err(e) = record {
        tracing::warn!(error = %e, "the run could not be recorded");
    }

    Ok(queued)
}

impl TickReport {
    /// A report that never ran, for a test that only cares what it looks like,
    /// read as the account it reports on.
    #[doc(hidden)]
    pub fn for_test(report: WatchReport, requests: u32, at: Epoch) -> Self {
        Self {
            viewer: Viewer {
                pk: report.account_pk,
                username: report.username.clone(),
            },
            report,
            requests,
            lists: Vec::new(),
            posts: None,
            timing: None,
            read_posts: Vec::new(),
            posts_commit: None,
            committable: Vec::new(),
            at,
            rename_cursor: None,
            renames_sent: Vec::new(),
        }
    }

    /// When this run concluded.
    pub fn at(&self) -> Epoch {
        self.at
    }

    /// Whether this run established anything at all.
    pub fn looked(&self) -> bool {
        self.lists.iter().any(|l| l.skipped.is_none())
            || self.posts.is_some_and(|posts| posts.skipped.is_none())
    }

    /// Why the posts were not looked at, when they are watched and were not.
    pub fn posts_skipped(&self) -> Option<PostsSkipped> {
        self.posts.and_then(|posts| posts.skipped)
    }

    /// Records that every file of `saved` is now where the entry keeps them,
    /// and what the report says of each post.
    pub fn record_saves(
        &mut self,
        done: Vec<snob_ig::shortcode::MediaPk>,
        saved: Vec<posts::SavedPost>,
    ) {
        if let Some(commit) = &mut self.posts_commit {
            commit.saved.extend(done.into_iter().map(|pk| pk.get()));
        }
        if let Some(posts) = &mut self.report.posts {
            posts.saved = saved;
        }
    }

    /// What this run amounts to, in the vocabulary `$?` and the README's table
    /// already use.
    pub fn outcome(&self) -> ExitCode {
        if self.looked() {
            return ExitCode::Ok;
        }
        // Posts alone, read within the hour: nothing was due, and nothing is
        // wrong.
        if self.lists.is_empty() && self.posts_skipped() == Some(PostsSkipped::ReadRecently) {
            return ExitCode::Ok;
        }
        refused_outcome(&self.lists, self.posts_skipped())
    }
}

/// A comparison, and what committing it would have to write.
struct Compared {
    report: WatchReport,
    marks: Vec<(ListKind, i64)>,
    rename_cursor: Option<i64>,
    renames_sent: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(provenance: Provenance, reason: StopReason) -> ListOutcome {
        ListOutcome {
            requests: 0,
            started_at: Epoch::new(1_000),
            taken_at: Epoch::new(1_100),
            account_pk: Pk::new(42),
            snapshot_id: 7,
            ..ListOutcome::for_test(provenance, reason)
        }
    }

    /// A capture with an unchanged counter is served on the schedule people
    /// actually write: the next run is due an interval after the previous one
    /// plus a jitter that only adds, so a window equal to the interval would
    /// always be just too short. Written against the schedule itself, so a
    /// longer jitter or a shorter window fails here.
    #[test]
    fn an_unchanged_capture_outlives_the_gap_to_the_next_run() {
        assert_eq!(REUSE_WINDOW, std::time::Duration::from_secs(24 * 3600));
        assert_eq!(
            watched_own_max_age(),
            REUSE_WINDOW,
            "the tick is what reads it"
        );

        let every = std::time::Duration::from_secs(6 * 3600);
        let schedule = snob_core::watch::schedule::Schedule::every(every).unwrap();
        // A list's capture is taken when its walk ends, and the run then walks
        // the other list before it finishes. The longest that second walk can
        // be without pausing for the day is a day's accounts at the slowest
        // the default pace allows: 2,000 at 12 a page is 167 pages of up to
        // 3 s, plus four sitting rests of up to 15 minutes and the look at
        // the profile before the first page, about 70 minutes.
        let pace = snob_ig::pace::Pace::default();
        let accounts = snob_core::budget::accounts_per_day(None, snob_core::clock::EpochMs::new(0));
        let pages = u64::from(accounts.div_ceil(pace.per_page));
        let rests = pages / u64::from(pace.pages_per_sitting);
        let longest_walk = std::time::Duration::from_millis(
            pace.dwell_ms.1 + pages * pace.step_ms.1 + rests * pace.sitting_pause_ms.1,
        );
        let worst_gap = every + schedule.jitter() + longest_walk;
        assert!(
            REUSE_WINDOW > worst_gap,
            "a capture taken on one run has to still be servable on the next: {worst_gap:?}"
        );
    }

    fn watched_own_max_age() -> std::time::Duration {
        Watched::own().list_args().max_age
    }

    /// A walk that came back short is refused, and refused as *incomplete*
    /// rather than as unverified.
    ///
    /// The two halves of `refusal` mean different things to the caller: nobody
    /// looked is something the next run fixes, while a truncated walk is a list
    /// whose missing accounts would read as people who left.
    #[test]
    fn a_walk_that_came_back_short_is_refused_as_incomplete() {
        for reason in [
            StopReason::RateLimit,
            StopReason::Truncated,
            StopReason::Canceled,
            StopReason::PageLimit,
            StopReason::Network,
            StopReason::SessionInvalid,
        ] {
            assert!(
                matches!(
                    refusal(&outcome(Provenance::Walked, reason)),
                    Some(Skipped::Incomplete(got, _)) if got == reason
                ),
                "a walk that ended {reason:?} was accepted as a basis for comparison"
            );
        }

        // A list nothing verified is the other refusal, and it wins: the run
        // never looked, so what the stored capture says about completeness is
        // beside the point.
        assert!(matches!(
            refusal(&outcome(Provenance::Cooldown, StopReason::Completed)),
            Some(Skipped::NobodyLooked(Provenance::Cooldown))
        ));

        assert!(
            refusal(&outcome(Provenance::Walked, StopReason::Completed)).is_none(),
            "a complete walk this run made is exactly what may be compared"
        );
    }

    /// And the run says so in the one place a timer can read without parsing
    /// English.
    #[test]
    fn a_run_whose_lists_all_came_back_short_does_not_exit_zero() {
        let report = WatchReport {
            account_pk: Pk::new(42),
            username: None,
            is_self: true,
            followers: None,
            following: None,
            renamed: Vec::new(),
            posts: None,
        };
        let mut tick = TickReport::for_test(report, 1, Epoch::default());
        tick.lists = vec![TickList {
            kind: ListKind::Followers,
            skipped: Some(Skipped::Incomplete(
                StopReason::RateLimit,
                Some(ExitCode::RateLimited),
            )),
        }];

        assert!(!tick.looked());
        assert_eq!(
            tick.outcome(),
            crate::exit::from_stop_reason(StopReason::RateLimit)
        );
    }

    /// A run is late past half an hour beyond its jitter, and never when it
    /// was due for nothing.
    #[test]
    fn a_run_is_late_past_half_an_hour_beyond_its_jitter() {
        let due = Epoch::new(1_700_000_000);
        let minutes = |n: u64| std::time::Duration::from_secs(n * 60);
        let on_time = RunTiming::due(Some(due), due + minutes(29), std::time::Duration::ZERO);
        assert_eq!(on_time.late_by(), Some(29 * 60));
        assert!(!on_time.late(), "a timer's quarter of an hour, and some");
        let late = RunTiming::due(Some(due), due + minutes(31), std::time::Duration::ZERO);
        assert!(late.late());
        let jittered = RunTiming::due(Some(due), due + minutes(40), minutes(15));
        assert!(
            !jittered.late(),
            "the jitter the loop rolled is no lateness"
        );
        assert!(!RunTiming::by_hand(due + minutes(600)).late());
        assert_eq!(RunTiming::by_hand(due).late_by(), None);
        let early = RunTiming::due(Some(due), due - minutes(1), std::time::Duration::ZERO);
        assert_eq!(early.late_by(), Some(0));
    }

    /// And a checkpoint is reported as a checkpoint, not as "log in again":
    /// `StopReason::SessionInvalid` covers exit codes 3 and 4, so the code
    /// Instagram's answer carried wins over the one rebuilt from the reason. A 3
    /// sends an unattended operator to `snob login`, which during the cooldown a
    /// challenge causes stores a session without validating it and says
    /// "Session stored".
    #[test]
    fn a_challenge_keeps_its_own_code_through_a_tick() {
        let short = |said| {
            let report = WatchReport {
                account_pk: Pk::new(42),
                username: None,
                is_self: true,
                followers: None,
                following: None,
                renamed: Vec::new(),
                posts: None,
            };
            let mut tick = TickReport::for_test(report, 1, Epoch::default());
            tick.lists = vec![TickList {
                kind: ListKind::Followers,
                skipped: Some(Skipped::Incomplete(StopReason::SessionInvalid, said)),
            }];
            tick
        };

        assert_eq!(
            short(Some(ExitCode::Challenge)).outcome(),
            ExitCode::Challenge,
            "what Instagram said beats what the store had to record"
        );
        assert_eq!(
            short(Some(ExitCode::NoSession)).outcome(),
            ExitCode::NoSession,
            "and the other half of the same reason still answers 3"
        );
        assert_eq!(
            short(None).outcome(),
            crate::exit::from_stop_reason(StopReason::SessionInvalid),
            "with nothing said, the reason is the whole answer"
        );
    }
}
