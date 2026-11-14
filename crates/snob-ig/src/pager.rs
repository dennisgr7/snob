//! Paginated walk over a followers or following list, with every stop
//! condition explicit: nothing here answers a failure with `continue`.

use std::collections::HashSet;
use std::error::Error;
use std::time::Duration;

use snob_core::model::StopReason;
use snob_core::{EpochMs, Pk};

use crate::awake::Asleep;
use crate::client::{Direction, IgClient};
use crate::error::{IgError, Reaction};
use crate::model::FriendshipsPage;
use crate::pace::{CancelToken, Pace};

/// No failure may produce an endless stream of requests, whatever happens to
/// the other stop conditions.
pub const HARD_PAGE_CAP: u32 = 2_000;

/// Past this **declared** size (never the walked one), falling short reads as
/// Instagram truncating the list. A guess from unverified reports (AGENTS.md,
/// "Known walls"), which is why both numbers are logged.
pub(crate) const TRUNCATION_THRESHOLD: usize = 10_000;

/// Pages in a row without a single new account before the list is taken to
/// be going round in circles.
const MAX_PAGES_WITHOUT_NEW: u32 = 3;

/// The share of a walk's rows, in percent, that may be accounts the same walk
/// already received before the list is not believed complete: repeats sit
/// where accounts never served would have been. The owner's number, with no
/// minimum sample.
pub(crate) const REPEAT_LIMIT_PERCENT: usize = 5;

/// The waits a walk pays. The request budget's own wait is inside the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// Before the walk's first page: [`Pace::dwell`].
    Dwell,
    /// Between two pages: [`Pace::step`].
    Step,
    /// What is left of the rest a full sitting owes ([`Pace::sitting_pause`]),
    /// paid before the next page of any walk of the account.
    Sitting,
    /// The day's accounts ran out: until the last 24 hours have room for
    /// `PAGES_AFTER_A_PAUSE` more pages.
    Day,
    /// The machine slept and the network is not back yet
    /// ([`crate::pace::RECONNECT_WAITS_SECS`]).
    Reconnect,
}

/// Pages the day must have room for before a walk paused on its accounts
/// wakes up, so it does not wake for every page as the day frees up.
pub(crate) const PAGES_AFTER_A_PAUSE: u32 = 8;

/// Well inside the claim's lifetime in `snob-store`'s `snapshots`, so a
/// sleeping walk is never taken for a dead one.
const HEARTBEAT: Duration = Duration::from_secs(10 * 60);

/// Called every [`HEARTBEAT`] by a walk asleep on the day's accounts: `false`
/// means the pages are no longer wanted, and the walk stops.
pub type Heartbeat<'a> = &'a dyn Fn() -> Result<bool, Box<dyn Error + Send + Sync>>;

/// What happens as the walk proceeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Started {
        estimated: Option<u64>,
        resumed: bool,
    },
    Page {
        number: u32,
        received: usize,
        added: usize,
        running_total: usize,
        /// The account's sitting as this page left it.
        sitting: snob_core::budget::Sitting,
    },
    Waiting {
        kind: WaitKind,
        duration: Duration,
    },
    Retrying {
        attempt: u32,
        after: Duration,
        error: String,
    },
    Warning(Warning),
    /// The machine slept for `slept` mid-walk; the walk reloads the page and
    /// opens the list again.
    Resumed {
        slept: Duration,
    },
    Finished {
        pages: u32,
        users: usize,
        reason: StopReason,
    },
}

/// Something the walk noticed, for the caller to word
/// (`snob-cli`'s `report::pager_warning`). All but `ShortOfDeclared` end the
/// walk as [`StopReason::Truncated`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    /// The cursor came back the same as the one just sent.
    SameCursorTwice,
    TwoEmptyPages,
    /// [`MAX_PAGES_WITHOUT_NEW`] pages in a row carried only accounts already
    /// walked.
    GoingInCircles,
    /// One empty page, and no counter to tell an account with no followers
    /// from one Instagram would not serve.
    EmptyAndNoCounter,
    /// A clean end far short of what the profile declared, read by
    /// [`truncated`] as Instagram having stopped serving pages.
    StoppedShort {
        walked: usize,
        declared: usize,
    },
    /// The same shortfall, read as a counter that overstates.
    ShortOfDeclared {
        walked: usize,
        declared: usize,
    },
    /// More than [`REPEAT_LIMIT_PERCENT`] of the rows were repeats.
    Repeated {
        repeated: usize,
        received: usize,
    },
}

/// What a walk does when the day's accounts run out before the list does.
/// Pausing is the default: it is the one that cannot make things worse with
/// Instagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverBudget {
    /// Sleep until the day has room, and carry on as the same walk.
    #[default]
    Pause,
    /// Read on, bounded only by the pace and the request budget's daily bucket.
    Continue,
}

#[derive(Debug, Clone)]
pub struct ListRequest<'a> {
    pub pk: Pk,
    /// Only names the referring page. May be empty.
    pub username: &'a str,
    pub direction: Direction,
    /// Cursor to continue an interrupted walk from.
    pub from: Option<&'a str>,
    /// For progress and the completion check. Never a stop condition.
    pub estimated: Option<u64>,
    pub max_pages: Option<u32>,
    /// How many were already stored, when resuming.
    pub already_stored: usize,
    pub over_budget: OverBudget,
}

#[derive(Debug)]
pub struct WalkSummary {
    pub pages: u32,
    pub users: usize,
    pub reason: StopReason,
    /// Where it left off, if anything is left.
    pub pending_cursor: Option<String>,
    pub error: Option<IgError>,
}

impl WalkSummary {
    pub fn is_complete(&self) -> bool {
        self.reason.yields_complete_list()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WalkError {
    #[error("the account is in cooldown for another {}", minutes(.remaining_ms))]
    Cooldown {
        until_ms: EpochMs,
        remaining_ms: i64,
    },
    #[error(transparent)]
    Budget(IgError),
    #[error("could not save the page: {0}")]
    Save(Box<dyn Error + Send + Sync>),
}

/// A remainder under a minute still says 1.
fn minutes(remaining_ms: &i64) -> String {
    match (remaining_ms / 60_000).max(1) {
        1 => "1 minute".to_string(),
        n => format!("{n} minutes"),
    }
}

fn expected_on_a_page(pace: &Pace) -> u32 {
    pace.per_page
}

pub struct ListWalker<'a> {
    client: &'a IgClient,
    pace: Pace,
    sleeps: bool,
    heartbeat: Option<Heartbeat<'a>>,
    /// Asked before every page: `true` ends the walk there, as a stop with
    /// its cursor. See [`ListWalker::with_stop`].
    stop: Option<&'a dyn Fn() -> bool>,
    /// The wall clock; the tests put one that jumps here.
    wall: fn() -> EpochMs,
    /// The clock that stands still while the machine sleeps
    /// ([`crate::awake::now`]).
    awake: fn() -> Duration,
}

enum Fetched {
    Page(FriendshipsPage),
    /// The request failed after the machine slept this long: a walk to pick
    /// up again ([`ListWalker::come_back`]), not a failure to retry.
    Slept(Duration),
}

impl<'a> ListWalker<'a> {
    /// Rate control is inside the client, and **the client decides whether
    /// the waits are real**, by the server it is pointed at: no caller can
    /// ask for a walk against Instagram with no waits.
    pub fn new(client: &'a IgClient) -> Self {
        Self {
            client,
            pace: Pace::default(),
            sleeps: client.is_live(),
            heartbeat: None,
            stop: None,
            wall: snob_core::clock::now_ms,
            awake: crate::awake::now,
        }
    }

    /// See [`Heartbeat`].
    #[must_use]
    pub fn with_heartbeat(mut self, heartbeat: Heartbeat<'a>) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }

    /// Sets what is asked before every page: `true` stops the walk as
    /// [`StopReason::Canceled`], with its cursor. For a reason outside the
    /// walk that is not the person stopping it (the battery running out under
    /// the monitor).
    #[must_use]
    pub fn with_stop(mut self, stop: &'a dyn Fn() -> bool) -> Self {
        self.stop = Some(stop);
        self
    }

    #[must_use]
    pub fn with_pace(mut self, pace: Pace) -> Self {
        self.pace = pace;
        self
    }

    fn cancel(&self) -> &CancelToken {
        self.client.pacer().cancel_token()
    }

    /// Walks the whole list, page by page. `save` receives each page as it
    /// arrives and **before any wait**, and returns how many users were new.
    pub async fn walk<S, O>(
        &self,
        request: ListRequest<'_>,
        mut save: S,
        mut observe: O,
    ) -> Result<WalkSummary, WalkError>
    where
        S: FnMut(&FriendshipsPage, u32) -> Result<usize, Box<dyn Error + Send + Sync>>,
        O: FnMut(Event),
    {
        self.check_cooldown().await?;
        // A walk is an action, and so is each sitting after its rest.
        let pacer = self.client.pacer();
        pacer.begin_action();

        observe(Event::Started {
            estimated: request.estimated,
            resumed: request.from.is_some(),
        });

        let mut state = WalkState::new(&request);
        // Opened once a walk, before its first page, and again after the
        // machine slept.
        let mut opened = false;
        let mut asleep = Asleep::new((self.wall)(), (self.awake)());

        let reason = loop {
            if self.cancel().is_canceled() {
                break StopReason::Canceled;
            }

            // Asked on every page: a cooldown another process writes while
            // this walk is in flight has to reach it. `break`, so the partial
            // keeps its cursor; `Pacer::clear` is the backstop inside the
            // client.
            if self
                .client
                .pacer()
                .cooldown_off_thread()
                .await
                .map_err(WalkError::Budget)?
                .is_some()
            {
                break StopReason::RateLimit;
            }

            if let Some(end) = state.cap_reached(&request) {
                break end;
            }

            if self.stop.is_some_and(|stop| stop()) {
                break StopReason::Canceled;
            }

            // **A rest owed is paid before the page**, whoever owes it
            // (`Pacer::rest_owed`), and before the list is opened. Never cut
            // by `--max-wait`. The loop starts over afterwards.
            if let Some((until, left)) = pacer
                .rest_owed(&self.pace, (self.wall)())
                .await
                .map_err(WalkError::Budget)?
            {
                if self.wait(WaitKind::Sitting, left, &mut observe).await {
                    break StopReason::Canceled;
                }
                pacer.rested(until).await.map_err(WalkError::Budget)?;
                pacer.begin_action();
                continue;
            }

            // The day's accounts, asked before the page: one that would go
            // over is not asked for until the day has room for a run of them.
            // Against a test server there is no day to spend.
            if self.sleeps && request.over_budget == OverBudget::Pause {
                let page = expected_on_a_page(&self.pace);
                if !pacer
                    .accounts_wait(page)
                    .await
                    .map_err(WalkError::Budget)?
                    .is_zero()
                {
                    let wait = pacer
                        .accounts_wait(page * PAGES_AFTER_A_PAUSE)
                        .await
                        .map_err(WalkError::Budget)?;
                    // Past the caller's limit the walk stops as at Ctrl+C.
                    if let Err(e) = pacer.within_the_limit(wait) {
                        break self.stop_reason_for(e, &mut state);
                    }
                    if self.sleep_through_the_day(wait, &mut observe).await? {
                        break StopReason::Canceled;
                    }
                    continue;
                }
            }

            // **The list is opened as a person opens it**: the app's
            // navigation (`IgClient::open_list`), then the dwell.
            if !opened {
                opened = true;
                if let Err(e) = self.client.open_list(request.username, false).await {
                    break self.stop_reason_for(e, &mut state);
                }
                let dwell = self.slowed(self.pace.dwell()).await;
                if self.wait(WaitKind::Dwell, dwell, &mut observe).await {
                    break StopReason::Canceled;
                }
            }

            // A step before every page but the first. The budget's own wait
            // is paid inside the client, after this one.
            if state.pages > 0 {
                let step = self.slowed(self.pace.step()).await;
                if self.wait(WaitKind::Step, step, &mut observe).await {
                    break StopReason::Canceled;
                }
            }

            // **A machine that slept is a walk to pick up again**: looked for
            // before every page and when one fails (`come_back`).
            let fetched = match self.slept(&mut asleep) {
                Some(slept) => Ok(Fetched::Slept(slept)),
                None => {
                    self.fetch_page(&request, &state, &mut asleep, &mut observe)
                        .await
                }
            };
            let page = match fetched {
                Ok(Fetched::Page(p)) => p,
                Ok(Fetched::Slept(slept)) => {
                    if let Err(e) = self.come_back(&request, slept, &mut observe).await {
                        break self.stop_reason_for(e, &mut state);
                    }
                    asleep = Asleep::new((self.wall)(), (self.awake)());
                    opened = false;
                    continue;
                }
                Err(e) => break self.stop_reason_for(e, &mut state),
            };
            let received = page.users.len();
            let added = save(&page, state.pages + 1).map_err(WalkError::Save)?;
            // Counted whether or not the walk goes on: the page that fills a
            // sitting owes its rest to whatever walks next.
            let sitting = pacer
                .page_walked(&self.pace, (self.wall)())
                .await
                .map_err(WalkError::Budget)?;

            state.pages += 1;
            state.users += added;
            observe(Event::Page {
                number: state.pages,
                received,
                added,
                running_total: state.users,
                sitting,
            });

            if let Some(end) = state.record_page(received, added, &page, &mut observe) {
                break end;
            }
        };

        // The rate the repeat rule judges, logged however the walk ended.
        tracing::debug!(
            list = ?request.direction,
            pages = state.pages,
            received = state.received,
            repeated = state.repeated,
            "the rows a walk received, and how many it had received already"
        );
        let reason = state.verify_completion(reason, &request, &mut observe);

        observe(Event::Finished {
            pages: state.pages,
            users: state.users,
            reason,
        });

        Ok(WalkSummary {
            pages: state.pages,
            users: state.users,
            reason,
            pending_cursor: if reason.yields_complete_list() {
                None
            } else {
                state.cursor.clone()
            },
            error: state.error,
        })
    }

    /// Sleeps until the day has room, calling the heartbeat every
    /// [`HEARTBEAT`]. `true` when cut short: a stop, or the heartbeat said the
    /// walk is no longer wanted.
    ///
    /// **The wait ends at a moment on the wall clock**: the timers a sleep
    /// runs on stand still while the machine is suspended, so counted in
    /// sleeps a laptop shut overnight would wake to the whole wait again. The
    /// sleeping is counted too, as the bound against a clock set back.
    async fn sleep_through_the_day<O: FnMut(Event)>(
        &self,
        wait: Duration,
        observe: &mut O,
    ) -> Result<bool, WalkError> {
        observe(Event::Waiting {
            kind: WaitKind::Day,
            duration: wait,
        });
        let until = (self.wall)() + wait;
        let mut slept = Duration::ZERO;
        while slept < wait {
            let on_the_wall = until - (self.wall)();
            let Ok(on_the_wall) = u64::try_from(on_the_wall) else {
                break;
            };
            if on_the_wall == 0 {
                break;
            }
            let step = Duration::from_millis(on_the_wall)
                .min(wait - slept)
                .min(HEARTBEAT);
            if self.cancel().sleep_or_cancel(step).await {
                return Ok(true);
            }
            if let Some(still_wanted) = self.heartbeat
                && !still_wanted().map_err(WalkError::Save)?
            {
                return Ok(true);
            }
            slept += step;
        }
        Ok(false)
    }

    async fn check_cooldown(&self) -> Result<(), WalkError> {
        let until = self
            .client
            .pacer()
            .cooldown_off_thread()
            .await
            .map_err(WalkError::Budget)?;
        if let Some(until_ms) = until {
            let remaining_ms = until_ms - snob_core::clock::now_ms();
            return Err(WalkError::Cooldown {
                until_ms,
                remaining_ms,
            });
        }
        Ok(())
    }

    async fn fetch_page<O: FnMut(Event)>(
        &self,
        request: &ListRequest<'_>,
        state: &WalkState,
        asleep: &mut Asleep,
        observe: &mut O,
    ) -> Result<Fetched, IgError> {
        let mut attempt = 0;
        loop {
            let result = self
                .client
                .friendships_page(
                    request.pk,
                    request.username,
                    request.direction,
                    self.pace.per_page,
                    state.cursor.as_deref(),
                )
                .await;

            let error = match result {
                Ok(p) => return Ok(Fetched::Page(p)),
                Err(e) => e,
            };

            // A request that failed across a sleep failed because of it, and
            // is not one of the retries.
            if error.reaction() == Reaction::Retry
                && let Some(slept) = self.slept(asleep)
            {
                return Ok(Fetched::Slept(slept));
            }

            // `reaction()` is the single authority: it is what guarantees a
            // 429 is never retried.
            if error.reaction() != Reaction::Retry || attempt >= self.pace.network_retries {
                return Err(error);
            }

            let after = self.pace.backoff(attempt);
            observe(Event::Retrying {
                attempt: attempt + 1,
                after,
                error: error.to_string(),
            });
            // `Canceled`, not the server's error: Ctrl+C during the backoff is
            // the user stopping.
            if self.sleeps && self.cancel().sleep_or_cancel(after).await {
                return Err(IgError::Canceled);
            }
            attempt += 1;
        }
    }

    fn stop_reason_for(&self, error: IgError, state: &mut WalkState) -> StopReason {
        let reason = match error.reaction() {
            Reaction::Cooldown => StopReason::RateLimit,
            Reaction::Retry => StopReason::Network,
            Reaction::Abort => match error {
                IgError::SessionExpired
                | IgError::SignedOutHere
                | IgError::UserAgentMismatch
                | IgError::Challenge { .. }
                | IgError::Checkpoint { .. } => StopReason::SessionInvalid,
                // Ctrl+C during the budget's wait arrives as an error.
                IgError::Canceled => StopReason::Canceled,
                // A wait past the caller's limit is the caller stopping.
                IgError::WaitTooLong { .. } => StopReason::Canceled,
                // `Pacer::clear`'s backstop: throttling, the same condition
                // the mid-walk check breaks on.
                IgError::InCooldown { .. } => StopReason::RateLimit,
                _ => StopReason::Network,
            },
        };
        state.error = Some(error);
        reason
    }

    fn slept(&self, asleep: &mut Asleep) -> Option<Duration> {
        asleep.check((self.wall)(), (self.awake)())
    }

    /// Picks the walk up again after the machine slept. **As at the end of a
    /// sitting, not as after a failure**: the document the list opens over is
    /// loaded again ([`IgClient::reload`]), waiting for the network as
    /// [`crate::pace::RECONNECT_WAITS_SECS`] allows, and the list is opened
    /// again with its dwell. An error ends the walk, with its cursor.
    async fn come_back<O: FnMut(Event)>(
        &self,
        request: &ListRequest<'_>,
        slept: Duration,
        observe: &mut O,
    ) -> Result<(), IgError> {
        tracing::info!(slept = ?slept, "the machine slept during a walk; picking it up again");
        observe(Event::Resumed { slept });
        self.client.pacer().begin_action();
        let mut waits = crate::pace::RECONNECT_WAITS_SECS.into_iter();
        loop {
            let error = match self.client.reload(request.username).await {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            let Some(after) = waits.next().filter(|_| error.reaction() == Reaction::Retry) else {
                return Err(error);
            };
            if self
                .wait(WaitKind::Reconnect, Duration::from_secs(after), observe)
                .await
            {
                return Err(IgError::Canceled);
            }
        }
    }

    async fn slowed(&self, drawn: Duration) -> Duration {
        self.client
            .pacer()
            .slowed(&self.pace, drawn, (self.wall)())
            .await
    }

    /// Returns `true` if it was canceled while waiting.
    async fn wait<O: FnMut(Event)>(
        &self,
        kind: WaitKind,
        duration: Duration,
        observe: &mut O,
    ) -> bool {
        if duration.is_zero() {
            return false;
        }
        observe(Event::Waiting { kind, duration });
        if !self.sleeps {
            return self.cancel().is_canceled();
        }
        self.cancel().sleep_or_cancel(duration).await
    }
}

struct WalkState {
    cursor: Option<String>,
    /// The cursor of the previous page, to catch one that does not advance.
    last_cursor: Option<String>,
    pages: u32,
    users: usize,
    empty_in_a_row: u32,
    barren_in_a_row: u32,
    /// Every account this walk has received, to tell a repeat. A resumed walk
    /// starts it empty.
    seen: HashSet<Pk>,
    received: usize,
    repeated: usize,
    error: Option<IgError>,
}

impl WalkState {
    fn new(request: &ListRequest<'_>) -> Self {
        Self {
            cursor: request.from.map(str::to_string),
            last_cursor: None,
            pages: 0,
            users: request.already_stored,
            empty_in_a_row: 0,
            barren_in_a_row: 0,
            seen: HashSet::new(),
            received: 0,
            repeated: 0,
            error: None,
        }
    }

    fn cap_reached(&self, request: &ListRequest<'_>) -> Option<StopReason> {
        if let Some(max) = request.max_pages
            && self.pages >= max
        {
            return Some(StopReason::PageLimit);
        }
        if self.pages >= HARD_PAGE_CAP {
            return Some(StopReason::Truncated);
        }
        None
    }

    fn record_page<O: FnMut(Event)>(
        &mut self,
        received: usize,
        added: usize,
        page: &FriendshipsPage,
        observe: &mut O,
    ) -> Option<StopReason> {
        for user in &page.users {
            self.received += 1;
            if !self.seen.insert(user.pk) {
                self.repeated += 1;
            }
        }

        let next = page.next_cursor().map(str::to_string);

        let Some(next) = next else {
            return Some(StopReason::Completed);
        };

        // A cursor that does not advance is a loop. Compared against the one
        // just sent too, which on the first page of a resumed walk is the
        // only one there is.
        if self.last_cursor.as_deref() == Some(next.as_str())
            || self.cursor.as_deref() == Some(next.as_str())
        {
            observe(Event::Warning(Warning::SameCursorTwice));
            return Some(StopReason::Truncated);
        }

        if received == 0 {
            self.empty_in_a_row += 1;
            if self.empty_in_a_row >= 2 {
                observe(Event::Warning(Warning::TwoEmptyPages));
                return Some(StopReason::Truncated);
            }
        } else {
            self.empty_in_a_row = 0;
        }

        if added == 0 {
            self.barren_in_a_row += 1;
            if self.barren_in_a_row >= MAX_PAGES_WITHOUT_NEW {
                observe(Event::Warning(Warning::GoingInCircles));
                return Some(StopReason::Truncated);
            }
        } else {
            self.barren_in_a_row = 0;
        }

        self.last_cursor = Some(next.clone());
        self.cursor = Some(next);
        None
    }

    /// Last filter: a clean ending far short of what was declared is probably
    /// not clean.
    fn verify_completion<O: FnMut(Event)>(
        &self,
        reason: StopReason,
        request: &ListRequest<'_>,
        observe: &mut O,
    ) -> StopReason {
        if reason != StopReason::Completed {
            return reason;
        }

        if self.repeated * 100 > self.received * REPEAT_LIMIT_PERCENT {
            observe(Event::Warning(Warning::Repeated {
                repeated: self.repeated,
                received: self.received,
            }));
            return StopReason::Truncated;
        }

        // Nothing walked and no counter: an account with no followers and an
        // Instagram that served none look the same. An empty list accepted
        // as complete makes every account followed an unfollower, so this
        // refuses.
        if request.estimated.is_none() && self.users == 0 {
            observe(Event::Warning(Warning::EmptyAndNoCounter));
            return StopReason::Truncated;
        }

        let Some(estimated) = request.estimated else {
            return reason;
        };

        let estimated = estimated as usize;
        if self.users >= estimated {
            return reason;
        }

        // More than a tenth short is more than deleted accounts account for.
        let far_short = estimated > self.users * 11 / 10;
        if !far_short {
            return reason;
        }

        if truncated(self.users, estimated) {
            observe(Event::Warning(Warning::StoppedShort {
                walked: self.users,
                declared: estimated,
            }));
            return StopReason::Truncated;
        }

        observe(Event::Warning(Warning::ShortOfDeclared {
            walked: self.users,
            declared: estimated,
        }));
        reason
    }
}

/// Which explanation for a short list to believe: a counter that overstates,
/// or Instagram having stopped serving pages. Truncated when the **declared**
/// size is past the threshold, or when the list is under half of it: deleted
/// accounts are not half of anybody's followers.
fn truncated(walked: usize, declared: usize) -> bool {
    declared >= TRUNCATION_THRESHOLD || walked * 2 < declared
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snob_core::session::{Session, SessionOrigin};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::client::harness::{SID, UA, client_with};
    use crate::client::page::PageError;
    use crate::pace::{KeptSitting, PAGES_PER_SITTING, Pacer};
    use crate::web::Told;

    #[test]
    fn the_cooldown_display_pluralizes_the_minutes() {
        let one = WalkError::Cooldown {
            until_ms: EpochMs::new(0),
            remaining_ms: 30_000,
        };
        assert_eq!(
            one.to_string(),
            "the account is in cooldown for another 1 minute"
        );

        let four = WalkError::Cooldown {
            until_ms: EpochMs::new(0),
            remaining_ms: 240_000,
        };
        assert_eq!(
            four.to_string(),
            "the account is in cooldown for another 4 minutes"
        );
    }

    fn client(server: &MockServer) -> IgClient {
        client_with(server, Pacer::unlimited())
    }

    /// A walk against Instagram pays every wait; one against a test server pays
    /// none, and neither is a choice a caller gets to make.
    ///
    /// The assertion behind AGENTS.md's "never walk a real account's lists
    /// without the limiter".
    #[tokio::test]
    async fn only_a_walk_against_a_test_server_skips_the_waits() {
        let server = MockServer::start().await;
        assert!(
            !ListWalker::new(&client(&server)).sleeps,
            "a mock server is not Instagram, so there is nothing to be polite to"
        );

        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        let live = IgClient::new(session, Pacer::unlimited()).unwrap();
        assert!(
            ListWalker::new(&live).sleeps,
            "a walk against Instagram has to pay its waits"
        );
    }

    /// A body with `n` users and, optionally, a cursor to the next page.
    fn body(from: u64, n: u64, cursor: Option<&str>) -> String {
        let users: Vec<String> = (from..from + n)
            .map(|i| format!(r#"{{"pk":{i},"username":"u{i}"}}"#))
            .collect();
        let cursor = match cursor {
            Some(c) => format!(r#","next_max_id":"{c}""#),
            None => String::new(),
        };
        format!(r#"{{"users":[{}]{cursor}}}"#, users.join(","))
    }

    fn request<'a>() -> ListRequest<'a> {
        ListRequest {
            pk: Pk::new(42),
            username: "someone",
            direction: Direction::Followers,
            from: None,
            estimated: None,
            max_pages: None,
            already_stored: 0,
            over_budget: OverBudget::Pause,
        }
    }

    /// Serves a scripted sequence of responses, one per request.
    async fn server(responses: Vec<ResponseTemplate>) -> MockServer {
        let server = MockServer::start().await;
        for (i, r) in responses.into_iter().enumerate() {
            Mock::given(method("GET"))
                .respond_with(r)
                .up_to_n_times(1)
                .with_priority(i as u8 + 1)
                .mount(&server)
                .await;
        }
        server
    }

    fn ok(body: String) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_string(body)
    }

    /// Walks collecting the events, without sleeping and with a free budget.
    async fn walk(
        server: &MockServer,
        request: ListRequest<'_>,
    ) -> (WalkSummary, Vec<Event>, Vec<Pk>) {
        let client = client(server);
        let walker = ListWalker::new(&client);

        let mut events = Vec::new();
        let mut seen: Vec<Pk> = Vec::new();

        let summary = walker
            .walk(
                request,
                |page, _| {
                    let before = seen.len();
                    for u in &page.users {
                        if !seen.contains(&u.pk) {
                            seen.push(u.pk);
                        }
                    }
                    Ok(seen.len() - before)
                },
                |e| events.push(e),
            )
            .await
            .unwrap();

        (summary, events, seen)
    }

    /// The one case where a clean ending is not a clean ending.
    ///
    /// One page, no users, no cursor: `record_page` reads that as the list
    /// being done, and it is also what Instagram serving nothing looks like.
    /// The declared counter is what tells the two apart, and the walk that
    /// hits this is the one whose profile poll already failed.
    ///
    /// Accepted as complete, the empty followers list makes every account you
    /// follow an unfollower — not a partial answer but a wrong one.
    #[tokio::test]
    async fn an_empty_list_with_no_counter_to_check_it_against_is_refused() {
        let server = server(vec![ok(body(0, 0, None))]).await;
        let (summary, events, _) = walk(&server, request()).await;

        assert_eq!(summary.users, 0);
        assert_eq!(summary.reason, StopReason::Truncated);
        assert!(!summary.is_complete());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Warning(Warning::EmptyAndNoCounter))),
            "{events:?}"
        );
    }

    /// The same answer, with the counter agreeing that the account really has
    /// nobody. Nothing to be suspicious of, and refusing it would make an
    /// empty account permanently unusable.
    #[tokio::test]
    async fn an_empty_list_the_counter_confirms_is_complete() {
        let server = server(vec![ok(body(0, 0, None))]).await;
        let request = ListRequest {
            estimated: Some(0),
            ..request()
        };
        let (summary, _, _) = walk(&server, request).await;

        assert_eq!(summary.reason, StopReason::Completed);
        assert!(summary.is_complete());
    }

    #[tokio::test]
    async fn it_walks_until_the_cursor_runs_out() {
        let server = server(vec![
            ok(body(0, 50, Some("c1"))),
            ok(body(50, 50, Some("c2"))),
            ok(body(100, 20, None)),
        ])
        .await;

        let (summary, _, seen) = walk(&server, request()).await;

        assert_eq!(summary.reason, StopReason::Completed);
        assert!(summary.is_complete());
        assert_eq!(summary.pages, 3);
        assert_eq!(summary.users, 120);
        assert_eq!(seen.len(), 120);
        assert_eq!(summary.pending_cursor, None);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    /// `n` pages of twelve with distinct accounts, the last without a
    /// cursor, numbered from `first`.
    fn pages(first: u64, n: u64) -> Vec<ResponseTemplate> {
        (first..first + n)
            .map(|i| {
                let cursor = (i + 1 < first + n).then(|| format!("c{i}"));
                ok(body(i * 12, 12, cursor.as_deref()))
            })
            .collect()
    }

    /// What a walk's waits and pages were, in order: `P` for a page, the
    /// wait's kind otherwise.
    fn cadence(events: &[Event]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Page { number, .. } => Some(format!("P{number}")),
                Event::Waiting { kind, .. } => Some(format!("{kind:?}")),
                _ => None,
            })
            .collect()
    }

    /// Walks `request` with the production pace and no sleeps, collecting
    /// the events.
    async fn walk_at_pace(client: &IgClient, request: ListRequest<'_>) -> Vec<Event> {
        // PRODUCTION pace: the real policy is what is under test.
        let walker = ListWalker::new(client).with_pace(Pace::default());
        let mut events = Vec::new();
        walker
            .walk(request, |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();
        events
    }

    /// A step comes between two pages and nowhere else: not before the
    /// first, which is the action starting, and not after the last. Before
    /// the first comes the dwell, once, as a person looks at the profile
    /// before opening its list.
    #[tokio::test]
    async fn a_step_comes_between_pages() {
        let server = server(pages(0, 3)).await;
        let events = walk_at_pace(&client(&server), request()).await;

        assert_eq!(
            cadence(&events),
            ["Dwell", "P1", "Step", "P2", "Step", "P3"],
            "{events:?}"
        );
        for e in &events {
            if let Event::Waiting { kind, duration } = e {
                let ms = duration.as_millis() as u64;
                let range = match kind {
                    WaitKind::Dwell => 1_500..=4_000,
                    _ => 1_000..=3_000,
                };
                assert!(range.contains(&ms), "a {kind:?} of {ms} ms");
            }
        }
    }

    /// Every walk dwells before its first page, the second of two on one
    /// client too, as the second list of a crossing is opened by a second
    /// click; a zeroed dwell is no wait at all.
    #[tokio::test]
    async fn every_walk_dwells_before_its_first_page() {
        let server = server([pages(0, 2), pages(100, 2)].concat()).await;
        let client = client(&server);
        for _ in 0..2 {
            let cadence = cadence(&walk_at_pace(&client, request()).await);
            assert_eq!(cadence, ["Dwell", "P1", "Step", "P2"]);
        }

        let server = self::server(pages(0, 2)).await;
        let client = self::client(&server);
        let walker = ListWalker::new(&client).with_pace(zero_pace());
        let mut events = Vec::new();
        walker
            .walk(request(), |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();
        assert_eq!(cadence(&events), ["P1", "P2"]);
    }

    /// Pins the pacing policy down: the sitting's rest lands after the
    /// fortieth page and nowhere else, every wait is inside its documented
    /// range, and none is paid after the last page.
    ///
    /// The rest is what is left of the one drawn when the fortieth page was
    /// walked, so it may come out a few milliseconds short of the range.
    #[tokio::test]
    async fn a_sitting_rests_after_its_fortieth_page() {
        let server = server(pages(0, 46)).await;
        let client = client(&server);
        let events = walk_at_pace(&client, request()).await;
        assert_eq!(
            client.pacer().actions(),
            2,
            "the walk begins an action, and so does the sitting after the rest"
        );
        let cadence = cadence(&events);

        let rests: Vec<usize> = cadence
            .iter()
            .enumerate()
            .filter(|(_, k)| *k == "Sitting")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(rests.len(), 1, "one rest in 46 pages: {cadence:?}");
        assert_eq!(cadence[rests[0] - 1], "P40");
        assert_eq!(cadence[rests[0] + 1], "Step");
        assert_eq!(cadence[rests[0] + 2], "P41");

        for e in &events {
            let Event::Waiting { kind, duration } = e else {
                continue;
            };
            let ms = duration.as_millis() as u64;
            let range = match kind {
                WaitKind::Dwell => (1_500, 4_000),
                WaitKind::Step => (1_000, 3_000),
                WaitKind::Sitting => (299_000, 900_000),
                WaitKind::Day => panic!("a free budget never pauses for the day"),
                WaitKind::Reconnect => panic!("a machine that never slept never reconnects"),
            };
            assert!(
                (range.0..=range.1).contains(&ms),
                "{kind:?} of {ms} ms is outside {range:?}"
            );
        }
        assert_eq!(cadence.last().map(String::as_str), Some("P46"));
    }

    /// A sitting is the account's reading, not one walk's: two walks of
    /// twenty-five pages on one client meet one rest, fifteen pages into the
    /// second.
    #[tokio::test]
    async fn two_walks_on_one_client_share_a_sitting() {
        let server = server([pages(0, 25), pages(100, 25)].concat()).await;
        let client = client(&server);

        let first = cadence(&walk_at_pace(&client, request()).await);
        assert!(
            !first.contains(&"Sitting".to_string()),
            "twenty-five pages fill no sitting: {first:?}"
        );

        let second = cadence(&walk_at_pace(&client, request()).await);
        let rests: Vec<usize> = second
            .iter()
            .enumerate()
            .filter(|(_, k)| *k == "Sitting")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(rests.len(), 1, "{second:?}");
        assert_eq!(second[rests[0] - 1], "P15");
        assert_eq!(server.received_requests().await.unwrap().len(), 50);
    }

    /// A walk that ends on a full sitting does not rest: there is nothing
    /// after the rest to wait for. The rest is left owed instead.
    #[tokio::test]
    async fn no_rest_follows_the_last_page() {
        let server = server(pages(0, 40)).await;
        let client = client(&server);
        let cadence = cadence(&walk_at_pace(&client, request()).await);

        assert!(!cadence.contains(&"Sitting".to_string()), "{cadence:?}");
        assert_eq!(cadence.last().map(String::as_str), Some("P40"));
        let owed = client
            .pacer()
            .rest_owed(&Pace::default(), snob_core::clock::now_ms())
            .await
            .unwrap();
        assert!(owed.is_some(), "the rest is owed to the next walk");
    }

    /// A walk that ends on a full sitting at its page cap does not rest
    /// either, though the list goes on: the cap ends the walk as surely as
    /// the list's last page does.
    #[tokio::test]
    async fn no_rest_follows_the_page_cap() {
        let server = server(pages(0, 46)).await;
        let client = client(&server);
        let request = ListRequest {
            max_pages: Some(40),
            ..request()
        };
        let walker = ListWalker::new(&client).with_pace(Pace::default());
        let mut events = Vec::new();
        let summary = walker
            .walk(request, |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();
        let cadence = cadence(&events);

        assert_eq!(summary.reason, StopReason::PageLimit);
        assert!(!cadence.contains(&"Sitting".to_string()), "{cadence:?}");
        assert_eq!(cadence.last().map(String::as_str), Some("P40"));
        assert_eq!(server.received_requests().await.unwrap().len(), 40);
    }

    /// The sitting a walk ended on stays full, and the next walk on the
    /// client pays its rest before anything else, its list opened and its
    /// first page included.
    #[tokio::test]
    async fn a_full_sitting_carries_over_to_the_next_walk() {
        let server = server([pages(0, 40), pages(100, 3)].concat()).await;
        let client = client(&server);

        let first = cadence(&walk_at_pace(&client, request()).await);
        assert!(!first.contains(&"Sitting".to_string()), "{first:?}");
        assert_eq!(first.last().map(String::as_str), Some("P40"));

        let second = cadence(&walk_at_pace(&client, request()).await);
        assert_eq!(
            second,
            ["Sitting", "Dwell", "P1", "Step", "P2", "Step", "P3"],
            "the carried-over sitting is rested before the next page"
        );
    }

    /// A list read a page at a time, one run a page, as an agent reads one:
    /// every call ends on its cap, and still the forty-first page waits for
    /// the rest the fortieth drew. Each call is a client and a pacer of its
    /// own over one budget, as each is a process of its own over one
    /// database.
    #[tokio::test]
    async fn a_walk_of_one_page_at_a_time_still_rests_after_forty() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ok(body(0, 12, Some("more"))))
            .mount(&server)
            .await;
        let budget = Arc::new(KeptSitting::default());
        let one_page = || ListRequest {
            max_pages: Some(1),
            ..request()
        };

        for call in 1..=PAGES_PER_SITTING + 1 {
            let client = client_with(&server, Pacer::new(budget.clone()));
            let cadence = cadence(&walk_at_pace(&client, one_page()).await);
            if call <= PAGES_PER_SITTING {
                assert_eq!(cadence, ["Dwell", "P1"], "call {call}");
            } else {
                assert_eq!(cadence, ["Sitting", "Dwell", "P1"], "call {call}");
            }
        }
        assert_eq!(
            budget.sitting().map(|s| (s.pages, s.rest_until)),
            Some((1, None)),
            "the rest was paid and a new sitting begun"
        );
    }

    /// A walk stopped during its rest leaves the rest owed, and the next
    /// walk, a run of its own, pays what is left of it before it sends
    /// anything; once the rest is over, the next walk goes at once and
    /// begins a new sitting.
    #[tokio::test]
    async fn a_walk_stopped_during_its_rest_leaves_the_rest_owed() {
        let server = server(pages(0, 10)).await;
        let budget = Arc::new(KeptSitting::default());
        let pace = Pace {
            pages_per_sitting: 2,
            sitting_pause_ms: (600_000, 600_000),
            ..zero_pace()
        };

        /// Walks with real waits, and stops the walk as its rest begins.
        async fn stopped_in_the_rest(
            client: &IgClient,
            pace: Pace,
            request: ListRequest<'_>,
        ) -> (WalkSummary, Vec<Event>) {
            let cancel = client.pacer().cancel_token().clone();
            let mut walker = ListWalker::new(client).with_pace(pace);
            walker.sleeps = true;
            let mut events = Vec::new();
            let summary = walker
                .walk(
                    request,
                    |p, _| Ok(p.users.len()),
                    |e| {
                        if matches!(
                            e,
                            Event::Waiting {
                                kind: WaitKind::Sitting,
                                ..
                            }
                        ) {
                            cancel.cancel();
                        }
                        events.push(e);
                    },
                )
                .await
                .unwrap();
            (summary, events)
        }

        let first = client_with(&server, Pacer::new(budget.clone()));
        let (summary, events) = stopped_in_the_rest(&first, pace, request()).await;
        assert_eq!(summary.reason, StopReason::Canceled);
        assert_eq!(cadence(&events), ["P1", "P2", "Sitting"]);
        let sent = server.received_requests().await.unwrap().len();
        let until = budget.sitting().and_then(|s| s.rest_until).expect("owed");

        let next = client_with(&server, Pacer::new(budget.clone()));
        let (summary, events) = stopped_in_the_rest(&next, pace, request()).await;
        assert_eq!(summary.reason, StopReason::Canceled);
        assert_eq!(cadence(&events), ["Sitting"], "{events:?}");
        let left = events.iter().find_map(|e| match e {
            Event::Waiting { duration, .. } => Some(*duration),
            _ => None,
        });
        assert!(
            left.is_some_and(
                |left| left <= Duration::from_secs(600) && left > Duration::from_secs(590)
            ),
            "what is left of the rest: {left:?}"
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            sent,
            "nothing was sent before the rest is over"
        );
        assert_eq!(budget.sitting().and_then(|s| s.rest_until), Some(until));

        fn rest_over() -> EpochMs {
            snob_core::clock::now_ms() + Duration::from_secs(601)
        }
        let after = client_with(&server, Pacer::new(budget.clone()));
        let mut walker = ListWalker::new(&after).with_pace(pace);
        walker.sleeps = true;
        walker.wall = rest_over;
        let one_page = ListRequest {
            max_pages: Some(1),
            ..request()
        };
        let mut events = Vec::new();
        walker
            .walk(one_page, |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();
        assert_eq!(cadence(&events), ["P1"]);
        assert_eq!(
            budget.sitting().map(|s| (s.pages, s.rest_until)),
            Some((1, None)),
            "the rest is over, and the page begins a new sitting"
        );
    }

    #[tokio::test]
    async fn a_repeating_cursor_stops_the_loop() {
        // Instagram always returning the same cursor: without the guard this is
        // an endless burst of requests.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ok(body(0, 50, Some("always_the_same"))))
            .mount(&server)
            .await;

        let (summary, events, _) = walk(&server, request()).await;

        assert_eq!(summary.reason, StopReason::Truncated);
        assert_eq!(summary.pages, 2, "it should stop as soon as it repeats");
        assert!(events.iter().any(|e| matches!(e, Event::Warning(_))));
    }

    #[tokio::test]
    async fn two_empty_pages_in_a_row_cut_it_short() {
        let server = server(vec![
            ok(body(0, 10, Some("c1"))),
            ok(body(0, 0, Some("c2"))),
            ok(body(0, 0, Some("c3"))),
        ])
        .await;

        let (summary, _, _) = walk(&server, request()).await;
        assert_eq!(summary.reason, StopReason::Truncated);
        assert!(!summary.is_complete());
    }

    #[tokio::test]
    async fn throttling_stops_without_retrying() {
        let server = server(vec![
            ok(body(0, 50, Some("c1"))),
            ok(body(50, 50, Some("c2"))),
            ResponseTemplate::new(429).set_body_string(r#"{"message":"","spam":true}"#),
        ])
        .await;

        let (summary, _, _) = walk(&server, request()).await;

        assert_eq!(summary.reason, StopReason::RateLimit);
        assert!(!summary.is_complete());
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            3,
            "a 429 must not cause a fourth request"
        );
        assert_eq!(summary.pending_cursor.as_deref(), Some("c2"));
    }

    #[tokio::test]
    async fn an_expired_session_stops_and_says_so() {
        let server =
            server(vec![ResponseTemplate::new(403).set_body_string(
                r#"{"message":"login_required","status":"fail"}"#,
            )])
            .await;

        let (summary, _, _) = walk(&server, request()).await;
        assert_eq!(summary.reason, StopReason::SessionInvalid);
        assert!(matches!(summary.error, Some(IgError::SessionExpired)));
    }

    #[tokio::test]
    async fn a_server_error_is_retried_and_then_given_up_on() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("oops"))
            .mount(&server)
            .await;

        let (summary, events, _) = walk(&server, request()).await;

        let retries = events
            .iter()
            .filter(|e| matches!(e, Event::Retrying { .. }))
            .count();
        assert_eq!(retries, 3, "it should retry three times");
        assert_eq!(summary.reason, StopReason::Network);
    }

    /// Ctrl+C during a retry backoff is the user stopping, not the server
    /// failing.
    ///
    /// A 503 plus an interrupt during the backoff exits 130, not 1 with
    /// `stopped_by = 'network'`: the walk's error is `Canceled`.
    ///
    /// The waits have to be on for this: a walk against a test server does not
    /// wait at all, so there would be no backoff to race the token against.
    /// Everything else in the pace is set to zero so the only real wait is the
    /// one under test, and the cancellation is fired from the `Retrying` event,
    /// which is emitted immediately before it.
    ///
    /// The client reads the same token before the retry and refuses it with
    /// `Canceled` too, so the reason and the error alone cannot tell the race
    /// from the whole 30-second backoff slept through. The time can: the walk
    /// comes back well inside the backoff only when the backoff itself saw the
    /// token.
    #[tokio::test]
    async fn canceling_during_a_backoff_is_not_a_network_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("oops"))
            .mount(&server)
            .await;

        let client = client(&server);
        let cancel = client.pacer().cancel_token().clone();
        let pace = Pace {
            step_ms: (0, 0),
            dwell_ms: (0, 0),
            sitting_pause_ms: (0, 0),
            backoff_base_ms: 30_000,
            ..Pace::default()
        };
        let mut walker = ListWalker::new(&client).with_pace(pace);
        walker.sleeps = true;

        let started = std::time::Instant::now();
        let summary = walker
            .walk(
                request(),
                |p, _| Ok(p.users.len()),
                |event| {
                    if matches!(event, Event::Retrying { .. }) {
                        cancel.cancel();
                    }
                },
            )
            .await
            .unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the backoff was slept through rather than cut short: {:?}",
            started.elapsed()
        );
        assert_eq!(summary.reason, StopReason::Canceled);
        assert!(
            matches!(summary.error, Some(IgError::Canceled)),
            "the server's error must not survive the user's interrupt: {:?}",
            summary.error
        );
    }

    #[tokio::test]
    async fn the_page_cap_stops_it_and_keeps_the_cursor() {
        // Distinct cursors per page: otherwise the repeating-cursor guard would
        // fire first.
        let server = server(vec![
            ok(body(0, 50, Some("c1"))),
            ok(body(50, 50, Some("c2"))),
            ok(body(100, 50, Some("c3"))),
        ])
        .await;

        let mut r = request();
        r.max_pages = Some(2);
        let (summary, _, _) = walk(&server, r).await;

        assert_eq!(summary.reason, StopReason::PageLimit);
        assert_eq!(summary.pages, 2);
        assert_eq!(summary.pending_cursor.as_deref(), Some("c2"));
        assert!(
            !summary.is_complete(),
            "a walk cut short does not describe the whole list"
        );
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            2,
            "the cap has to save real requests"
        );
    }

    /// The walk's own check between pages is what stops it, with no error:
    /// the client's refusal of the next request would stop it too, but that
    /// arrives as `IgError::Canceled` in the summary.
    #[tokio::test]
    async fn canceling_stops_at_the_page_boundary() {
        let server = server(vec![
            ok(body(0, 50, Some("c1"))),
            ok(body(50, 50, Some("c2"))),
            ok(body(100, 50, Some("c3"))),
        ])
        .await;

        let client = client(&server);
        let cancel = client.pacer().cancel_token().clone();
        let walker = ListWalker::new(&client);

        let summary = walker
            .walk(
                request(),
                |p, number| {
                    if number == 2 {
                        cancel.cancel();
                    }
                    Ok(p.users.len())
                },
                |_| {},
            )
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Canceled);
        assert_eq!(summary.pages, 2);
        assert!(summary.pending_cursor.is_some());
        assert!(
            summary.error.is_none(),
            "the client stopped it, not the walk: {:?}",
            summary.error
        );
    }

    #[tokio::test]
    async fn resuming_starts_from_the_stored_cursor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ok(body(0, 10, None)))
            .mount(&server)
            .await;

        let mut r = request();
        r.from = Some("from_here");
        r.already_stored = 100;
        let (summary, events, _) = walk(&server, r).await;

        assert!(matches!(
            events.first(),
            Some(Event::Started { resumed: true, .. })
        ));
        // What was already stored counts towards the total.
        assert_eq!(summary.users, 110);

        let requests = server.received_requests().await.unwrap();
        assert!(
            requests[0]
                .url
                .query()
                .unwrap()
                .contains("max_id=from_here"),
            "the first request should continue from the cursor"
        );
    }

    #[tokio::test]
    async fn falling_short_on_a_small_list_is_not_truncation() {
        // Instagram's counter includes deleted accounts that no longer appear.
        let server = server(vec![ok(body(0, 80, None))]).await;

        let mut r = request();
        r.estimated = Some(100);
        let (summary, events, _) = walk(&server, r).await;

        assert_eq!(summary.reason, StopReason::Completed);
        assert!(
            summary.is_complete(),
            "80 out of 100 on a small list is normal"
        );
        assert!(events.iter().any(|e| matches!(e, Event::Warning(_))));
    }

    /// The rule that decides between "the counter overstates" and "Instagram
    /// stopped serving", pinned down without walking anything.
    #[test]
    fn the_two_explanations_for_a_short_list_are_told_apart() {
        // A tenth missing off a small list is the counter counting the dead.
        assert!(!truncated(80, 100));
        assert!(!truncated(270, 300));

        // Half missing is not, whatever the size. This is the case that used
        // to be reported as a complete list.
        assert!(truncated(100, 3_000));
        assert!(truncated(1_000, 2_001));

        // Past the declared threshold, any real shortfall is suspect — and it
        // is the declared count that is measured, not the walked one.
        assert!(truncated(6_000, 40_000));
        assert!(truncated(9_000, 10_000));

        // Just under the threshold with a believable difference stays clean.
        assert!(!truncated(8_000, 9_999));
    }

    /// The regression the rule above exists for: a walk that ends cleanly far
    /// short of a large declared count must not become a basis for comparison.
    #[tokio::test]
    async fn stopping_far_short_of_a_large_counter_is_truncation() {
        let server = server(vec![ok(body(0, 50, None))]).await;

        let mut r = request();
        r.estimated = Some(40_000);
        let (summary, _, _) = walk(&server, r).await;

        assert_eq!(summary.reason, StopReason::Truncated);
        assert!(
            !summary.is_complete(),
            "50 accounts out of a declared 40,000 is a list that stopped, not a counter that lied"
        );
    }

    #[tokio::test]
    async fn falling_short_on_a_large_list_is_truncation() {
        let mut responses: Vec<ResponseTemplate> = (0..219)
            .map(|i| ok(body(i * 50, 50, Some(&format!("c{i}")))))
            .collect();
        responses.push(ok(body(10_950, 50, None)));
        let server = server(responses).await;

        let mut r = request();
        r.estimated = Some(30_000);
        let (summary, _, _) = walk(&server, r).await;

        assert_eq!(summary.reason, StopReason::Truncated);
        assert!(
            !summary.is_complete(),
            "a large list that gets cut short cannot be compared against"
        );
    }

    #[tokio::test]
    async fn a_cooldown_prevents_starting() {
        use snob_core::budget::{RateBudget, RateBudgetError};

        struct InCooldown;
        impl RateBudget for InCooldown {
            fn reserve(&self) -> Result<Duration, RateBudgetError> {
                Ok(Duration::ZERO)
            }
            fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
                self.reserve()
            }
            fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
                Ok(Some(
                    snob_core::clock::now_ms() + Duration::from_millis(3_600_000),
                ))
            }
            fn start_cooldown(&self, _: &str, _: Duration) -> Result<EpochMs, RateBudgetError> {
                Ok(EpochMs::new(0))
            }
        }

        let server = MockServer::start().await;
        let client = client_with(&server, Pacer::new(std::sync::Arc::new(InCooldown)));
        let walker = ListWalker::new(&client);

        let error = walker
            .walk(request(), |p, _| Ok(p.users.len()), |_| {})
            .await
            .unwrap_err();

        assert!(matches!(error, WalkError::Cooldown { .. }));
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            0,
            "in cooldown not a single request is made"
        );
    }

    /// A cooldown another process writes stops a walk already in flight.
    ///
    /// The database is shared per user, so this is the ordinary case rather
    /// than a contrived one: `snob watch` is eight minutes into a long walk,
    /// `snob whoami` in another terminal draws a 429, and the cooldown that
    /// stops *that* process was invisible to this one. The walk is the longest
    /// unbroken run of requests the tool produces and nothing on the way
    /// through re-read the table.
    ///
    /// The pages carry **distinct** users on purpose: with repeats,
    /// `MAX_PAGES_WITHOUT_NEW` would end the walk on its own and prove nothing.
    #[tokio::test]
    async fn a_cooldown_written_mid_walk_stops_the_walk() {
        use snob_core::budget::{RateBudget, RateBudgetError};

        /// Answers `None` until the walk is under way, then `Some` — the shape
        /// of another process writing the row while this one pages.
        ///
        /// **Three asks, not two, and the third one is the pacer's.** The walk
        /// consults the table at the entry check and once per iteration, and
        /// since the backstop went into `Pacer::clear` it is consulted again
        /// before every request. At two the cooldown became visible during the
        /// first page's reservation, so the walk stopped with nothing read and
        /// no cursor — which is the backstop working, and a different test from
        /// this one. `pace.rs` covers that. This one is about the check between
        /// pages, where the walk `break`s instead of erroring and the partial
        /// stays resumable, so the fixture has to let a page through first.
        #[derive(Default)]
        struct CooldownAfterThree(std::sync::atomic::AtomicUsize);
        impl RateBudget for CooldownAfterThree {
            fn reserve(&self) -> Result<Duration, RateBudgetError> {
                Ok(Duration::ZERO)
            }
            fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
                self.reserve()
            }
            fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
                let asked = self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok((asked >= 3)
                    .then(|| snob_core::clock::now_ms() + Duration::from_millis(7_200_000)))
            }
            fn start_cooldown(&self, _: &str, _: Duration) -> Result<EpochMs, RateBudgetError> {
                Ok(EpochMs::new(0))
            }
        }

        let server = server(vec![
            ok(body(0, 50, Some("c1"))),
            ok(body(50, 50, Some("c2"))),
            ok(body(100, 50, Some("c3"))),
            ok(body(150, 50, Some("c4"))),
        ])
        .await;

        let client = client_with(
            &server,
            Pacer::new(std::sync::Arc::new(CooldownAfterThree::default())),
        );
        let walker = ListWalker::new(&client);
        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |_| {})
            .await
            .unwrap();

        assert_eq!(
            summary.reason,
            StopReason::RateLimit,
            "the walk stopped for the reason it really stopped for"
        );
        assert!(
            server.received_requests().await.unwrap().len() < 4,
            "pages kept going out after the cooldown was written"
        );
        assert!(
            summary.pending_cursor.is_some(),
            "stopping is not the same as throwing the partial away"
        );
    }

    /// A budget that holds a number of accounts for the day and counts what it
    /// is charged. Asked for more than it holds, it answers a short wait and
    /// refills to `refill`: the day has passed by the time anybody asks again.
    struct Day {
        left: std::sync::atomic::AtomicU32,
        refill: u32,
    }

    impl Day {
        fn new(left: u32, refill: u32) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                left: std::sync::atomic::AtomicU32::new(left),
                refill,
            })
        }

        fn left(&self) -> u32 {
            self.left.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl snob_core::budget::RateBudget for Day {
        fn reserve(&self) -> Result<Duration, snob_core::budget::RateBudgetError> {
            Ok(Duration::ZERO)
        }
        fn reserve_write(&self) -> Result<Duration, snob_core::budget::RateBudgetError> {
            Ok(Duration::ZERO)
        }
        fn cooldown(&self) -> Result<Option<EpochMs>, snob_core::budget::RateBudgetError> {
            Ok(None)
        }
        fn start_cooldown(
            &self,
            _: &str,
            _: Duration,
        ) -> Result<EpochMs, snob_core::budget::RateBudgetError> {
            Ok(EpochMs::new(0))
        }
        fn accounts_wait(&self, n: u32) -> Result<Duration, snob_core::budget::RateBudgetError> {
            if n <= self.left() {
                return Ok(Duration::ZERO);
            }
            self.left
                .store(self.refill, std::sync::atomic::Ordering::Relaxed);
            Ok(Duration::from_millis(20))
        }
        fn spend_accounts(&self, n: u32) -> Result<(), snob_core::budget::RateBudgetError> {
            self.left.store(
                self.left().saturating_sub(n),
                std::sync::atomic::Ordering::Relaxed,
            );
            Ok(())
        }
    }

    /// A wall clock that reads its starting moment once, then two hours
    /// later: the machine slept through the day's wait.
    fn slept_through() -> EpochMs {
        static READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let start = 1_700_000_000_000;
        if READ.swap(true, std::sync::atomic::Ordering::SeqCst) {
            EpochMs::new(start + 2 * 3_600_000)
        } else {
            EpochMs::new(start)
        }
    }

    /// A wall clock somebody set an hour back once the wait had started.
    fn set_back() -> EpochMs {
        static READ: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let start = 1_700_000_000_000;
        if READ.swap(true, std::sync::atomic::Ordering::SeqCst) {
            EpochMs::new(start - 3_600_000)
        } else {
            EpochMs::new(start)
        }
    }

    /// A day's wait the machine slept through is over when it wakes: the
    /// budget's day is on the wall clock, and so is the end of the wait.
    #[tokio::test]
    async fn a_day_wait_slept_through_is_over_on_waking() {
        let server = MockServer::start().await;
        let client = client_with(&server, Pacer::new(Day::new(0, 0)));
        let mut walker = ListWalker::new(&client);
        walker.wall = slept_through;

        let started = std::time::Instant::now();
        let mut events = Vec::new();
        let cut_short = walker
            .sleep_through_the_day(Duration::from_secs(3_600), &mut |e| events.push(e))
            .await
            .unwrap();

        assert!(!cut_short);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(days_waited(&events), 1, "the wait is still announced");
    }

    /// A wall clock set back does not stretch the wait past what the budget
    /// asked for.
    #[tokio::test]
    async fn a_clock_set_back_does_not_stretch_a_day_wait() {
        let server = MockServer::start().await;
        let client = client_with(&server, Pacer::new(Day::new(0, 0)));
        let mut walker = ListWalker::new(&client);
        walker.wall = set_back;

        let started = std::time::Instant::now();
        let cut_short = walker
            .sleep_through_the_day(Duration::from_millis(50), &mut |_| {})
            .await
            .unwrap();

        assert!(!cut_short);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// Answers as it is told to, and says the machine slept as it does: the
    /// request in flight when the lid closed.
    struct AsTheLidCloses {
        answer: ResponseTemplate,
        slept: &'static std::sync::atomic::AtomicBool,
    }

    impl wiremock::Respond for AsTheLidCloses {
        fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
            self.slept.store(true, std::sync::atomic::Ordering::SeqCst);
            self.answer.clone()
        }
    }

    /// A clock the machine is awake by that never moves: every move of the
    /// wall clock in these tests is then time spent asleep.
    fn never_moves() -> Duration {
        Duration::from_secs(1)
    }

    const NIGHT_MS: i64 = 8 * 3_600_000;

    static SLEPT_MID_PAGE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn wall_after_a_night_mid_page() -> EpochMs {
        let slept = SLEPT_MID_PAGE.load(std::sync::atomic::Ordering::SeqCst);
        EpochMs::new(1_700_000_000_000 + if slept { NIGHT_MS } else { 0 })
    }

    /// A page that fails because the machine slept under it is not a network
    /// failure to retry: the walk is picked up again, the list opened again
    /// with its dwell, and the same page asked once the walk is back.
    #[tokio::test]
    async fn a_page_failing_across_a_sleep_picks_the_walk_up_again() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(AsTheLidCloses {
                answer: ResponseTemplate::new(503),
                slept: &SLEPT_MID_PAGE,
            })
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ok(body(0, 25, None)))
            .with_priority(2)
            .mount(&server)
            .await;
        let client = client(&server);
        let mut walker = ListWalker::new(&client);
        walker.wall = wall_after_a_night_mid_page;
        walker.awake = never_moves;

        let mut events = Vec::new();
        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Completed, "{events:?}");
        assert!(
            events.contains(&Event::Resumed {
                slept: Duration::from_millis(u64::try_from(NIGHT_MS).unwrap())
            }),
            "{events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, Event::Retrying { .. })),
            "no retry spent on the sleep: {events:?}"
        );
        let dwells = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Waiting {
                        kind: WaitKind::Dwell,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(dwells, 2, "the list is opened again: {events:?}");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    static SLEPT_BEFORE_A_PAGE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn wall_after_a_night_before_a_page() -> EpochMs {
        // The first look is the walk starting; every one after it finds the
        // night gone by.
        if SLEPT_BEFORE_A_PAGE.swap(true, std::sync::atomic::Ordering::SeqCst) {
            EpochMs::new(1_700_000_000_000 + NIGHT_MS)
        } else {
            EpochMs::new(1_700_000_000_000)
        }
    }

    static SLEPT_UNDER_A_DEAD_NETWORK: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    fn wall_after_a_night_with_no_network() -> EpochMs {
        if SLEPT_UNDER_A_DEAD_NETWORK.swap(true, std::sync::atomic::Ordering::SeqCst) {
            EpochMs::new(1_700_000_000_000 + NIGHT_MS)
        } else {
            EpochMs::new(1_700_000_000_000)
        }
    }

    /// A browser's page that answers lists from `rest` and loads the
    /// document as `document` says, counting each document asked.
    fn a_page_reloading(
        document: impl Fn(u32) -> Result<Told, PageError> + Send + Sync + 'static,
    ) -> (IgClient, std::sync::Arc<std::sync::atomic::AtomicU32>) {
        use crate::client::harness::{Scripted, page_said, spending_client};
        let documents = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = std::sync::Arc::clone(&documents);
        let page = Scripted::telling(move |call| match &call.ask {
            crate::web::Ask::Document { .. } => {
                document(counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
            }
            crate::web::Ask::Rest { .. } => Ok(Told::Answer(page_said(200, &body(0, 25, None)))),
            _ => Ok(Told::Answer(page_said(200, r#"{"status":"ok"}"#))),
        });
        let (client, _) = spending_client(page);
        (client, documents)
    }

    /// A walk on a browser's page whose list pages each took `waited_ms` to
    /// arrive, five pages long, at the production pace: its events.
    async fn walk_answered_in(waited_ms: u32) -> Vec<Event> {
        use crate::client::harness::{Scripted, page_said, spending_client};
        let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let page = Scripted::telling(move |call| match &call.ask {
            crate::web::Ask::Rest { .. } => {
                let i = served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let cursor = (i < 4).then(|| format!("c{i}"));
                let from = i as u64 * 12;
                Ok(Told::Answer(crate::client::page::PageResponse {
                    waited_ms: Some(waited_ms),
                    ..page_said(200, &body(from, 12, cursor.as_deref()))
                }))
            }
            _ => Ok(Told::Answer(page_said(200, r#"{"status":"ok"}"#))),
        });
        let (client, _) = spending_client(page);
        walk_at_pace(&client, request()).await
    }

    /// The steps of a walk, in order, in milliseconds.
    fn steps(events: &[Event]) -> Vec<u128> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Waiting {
                    kind: WaitKind::Step,
                    duration,
                } => Some(duration.as_millis()),
                _ => None,
            })
            .collect()
    }

    /// Pages that take three seconds to arrive lengthen the step to twice
    /// that once three of them have: one or two slow answers are not yet a
    /// slow server.
    #[tokio::test]
    async fn slow_answers_lengthen_the_steps_after_them() {
        let events = walk_answered_in(3_000).await;
        let steps = steps(&events);
        assert_eq!(steps.len(), 4, "{events:?}");
        for step in &steps[..2] {
            assert!((1_000..=3_000).contains(step), "{steps:?}");
        }
        assert_eq!(steps[2..], [6_000, 6_000], "{steps:?}");
    }

    /// Answers as fast as Instagram's ordinarily are leave every step where
    /// the pace drew it.
    #[tokio::test]
    async fn fast_answers_leave_the_steps_alone() {
        let events = walk_answered_in(250).await;
        let steps = steps(&events);
        assert_eq!(steps.len(), 4, "{events:?}");
        for step in &steps {
            assert!((1_000..=3_000).contains(step), "{steps:?}");
        }
    }

    /// After a sleep the page the walk reads from is loaded again, as many
    /// times as the network takes to come back, with a wait before each, and
    /// the walk then carries on.
    #[tokio::test]
    async fn the_page_is_reloaded_once_the_network_is_back() {
        let (client, documents) = a_page_reloading(|n| {
            if n < 2 {
                Err(PageError::Unreachable("the radio is still off".into()))
            } else {
                Ok(Told::Document {
                    answer: crate::client::harness::page_said(200, ""),
                    bundles: Vec::new(),
                })
            }
        });
        let mut walker = ListWalker::new(&client);
        walker.wall = wall_after_a_night_before_a_page;
        walker.awake = never_moves;

        let mut events = Vec::new();
        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Completed, "{events:?}");
        assert_eq!(documents.load(std::sync::atomic::Ordering::SeqCst), 3);
        let reconnects = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Waiting {
                        kind: WaitKind::Reconnect,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(reconnects, 2, "{events:?}");
    }

    /// A network that never comes back ends the walk as a network failure,
    /// with its cursor, once the waits are spent.
    #[tokio::test]
    async fn a_network_that_never_comes_back_ends_the_walk_on_it() {
        let (client, documents) =
            a_page_reloading(|_| Err(PageError::Unreachable("nothing answers".into())));
        let mut walker = ListWalker::new(&client);
        walker.wall = wall_after_a_night_with_no_network;
        walker.awake = never_moves;

        let mut events = Vec::new();
        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Network, "{events:?}");
        let tries = u32::try_from(crate::pace::RECONNECT_WAITS_SECS.len()).unwrap() + 1;
        assert_eq!(documents.load(std::sync::atomic::Ordering::SeqCst), tries);
        assert!(
            !events.iter().any(|e| matches!(e, Event::Page { .. })),
            "nothing asked of a page that could not be loaded: {events:?}"
        );
    }

    /// A stop asked from outside ends the walk between two pages, as a stop
    /// with its cursor, and asks nothing more.
    #[tokio::test]
    async fn a_stop_asked_between_pages_keeps_the_cursor() {
        let server = server(vec![ok(body(0, 25, Some("c1"))), ok(body(25, 25, None))]).await;
        let client = client(&server);
        let pages = std::sync::atomic::AtomicU32::new(0);
        let stop = || pages.load(std::sync::atomic::Ordering::SeqCst) >= 1;
        let walker = ListWalker::new(&client).with_stop(&stop);

        let summary = walker
            .walk(
                request(),
                |p, _| {
                    pages.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(p.users.len())
                },
                |_| {},
            )
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Canceled);
        assert_eq!(summary.pending_cursor.as_deref(), Some("c1"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    fn zero_pace() -> Pace {
        Pace {
            step_ms: (0, 0),
            dwell_ms: (0, 0),
            sitting_pause_ms: (0, 0),
            slow: snob_core::budget::SlowRule::OFF,
            ..Pace::default()
        }
    }

    fn days_waited(events: &[Event]) -> usize {
        events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::Waiting {
                        kind: WaitKind::Day,
                        ..
                    }
                )
            })
            .count()
    }

    /// The page that would go past the day is not asked for until the day
    /// has room, and then the same walk carries on to the end, telling its
    /// caller it is still there while it sleeps.
    #[tokio::test]
    async fn the_page_that_would_go_past_the_day_waits_for_it() {
        let server = server(vec![
            ok(body(0, 25, Some("c1"))),
            ok(body(25, 25, Some("c2"))),
            ok(body(50, 25, None)),
        ])
        .await;
        let budget = Day::new(60, 25);
        let client = client_with(&server, Pacer::new(budget.clone()));
        let beats = std::sync::atomic::AtomicU32::new(0);
        let heartbeat = || {
            beats.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(true)
        };
        let mut walker = ListWalker::new(&client)
            .with_pace(zero_pace())
            .with_heartbeat(&heartbeat);
        walker.sleeps = true;

        let mut events = Vec::new();
        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Completed);
        assert_eq!(summary.pages, 3);
        assert_eq!(days_waited(&events), 1, "sixty accounts fit two pages");
        assert_eq!(beats.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
        assert_eq!(budget.left(), 0, "what the pages carried was charged");
    }

    /// A list going round in circles is caught however many pauses it is
    /// read across: the guards carry over a pause, or a day that allows one
    /// page at a time would never let a guard count to two.
    #[tokio::test]
    async fn a_list_going_in_circles_is_caught_across_pauses() {
        let a = || ok(body(0, 25, Some("a")));
        let b = || ok(body(25, 25, Some("b")));
        let server = server(vec![a(), b(), a(), b(), a(), b(), a(), b()]).await;
        let client = client_with(&server, Pacer::new(Day::new(25, 25)));
        let mut walker = ListWalker::new(&client).with_pace(zero_pace());
        walker.sleeps = true;

        let mut seen = std::collections::HashSet::new();
        let mut events = Vec::new();
        let summary = walker
            .walk(
                request(),
                |p, _| Ok(p.users.iter().filter(|u| seen.insert(u.pk)).count()),
                |e| events.push(e),
            )
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Truncated);
        assert!(events.contains(&Event::Warning(Warning::GoingInCircles)));
        assert_eq!(
            days_waited(&events),
            4,
            "a pause before every page but the first"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 5);
    }

    /// `--max-pages` caps the walk, not each stretch between two pauses.
    #[tokio::test]
    async fn the_page_cap_counts_across_a_pause() {
        let server = server(vec![
            ok(body(0, 25, Some("c1"))),
            ok(body(25, 25, Some("c2"))),
            ok(body(50, 25, None)),
        ])
        .await;
        let client = client_with(&server, Pacer::new(Day::new(25, 25)));
        let mut walker = ListWalker::new(&client).with_pace(zero_pace());
        walker.sleeps = true;

        let request = ListRequest {
            max_pages: Some(2),
            ..request()
        };
        let summary = walker
            .walk(request, |p, _| Ok(p.users.len()), |_| {})
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::PageLimit);
        assert_eq!(summary.pages, 2);
        assert_eq!(summary.pending_cursor.as_deref(), Some("c2"));
    }

    /// A walk whose capture another process took over while it slept stops
    /// there, with its cursor, and sends nothing more.
    #[tokio::test]
    async fn a_walk_no_longer_wanted_stops_while_it_sleeps() {
        let server = server(vec![ok(body(0, 25, Some("c1"))), ok(body(25, 25, None))]).await;
        let client = client_with(&server, Pacer::new(Day::new(25, 25)));
        let heartbeat = || Ok(false);
        let mut walker = ListWalker::new(&client)
            .with_pace(zero_pace())
            .with_heartbeat(&heartbeat);
        walker.sleeps = true;

        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |_| {})
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Canceled);
        assert_eq!(summary.pending_cursor.as_deref(), Some("c1"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// Choosing to finish today reads past the day, and still charges it.
    #[tokio::test]
    async fn finishing_today_reads_past_the_day() {
        let server = server(vec![
            ok(body(0, 25, Some("c1"))),
            ok(body(25, 25, Some("c2"))),
            ok(body(50, 25, None)),
        ])
        .await;
        let budget = Day::new(30, 0);
        let client = client_with(&server, Pacer::new(budget.clone()));
        let mut walker = ListWalker::new(&client).with_pace(zero_pace());
        walker.sleeps = true;

        let request = ListRequest {
            over_budget: OverBudget::Continue,
            ..request()
        };
        let mut events = Vec::new();
        let summary = walker
            .walk(request, |p, _| Ok(p.users.len()), |e| events.push(e))
            .await
            .unwrap();

        assert_eq!(summary.reason, StopReason::Completed);
        assert_eq!(days_waited(&events), 0);
        assert_eq!(budget.left(), 0);
    }

    /// The point of moving the budget into the client: a walk pays for every
    /// page without the walker having to remember to.
    #[tokio::test]
    async fn every_page_is_charged_to_the_budget() {
        use snob_core::budget::{RateBudget, RateBudgetError};

        #[derive(Default)]
        struct Counting(std::sync::atomic::AtomicUsize);
        impl RateBudget for Counting {
            fn reserve(&self) -> Result<Duration, RateBudgetError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(Duration::ZERO)
            }
            fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
                self.reserve()
            }
            fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
                Ok(None)
            }
            fn start_cooldown(&self, _: &str, _: Duration) -> Result<EpochMs, RateBudgetError> {
                Ok(EpochMs::new(0))
            }
        }

        let server = server(vec![
            ok(body(0, 50, Some("c1"))),
            ok(body(50, 50, Some("c2"))),
            ok(body(100, 20, None)),
        ])
        .await;

        let budget = std::sync::Arc::new(Counting::default());
        let client = client_with(&server, Pacer::new(budget.clone()));
        let walker = ListWalker::new(&client);
        let summary = walker
            .walk(request(), |p, _| Ok(p.users.len()), |_| {})
            .await
            .unwrap();

        assert_eq!(summary.pages, 3);
        assert_eq!(
            budget.0.load(std::sync::atomic::Ordering::Relaxed),
            3,
            "one reservation per request, taken by the client itself"
        );
    }

    #[tokio::test]
    async fn repeats_do_not_inflate_the_count() {
        // The same account on two pages: the running total follows what the
        // saver reports as new, not what arrived.
        let server =
            server(vec![
            ok(r#"{"users":[{"pk":1,"username":"a"},{"pk":2,"username":"b"}],"next_max_id":"c1"}"#
                .to_string()),
            ok(r#"{"users":[{"pk":2,"username":"b"},{"pk":3,"username":"c"}]}"#.to_string()),
        ])
            .await;

        let (summary, _, seen) = walk(&server, request()).await;
        assert_eq!(summary.users, 3);
        assert_eq!(seen, vec![Pk::new(1), Pk::new(2), Pk::new(3)]);
    }

    /// A page of the accounts `pks`, in that order.
    fn rows(pks: impl IntoIterator<Item = u64>, cursor: Option<&str>) -> String {
        let users: Vec<String> = pks
            .into_iter()
            .map(|i| format!(r#"{{"pk":{i},"username":"u{i}"}}"#))
            .collect();
        let cursor = match cursor {
            Some(c) => format!(r#","next_max_id":"{c}""#),
            None => String::new(),
        };
        format!(r#"{{"users":[{}]{cursor}}}"#, users.join(","))
    }

    /// Walks two pages, `first` then `second`, and says how it ended.
    async fn two_pages(first: String, second: String) -> (StopReason, Vec<Event>) {
        let server = server(vec![ok(first), ok(second)]).await;
        let (summary, events, _) = walk(&server, request()).await;
        (summary.reason, events)
    }

    /// More than one row in twenty served twice and the list is not believed
    /// complete, however short it is; at or under it, it is.
    #[tokio::test]
    async fn a_list_that_repeats_more_than_one_row_in_twenty_is_truncated() {
        // One repeat in thirteen rows: no sample is too small to count.
        let (reason, events) = two_pages(rows(0..11, Some("c1")), rows([0, 11], None)).await;
        assert_eq!(reason, StopReason::Truncated);
        assert!(
            events.contains(&Event::Warning(Warning::Repeated {
                repeated: 1,
                received: 13,
            })),
            "{events:?}"
        );

        // One in thirty.
        let (reason, _) = two_pages(rows(0..15, Some("c1")), rows((15..29).chain([0]), None)).await;
        assert_eq!(reason, StopReason::Completed);

        // Nine in a hundred and fifty, six percent.
        let (reason, events) =
            two_pages(rows(0..75, Some("c1")), rows((75..141).chain(0..9), None)).await;
        assert_eq!(reason, StopReason::Truncated);
        assert!(
            events.contains(&Event::Warning(Warning::Repeated {
                repeated: 9,
                received: 150,
            })),
            "{events:?}"
        );

        // Six in a hundred and fifty, four percent.
        let (reason, _) =
            two_pages(rows(0..75, Some("c1")), rows((75..144).chain(0..6), None)).await;
        assert_eq!(reason, StopReason::Completed);
    }

    /// Every walk writes down the rows it received and how many were
    /// repeats, under the threshold too, so that the live check measures the
    /// rate whatever it is.
    #[tokio::test]
    async fn a_walk_logs_its_repeat_rate_under_the_threshold_too() {
        #[derive(Clone, Default)]
        struct Lines(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let lines = Lines::default();
        let writer = lines.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish();
        // Every task of this test runs on its one thread. With a single
        // dispatcher registered, a test walking on another thread meanwhile
        // asks only its own (none) whether the walk's line is wanted and
        // caches "never" for everyone; a second one, alive for the test,
        // makes every thread ask them all.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
        let _logged = tracing::subscriber::set_default(subscriber);
        // One in thirty: under the threshold, so no warning says it.
        let (reason, events) =
            two_pages(rows(0..15, Some("c1")), rows((15..29).chain([0]), None)).await;
        assert_eq!(reason, StopReason::Completed);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Warning(Warning::Repeated { .. }))),
            "{events:?}"
        );

        let said = String::from_utf8(lines.0.lock().unwrap().clone()).unwrap();
        let rate: Vec<&str> = said
            .lines()
            .filter(|line| line.contains("the rows a walk received"))
            .collect();
        assert_eq!(rate.len(), 1, "{said}");
        for field in ["list=Followers", "pages=2", "received=30", "repeated=1"] {
            assert!(rate[0].contains(field), "{field} in {}", rate[0]);
        }
    }

    /// A resumed walk served the rows an earlier run of it stored has
    /// received each of them once: none is a repeat.
    #[tokio::test]
    async fn a_resumed_walk_re_served_its_stored_rows_has_no_repeats() {
        let server = server(vec![ok(rows(0..12, Some("c6"))), ok(rows(12..20, None))]).await;
        let client = client(&server);
        let walker = ListWalker::new(&client);
        let request = ListRequest {
            from: Some("c5"),
            already_stored: 20,
            ..request()
        };
        let mut events = Vec::new();
        let summary = walker
            .walk(request, |_, _| Ok(0), |e| events.push(e))
            .await
            .unwrap();
        assert_eq!(summary.reason, StopReason::Completed, "{events:?}");
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Warning(Warning::Repeated { .. }))),
            "{events:?}"
        );
    }
}
