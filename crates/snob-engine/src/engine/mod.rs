//! Getting a list, and everything that decides how.

/// How old a stored capture whose counter has not moved may be and still be
/// served: a day.
pub const DEFAULT_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

pub mod cooldown;
pub mod estimate;
pub mod freshness;
pub mod people;
pub mod retention;
pub mod target;
pub mod walk;

use anyhow::Result;
use snob_core::model::{ListKind, StopReason, User};
use snob_core::{Epoch, Pk};
use snob_store::store::{accounts, snapshots, users};

use crate::app::App;
use crate::exit::ExitCode;

/// Where a returned list came from, in the sense that decides whether two of
/// them may be crossed against each other.
///
/// It is whether anything in **this run** established that it still describes
/// the account: two lists a month apart are fine to cross if both counters were
/// checked just now and neither had moved, and two lists an hour apart are not
/// fine if nobody looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Walked in this run. It is the account as it is.
    Walked,
    /// Stored, and a counter poll in this run said it had not moved.
    CounterVerified,
    /// Stored, served during a cooldown. Nothing may be spent to check.
    Cooldown,
    /// Stored, served because the counter poll failed. No evidence either way.
    PollFailed,
    /// Stored, served because `--offline` said not to look.
    CacheFlag,
    /// Stored, served because the counter moved but the list was walked less
    /// than [`ListQuery::walk_at_most_every`] ago.
    WalkedRecently,
}

impl Provenance {
    /// Whether this list is known to describe the account as it is now.
    pub fn describes_now(self) -> bool {
        matches!(self, Self::Walked | Self::CounterVerified)
    }

    /// The word a program reads this by (`--progress-json`'s `served` event,
    /// which `snob mcp` hands an agent as `served`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Walked => "walked",
            Self::CounterVerified => "stored_unchanged",
            Self::Cooldown => "stored_in_cooldown",
            Self::PollFailed => "stored_after_failed_poll",
            Self::CacheFlag => "stored",
            Self::WalkedRecently => "stored_walked_recently",
        }
    }
}

#[derive(Debug)]
pub struct ListOutcome {
    /// How this list was obtained, and therefore what is known about whether it
    /// is still true.
    pub provenance: Provenance,
    pub reason: StopReason,
    pub requests: u32,
    /// When the walk that produced this list **began**.
    pub started_at: Epoch,
    /// When it finished.
    pub taken_at: Epoch,
    /// Whose list this is.
    pub account_pk: Pk,
    /// The stored capture these users came out of.
    pub snapshot_id: i64,
    /// What Instagram actually said, when a walk stopped because it said
    /// something.
    pub stopped_by: Option<ExitCode>,
    /// Whether a second run would continue this walk rather than start it
    /// again.
    pub resumable: bool,
    /// How many follows and unfollows the account made through snob since a
    /// stored list of its own following was walked, which it is handed out with
    /// applied (`serve_stored`).
    pub own_changes: usize,
}

impl ListOutcome {
    /// What a stored snapshot answers with.
    pub(crate) fn cached(snapshot: &snapshots::Snapshot, provenance: Provenance) -> Self {
        debug_assert!(
            !matches!(provenance, Provenance::Walked),
            "a stored list was not walked"
        );
        Self {
            provenance,
            reason: StopReason::Completed,
            // Filled in by `list`, which is the only place that sees the whole
            // run and can ask the pacer what it really charged.
            requests: 0,
            started_at: snapshot.started_at,
            // The view this comes from cannot return an open snapshot, so the
            // fallback is unreachable.
            taken_at: snapshot.taken_at.unwrap_or_default(),
            account_pk: snapshot.account_pk,
            snapshot_id: snapshot.id,
            stopped_by: None,
            // A stored list is a finished one — the view this comes from cannot
            // return anything else — so there is nothing left to continue.
            resumable: false,
            // `serve_stored` counts them once it has applied them.
            own_changes: 0,
        }
    }

    /// Whether this list came out of storage rather than a walk in this run:
    /// every provenance but `Walked`, `CounterVerified` included.
    pub fn is_stored(&self) -> bool {
        self.provenance != Provenance::Walked
    }

    pub fn is_complete(&self) -> bool {
        self.reason.yields_complete_list()
    }

    /// The code a command should exit with when it has to refuse this result.
    pub fn exit_code(&self) -> ExitCode {
        self.stopped_by
            .unwrap_or_else(|| crate::exit::from_stop_reason(self.reason))
    }

    /// The code for a result that **was printed** out of this list, short or
    /// not.
    pub fn exit_code_for_a_printed_result(&self) -> ExitCode {
        match self.reason {
            StopReason::Completed | StopReason::PageLimit => ExitCode::Ok,
            _ => self.exit_code(),
        }
    }

    /// A walked-or-stored outcome with every other field at its plainest: one
    /// request, epoch zero, account and capture 1.
    #[doc(hidden)]
    pub fn for_test(provenance: Provenance, reason: StopReason) -> Self {
        Self {
            provenance,
            reason,
            requests: 1,
            started_at: Epoch::default(),
            taken_at: Epoch::default(),
            account_pk: Pk::new(1),
            snapshot_id: 1,
            stopped_by: None,
            resumable: false,
            own_changes: 0,
        }
    }

    /// Whether this is the list of the account the run acts as.
    pub fn is_own(&self, viewer: &crate::app::Viewer) -> bool {
        self.account_pk == viewer.pk
    }
}

/// What a list is asked for: which account, and how the answer may be got.
#[derive(Debug, Clone)]
pub struct ListQuery {
    /// The account, as typed. `None` is the session's own.
    pub target: Option<String>,
    /// Consent given in advance, for a list that is somebody else's.
    pub yes: bool,
    /// Walk even when storage could answer.
    pub refresh: bool,
    /// Answer out of storage and spend nothing.
    pub cache: bool,
    /// How old a stored list may be and still be served.
    pub max_age: std::time::Duration,
    /// Start over rather than continue an interrupted walk.
    pub no_resume: bool,
    /// Stop after this many pages.
    pub max_pages: Option<u32>,
    /// What a walk does when the day's accounts run out before the list does.
    pub over_budget: Option<snob_ig::pager::OverBudget>,
    /// The shortest gap between two walks of one list, whatever the counter
    /// says. `None` walks whenever the stored list no longer answers.
    pub walk_at_most_every: Option<std::time::Duration>,
}

/// Gets one list, deciding along the way whether anything needs fetching.
pub async fn list(
    app: &mut App,
    args: &ListQuery,
    kind: ListKind,
) -> Result<(Vec<User>, ListOutcome)> {
    // Measured rather than added up along the way.
    let before = app.client().pacer().spent();
    let (users, mut outcome) = decide(app, args, kind).await?;
    outcome.requests = app.client().pacer().spent().saturating_sub(before);
    Ok((users, outcome))
}

async fn decide(
    app: &mut App,
    args: &ListQuery,
    kind: ListKind,
) -> Result<(Vec<User>, ListOutcome)> {
    if let Some(held) = app.held()? {
        return cooldown::serve(app, args, kind, &held);
    }

    // **`--offline` is not asked about**, because there is nothing to agree to:
    // consent governs enumerating somebody else's lists, and this reads a list
    // that was already walked — with permission — off this machine's own disk.
    if !args.cache {
        let someone_is_there = app.can_ask();
        ask_consent_with(app, args, someone_is_there).await?;
    }

    // The check at the top cannot see a cooldown that landed while the
    // confirmation prompt was open — one written by the service sharing this
    // database, for instance — and resolving is itself a request, so the
    // second check comes here, before `target::resolve`: in cooldown nothing
    // may be spent, not even the counter poll.
    if let Some(held) = app.held()? {
        return cooldown::serve(app, args, kind, &held);
    }

    // A crossing asks for two lists, and resolving is a request.
    let target = match app.resolved_target(args.target.as_deref()) {
        Some(target) => target,
        None => {
            let target = if args.cache {
                target::from_store(app, args.target.as_deref(), kind)?
            } else {
                target::resolve(app, args).await?
            };
            app.remember_target(args.target.as_deref(), target.clone());
            target
        }
    };

    // The user row has to exist before the account row: `accounts.pk`
    // references `users.pk`.
    match target.username.as_deref() {
        Some(username) => {
            users::upsert(app.db().conn(), &User::named(target.pk, username)).map(|_| ())?
        }
        None => users::ensure(app.db().conn(), target.pk)?,
    }
    accounts::upsert(app.db().conn(), target.pk, target.is_self)?;

    let stored = snob_store::store::snapshots::latest_complete(app.db().conn(), target.pk, kind)?;

    if args.cache {
        let Some(snapshot) = stored else {
            return Err(crate::sentences::refuse_nothing_stored(
                kind,
                args.target.as_deref(),
            ));
        };
        // `--offline` is a promise not to spend a request, so nothing here
        // checked whether the stored list is still true.
        return serve_stored(app, &snapshot, Provenance::CacheFlag);
    }

    freshness::decide_and_fetch(app, args, kind, &target, stored).await
}

/// A stored capture as the answer: its members, and where they came from.
///
/// **The account's own following is handed out with the follows and unfollows
/// it made through snob since the walk applied**
/// (`snob_store::store::own_writes`), and says so, naming them: the list is
/// still the walk's, of its date and provenance, plus what this computer knows
/// it changed, having sent it.
pub(super) fn serve_stored(
    app: &App,
    snapshot: &snapshots::Snapshot,
    provenance: Provenance,
) -> Result<(Vec<User>, ListOutcome)> {
    let mut members = snapshots::members(app.db().conn(), snapshot.id)?;
    let mut outcome = ListOutcome::cached(snapshot, provenance);
    if snapshot.kind == ListKind::Following && snapshot.account_pk == app.viewer().pk {
        let applied =
            snob_store::store::own_writes::apply(app.db().conn(), &mut members, outcome.taken_at)?;
        if !applied.is_empty() {
            app.warn(&crate::sentences::own_writes_applied(
                outcome.taken_at,
                &applied,
            ));
            outcome.own_changes = applied.len();
        }
    }
    Ok((members, outcome))
}

/// Asks before enumerating somebody else's account, at most once per run.
///
/// It happens **before** the account is resolved, because resolving is already
/// a request: asking afterwards means that answering "no" has still spent one
/// on a run the user never authorized.
///
/// **The third argument is for tests only**; `decide` passes
/// `App::can_ask`.
#[doc(hidden)]
pub async fn ask_consent_with(
    app: &mut App,
    args: &ListQuery,
    someone_is_there: bool,
) -> Result<()> {
    let Some(typed) = args.target.as_deref() else {
        return Ok(()); // your own account, nothing to agree to
    };
    // Cleaned before it is used as a key, so the answer is filed under the
    // account rather than under a spelling of it.
    let name = target::clean(typed);
    if args.yes || app.has_consent(name) {
        return Ok(());
    }

    let is_own_name = app
        .viewer()
        .username
        .as_deref()
        .is_some_and(|mine| mine.eq_ignore_ascii_case(name));
    if is_own_name {
        return Ok(());
    }

    let shown = target::label(app, args.target.as_deref());

    // Being unable to ask and being told no are two different events: `confirm`
    // answers with its default the moment nobody can answer, which would blame
    // the user for a "no" nobody gave.
    if !someone_is_there {
        return Err(crate::sentences::refuse_unconsented(
            &shown,
            app.consent_in_advance(),
        ));
    }

    app.warn(crate::sentences::READING_SOMEBODY_ELSES_LIST);
    if !app
        .confirm(crate::sentences::ask_to_continue(&shown))
        .await?
    {
        return Err(crate::sentences::refuse_declined(&shown));
    }
    app.record_consent(name);
    Ok(())
}

/// Pays a rest the account's sitting owes before the next page of `of`, and
/// says so ([`crate::host::info`]): a page read with no bar of its own would
/// otherwise stand still for minutes. `true` when the wait was stopped.
pub(crate) async fn rest_before_a_page(
    client: &snob_ig::client::IgClient,
    of: &str,
) -> Result<bool> {
    Ok(client
        .rest_before_a_page(&snob_ig::pace::Pace::default(), |left| {
            crate::host::info(&format!(
                "Resting {} between sittings before the next page of {of}.",
                crate::sentences::rest_left(left)
            ));
        })
        .await?)
}

/// A complete stored capture of account 1's followers, the row the storage
/// paths serve from.
#[cfg(test)]
pub(crate) fn stored_snapshot(
    started_at: Epoch,
    taken_at: Epoch,
    declared_count: Option<u64>,
) -> snapshots::Snapshot {
    snapshots::Snapshot {
        id: 1,
        account_pk: Pk::new(1),
        kind: ListKind::Followers,
        started_at,
        taken_at: Some(taken_at),
        member_count: 0,
        declared_count,
        next_cursor: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_outcome_is_complete_by_construction() {
        let snapshot = snapshots::Snapshot {
            account_pk: Pk::new(7),
            ..stored_snapshot(Epoch::default(), Epoch::default(), None)
        };
        let outcome = ListOutcome::cached(&snapshot, Provenance::CounterVerified);
        assert!(outcome.is_complete());
        assert!(outcome.is_stored());
        // Both ends of the interval come off the row, so they cannot disagree
        // with the members read from the same one.
        assert_eq!(outcome.account_pk, Pk::new(7));
    }

    /// A cap the user asked for is not a failure, on either side of a
    /// crossing (`cli.rs` promises 0 for it), and neither is anything served
    /// from storage. Everything else keeps the code that says what happened,
    /// so a script can tell "wait" from "log in again".
    #[test]
    fn the_exit_code_reports_what_stopped_the_walk() {
        let exit_code = |provenance, reason| {
            ListOutcome::for_test(provenance, reason).exit_code_for_a_printed_result()
        };
        let walked = |reason| exit_code(Provenance::Walked, reason);
        for reason in [StopReason::Completed, StopReason::PageLimit] {
            assert_eq!(walked(reason), ExitCode::Ok);
        }
        assert_eq!(walked(StopReason::RateLimit), ExitCode::RateLimited);
        assert_eq!(walked(StopReason::Canceled), ExitCode::Interrupted);
        assert_eq!(walked(StopReason::SessionInvalid), ExitCode::NoSession);
        // A plain list prints what it got and still exits with what stopped
        // it: the truncation wall is not a success.
        for stopped in [StopReason::Truncated, StopReason::Network] {
            assert_eq!(
                walked(stopped),
                ExitCode::Error,
                "{stopped:?} is not something the user asked for"
            );
        }
        // A stored list is a stored list, whatever ended the walk that made it.
        assert_eq!(
            exit_code(Provenance::CounterVerified, StopReason::Completed),
            ExitCode::Ok
        );
    }
}
