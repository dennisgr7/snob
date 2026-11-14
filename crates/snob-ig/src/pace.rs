//! Request pacing and cancellation. See "Talking to Instagram" in `AGENTS.md`.
//!
//! **The numbers in [`Pace`] are not changed without a documented reason**,
//! written beside the number. Slower is always acceptable; faster has to be
//! argued for. None of them is a published rate:
//!
//! - **The cadence** is the web app's, per user action: a recorded session
//!   pages a list at a median of 1.2 s a page (26 pages in 37.5 s, no pause)
//!   and opens profiles 0.6 to 2.1 s apart. snob is slower than that, and
//!   rests after a sitting as somebody scrolling a list stops somewhere.
//! - **The volume** is what the account is judged on: Meta's paper on the
//!   system that issues these warnings (arXiv 2502.17693, February 2025)
//!   weighs each request by the accounts it returns. That ceiling is
//!   `budget::accounts_per_day`, not a pace, and it is the one that binds.
//! - **The first push-back is a hard stop and a cooldown**, never a retry.
//!
//! The request budget's pace bucket (`snob-store`'s `rate_budget.rs`) sits
//! under all of it. With the app's `show_many` a list page is two requests, so
//! the bucket starts pacing a sitting after about fourteen pages, not forty,
//! which is slower and is left so.
//!
//! Not evidence: Meta's "200 calls per user per hour" is the Graph API's, and
//! a recorded session in which nothing was refused says what the app does, not
//! how far past it one may go. `x-ig-capacity-level` and `x-ig-peak-time` are
//! written down and never acted on: they describe a datacenter, not an
//! account. **What is acted on is how long the answers take, and only to slow
//! down** ([`SlowRule`], the idea of Scrapling's AutoThrottle).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use snob_core::EpochMs;
use snob_core::budget::{AnswerTime, RateBudget, RateBudgetError, Sitting, SittingRule, SlowRule};

use crate::error::IgError;

/// What the web app asks for, `count=12` on both lists, of which it gets
/// eight to twelve back. A list endpoint serves up to fifty when asked, so
/// this makes two to four times the requests, on the owner's decision that a
/// list is read in the pages the app reads it in.
pub const ACCOUNTS_PER_PAGE: u32 = 12;

/// The fewest the app gets back on a page that is not the last. Nothing waits
/// on it: it is what an estimate (`--dry-run`) takes for a walk's most pages.
pub const ACCOUNTS_PER_PAGE_FEWEST: u32 = 8;

/// The wait between the pages of one action, drawn anew each time: one to
/// three seconds, slower at the median than the recorded app's 1.2 s a page.
pub const STEP_MS: (u64, u64) = (1_000, 3_000);

/// The wait between two pages of a post's comments: two to five seconds. The
/// "load more" clicks of the capture of 2026-11-13 came 2.3, 4.5 and 7.5 s
/// apart; three samples, so an estimate.
pub const COMMENT_STEP_MS: (u64, u64) = (2_000, 5_000);

/// The wait before the first page of a list: one and a half to four seconds.
/// In the recording of 2026-10-01 a person clicked a list 2.55 s after its
/// profile opened at the median, between 1.1 and 4.5. Paid before every list.
pub const DWELL_MS: (u64, u64) = (1_500, 4_000);

/// Forty pages, about 480 accounts: a long scroll (the recorded session pages
/// twenty-six in a row), and about a quarter of the day's accounts.
pub const PAGES_PER_SITTING: u32 = 40;

/// The rest after a full sitting: five to fifteen minutes. Long enough to
/// refill the pace bucket, short enough to stay inside a walk's claim
/// (`CLAIM_TTL_SECS` in `snob-store`'s `snapshots.rs`).
///
/// **The rest is owed, not taken on the spot** ([`snob_core::budget::Sitting`]):
/// whatever walks the account next pays what is left of it. The longest of
/// the range is also how long an unfilled sitting left alone takes to end.
pub const SITTING_PAUSE_MS: (u64, u64) = (300_000, 900_000);

/// How long a walk that finds the machine slept waits for the network, in
/// seconds, before each attempt after the first: two and a quarter minutes in
/// all. **Only ever an added wait.** The ordinary retries (2, 4, 8 s) are for
/// a server that failed, not for a radio that is still off.
pub const RECONNECT_WAITS_SECS: [u64; 5] = [5, 10, 20, 40, 60];

/// A step or a dwell lasts at least this many times the median of the latest
/// answers, which keeps snob's share of the server's time to a third. With
/// ordinary answers (under half a second) it changes nothing. Scrapling's
/// AutoThrottle waits once the answer's time; twice is slower.
pub const SLOW_FACTOR: u32 = 2;

/// The median is over the latest five healthy answers and needs three
/// ([`SlowRule::least`]): one slow answer is noise.
pub const SLOW_SAMPLES: usize = 5;

/// How far back an answer still counts. Shorter than the longest rest, so a
/// sitting that rested starts with the server as it is now.
pub const SLOW_WINDOW: Duration = Duration::from_secs(10 * 60);

/// The longest floor slow answers set. Answers of thirty seconds are a
/// failing server, which retries and push-back handling already answer.
pub const SLOW_CEILING: Duration = Duration::from_secs(60);

/// Enough for [`SLOW_SAMPLES`], and for a push-back's log line.
const ANSWERS_KEPT: usize = 8;

/// Request cadence: within an action, and between sittings.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    /// [`ACCOUNTS_PER_PAGE`].
    pub per_page: u32,
    /// [`STEP_MS`].
    pub step_ms: (u64, u64),
    /// [`DWELL_MS`].
    pub dwell_ms: (u64, u64),
    /// [`SITTING_PAUSE_MS`].
    pub sitting_pause_ms: (u64, u64),
    /// [`PAGES_PER_SITTING`].
    pub pages_per_sitting: u32,
    /// [`SLOW_FACTOR`], [`SLOW_SAMPLES`], [`SLOW_WINDOW`], [`SLOW_CEILING`].
    pub slow: SlowRule,
    /// How many times a network failure is retried before giving up.
    pub network_retries: u32,
    /// Base of the exponential delay between retries.
    pub backoff_base_ms: u64,
}

impl Default for Pace {
    /// A page every one to three seconds, and a rest of five to fifteen
    /// minutes about every 480 accounts.
    fn default() -> Self {
        Self {
            per_page: ACCOUNTS_PER_PAGE,
            step_ms: STEP_MS,
            dwell_ms: DWELL_MS,
            sitting_pause_ms: SITTING_PAUSE_MS,
            pages_per_sitting: PAGES_PER_SITTING,
            slow: SlowRule {
                factor: SLOW_FACTOR,
                samples: SLOW_SAMPLES,
                window: SLOW_WINDOW,
                ceiling: SLOW_CEILING,
            },
            network_retries: 3,
            backoff_base_ms: 2_000,
        }
    }
}

impl Pace {
    /// The default pace with [`COMMENT_STEP_MS`] between the pages.
    pub fn for_comments() -> Self {
        Self {
            step_ms: COMMENT_STEP_MS,
            ..Self::default()
        }
    }
}

impl Pace {
    /// The pace for somebody else's lists: the same cadence, and a network
    /// failure retried one time fewer and after a longer wait.
    pub fn third_party() -> Self {
        Self {
            network_retries: 2,
            backoff_base_ms: 3_000,
            ..Self::default()
        }
    }

    pub fn step(&self) -> Duration {
        jitter(self.step_ms)
    }

    pub fn dwell(&self) -> Duration {
        jitter(self.dwell_ms)
    }

    pub fn sitting_pause(&self) -> Duration {
        jitter(self.sitting_pause_ms)
    }

    /// The sitting this pace makes, with a rest drawn for the page that
    /// fills it.
    fn sitting_rule(&self) -> SittingRule {
        SittingRule {
            pages: self.pages_per_sitting.max(1),
            rest: self.sitting_pause(),
            idle: self.longest_rest(),
        }
    }

    fn longest_rest(&self) -> Duration {
        Duration::from_millis(self.sitting_pause_ms.0.max(self.sitting_pause_ms.1))
    }

    pub(crate) fn backoff(&self, attempt: u32) -> Duration {
        let base = self.backoff_base_ms.saturating_mul(1u64 << attempt.min(6));
        Duration::from_millis(base)
    }
}

/// Both ends included.
fn jitter((min, max): (u64, u64)) -> Duration {
    if max <= min {
        return Duration::from_millis(min);
    }
    Duration::from_millis(fastrand::u64(min..=max))
}

fn budget_error(error: RateBudgetError) -> IgError {
    IgError::Budget(error.to_string())
}

/// Who pays for a request, and who gets told when paying means waiting. The
/// only way to Instagram is through [`crate::client::IgClient`], and the only
/// way through it is past here. Its wait adds to the walker's own step.
pub struct Pacer {
    budget: Arc<dyn RateBudget>,
    cancel: CancelToken,
    /// Told before a wait the budget imposed.
    announce: Option<Arc<dyn Fn(Duration) + Send + Sync>>,
    spent: AtomicU32,
    /// The sitting, when the budget does not keep it
    /// ([`RateBudget::keeps_sitting`]): a test double's.
    sitting: std::sync::Mutex<Option<Sitting>>,
    actions: AtomicU32,
    max_wait: Option<Duration>,
    /// The latest answers this process was given, newest last.
    answers: std::sync::Mutex<std::collections::VecDeque<AnswerTime>>,
}

impl Pacer {
    pub fn new(budget: Arc<dyn RateBudget>) -> Self {
        Self {
            budget,
            cancel: CancelToken::default(),
            announce: None,
            spent: AtomicU32::new(0),
            sitting: std::sync::Mutex::new(None),
            actions: AtomicU32::new(0),
            max_wait: None,
            answers: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    /// Marks the start of an action long enough for the account to move in: a
    /// walk, each sitting after its rest, a monitor tick. What is remembered
    /// across requests is stamped with [`Self::actions`] and goes stale when a
    /// new one begins. Opening a profile and a download do not advance it.
    pub fn begin_action(&self) {
        self.actions.fetch_add(1, Ordering::Relaxed);
    }

    pub fn actions(&self) -> u32 {
        self.actions.load(Ordering::Relaxed)
    }

    /// Counts a page walked at `at` in the account's sitting
    /// (`Sitting::after_page`); the page that fills it owes a rest drawn from
    /// `pace` ([`Self::rest_owed`]). Kept by the budget, so it outlives the
    /// process and spans lists. Answers with the sitting as this page left it.
    pub async fn page_walked(&self, pace: &Pace, at: EpochMs) -> Result<Sitting, IgError> {
        let rule = pace.sitting_rule();
        if self.budget.keeps_sitting() {
            return self
                .off_thread(move |budget| budget.sitting_page(at, rule))
                .await;
        }
        let mut sitting = self.sitting.lock().unwrap_or_else(|e| e.into_inner());
        let after = Sitting::after_page(*sitting, at, rule);
        *sitting = Some(after);
        Ok(after)
    }

    /// The rest owed at `now`: when it ends, and what is left of it, never
    /// more than the longest rest of `pace` (a clock set back would otherwise
    /// make it longer).
    pub async fn rest_owed(
        &self,
        pace: &Pace,
        now: EpochMs,
    ) -> Result<Option<(EpochMs, Duration)>, IgError> {
        let until = if self.budget.keeps_sitting() {
            self.off_thread(move |budget| budget.rest_owed(now)).await?
        } else {
            self.sitting
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .and_then(|s| s.rest_owed(now))
        };
        Ok(until.map(|until| {
            let left = u64::try_from(until - now).unwrap_or(0);
            (until, Duration::from_millis(left).min(pace.longest_rest()))
        }))
    }

    /// Records that the rest ending at `until` was paid: the sitting starts
    /// again.
    pub async fn rested(&self, until: EpochMs) -> Result<(), IgError> {
        if self.budget.keeps_sitting() {
            return self.off_thread(move |budget| budget.rest_paid(until)).await;
        }
        let mut sitting = self.sitting.lock().unwrap_or_else(|e| e.into_inner());
        *sitting = sitting.map(|s| s.rested(until));
        Ok(())
    }

    /// Writes down how long an answer took and the load it declared, in the
    /// process and, when the budget keeps them, in the account's database.
    /// **A measurement never fails a request.**
    pub async fn answered(&self, answer: AnswerTime) {
        {
            let mut kept = self.answers.lock().unwrap_or_else(|e| e.into_inner());
            if kept.len() == ANSWERS_KEPT {
                kept.pop_front();
            }
            kept.push_back(answer.clone());
        }
        if self.budget.keeps_answers()
            && let Err(e) = self
                .off_thread(move |budget| budget.answer_timed(&answer))
                .await
        {
            tracing::debug!(error = %e, "could not write down how long an answer took");
        }
    }

    /// Oldest first.
    pub fn latest_answers(&self) -> Vec<AnswerTime> {
        self.answers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// `drawn`, lengthened when the account's latest healthy answers were slow
    /// ([`SlowRule::lengthen`]): **never shorter than `drawn`**. A budget that
    /// cannot be read leaves it as it is.
    pub async fn slowed(&self, pace: &Pace, drawn: Duration, now: EpochMs) -> Duration {
        let rule = pace.slow;
        if rule.factor == 0 {
            return drawn;
        }
        let since = rule.since(now);
        let recent = if self.budget.keeps_answers() {
            let samples = rule.samples;
            match self
                .off_thread(move |budget| budget.recent_waits(since, samples))
                .await
            {
                Ok(recent) => recent,
                Err(e) => {
                    tracing::debug!(error = %e, "could not read how long the latest answers took");
                    return drawn;
                }
            }
        } else {
            self.latest_answers()
                .iter()
                .rev()
                .filter(|a| a.healthy() && a.at > since)
                .map(|a| a.waited)
                .take(rule.samples)
                .collect()
        };
        let wait = rule.lengthen(drawn, &recent);
        if wait > drawn {
            tracing::info!(
                median_ms = rule
                    .floor(&recent)
                    .map_or(0, |f| f.as_millis() / u128::from(rule.factor)),
                drawn_ms = drawn.as_millis(),
                wait_ms = wait.as_millis(),
                "Instagram is answering slowly; waiting longer before the next request"
            );
        }
        wait
    }

    /// How many requests have been paid for since the client was built:
    /// what the budget was charged, retries and lookups included.
    pub fn spent(&self) -> u32 {
        self.spent.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Refuses, with [`IgError::WaitTooLong`], any wait for the
    /// budget longer than `limit` instead of sleeping through it.
    ///
    /// **Only waits for the budget**: the pace's steps and a sitting's rest
    /// are not cut short. The rest is owed in the account's database, so the
    /// next run would only owe it again.
    #[must_use]
    pub fn waiting_at_most(mut self, limit: Option<Duration>) -> Self {
        self.max_wait = limit;
        self
    }

    pub fn max_wait(&self) -> Option<Duration> {
        self.max_wait
    }

    /// `Err` when `wait` is past [`Self::max_wait`]. **A refusal stops the
    /// run**, by canceling its token: every later request would meet the same
    /// budget.
    pub(crate) fn within_the_limit(&self, wait: Duration) -> Result<(), IgError> {
        match self.max_wait {
            Some(limit) if wait > limit => {
                self.cancel.cancel();
                Err(IgError::WaitTooLong { wait, limit })
            }
            _ => Ok(()),
        }
    }

    /// Sets who gets told about a wait the budget imposed.
    #[must_use]
    pub fn announcing(mut self, announce: Arc<dyn Fn(Duration) + Send + Sync>) -> Self {
        self.announce = Some(announce);
        self
    }

    /// Grants everything and counts nothing. **Tests only**: against
    /// Instagram it would skip rate control entirely.
    #[doc(hidden)]
    pub fn unlimited() -> Self {
        Self::new(Arc::new(snob_core::budget::UnlimitedRateBudget))
    }

    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    /// Until when the account is in cooldown, if it is.
    pub fn cooldown(&self) -> Result<Option<EpochMs>, IgError> {
        self.budget.cooldown().map_err(budget_error)
    }

    /// The brake on every account, when one stands; names the accounts behind
    /// it.
    pub fn brake(&self) -> Result<Option<snob_core::budget::Brake>, IgError> {
        self.budget.brake().map_err(budget_error)
    }

    /// Records that Instagram pushed back.
    pub fn start_cooldown(&self, reason: &str, minimum: Duration) -> Result<EpochMs, IgError> {
        self.budget
            .start_cooldown(reason, minimum)
            .map_err(budget_error)
    }

    /// [`Self::cooldown`] off the async worker ([`Self::off_thread`]): it runs
    /// before every request, where a worker blocked on the database's busy
    /// timeout is a Ctrl+C that does nothing.
    pub(crate) async fn cooldown_off_thread(&self) -> Result<Option<EpochMs>, IgError> {
        self.off_thread(|budget| budget.cooldown()).await
    }

    /// How long until `accounts` more may be read today
    /// ([`RateBudget::accounts_wait`]).
    pub async fn accounts_wait(&self, accounts: u32) -> Result<Duration, IgError> {
        self.off_thread(move |budget| budget.accounts_wait(accounts))
            .await
    }

    /// Records the accounts a list page carried.
    pub async fn spend_accounts(&self, accounts: u32) -> Result<(), IgError> {
        self.off_thread(move |budget| budget.spend_accounts(accounts))
            .await
    }

    /// How many accounts may still be read today without waiting.
    pub fn accounts_left(&self) -> Result<u32, IgError> {
        self.budget.accounts_left().map_err(budget_error)
    }

    async fn charge(&self, write: bool) -> Result<Duration, IgError> {
        self.off_thread(move |budget| {
            if write {
                budget.reserve_write()
            } else {
                budget.reserve()
            }
        })
        .await
    }

    /// Asks the budget something off the async worker. `spawn_blocking`, not
    /// `block_in_place`, which panics on the current-thread runtime
    /// `#[tokio::test]` builds.
    async fn off_thread<T: Send + 'static>(
        &self,
        ask: impl FnOnce(&dyn RateBudget) -> Result<T, RateBudgetError> + Send + 'static,
    ) -> Result<T, IgError> {
        let budget = Arc::clone(&self.budget);
        tokio::task::spawn_blocking(move || ask(budget.as_ref()))
            .await
            .map_err(|e| IgError::Budget(format!("the budget task failed: {e}")))?
            .map_err(budget_error)
    }

    /// Takes a slot and waits for it. **Every request goes through here**,
    /// which is why cancellation and the cooldown are read here, before the
    /// reservation: no request is sent after cancellation, and a refusal
    /// never charges for itself.
    pub(crate) async fn clear_to_send(&self) -> Result<(), IgError> {
        self.clear(false, true).await
    }

    /// Pays for a read that has **already been sent** (a redirect hop the
    /// browser followed, `IgClient::pay_for_hops`): as [`Self::clear_to_send`]
    /// but for the limit on the wait, which cannot un-send anything.
    pub(crate) async fn pay_for_sent(&self) -> Result<(), IgError> {
        self.clear(false, false).await
    }

    /// The same for a write, which pays the read budgets and the write
    /// bucket. A separate method, not a flag a caller could pass wrongly.
    pub(crate) async fn clear_to_send_write(&self) -> Result<(), IgError> {
        self.clear(true, true).await
    }

    /// Waits until a write would be sent without waiting, and spends nothing:
    /// a write from the browser pays that wait before its profile is loaded
    /// (`IgClient::write_from_the_page`). [`Self::clear_to_send_write`] still
    /// charges it.
    pub(crate) async fn rested_for_a_write(&self) -> Result<(), IgError> {
        if self.cancel.is_canceled() {
            return Err(IgError::Canceled);
        }
        if let Some(until_ms) = self.cooldown_off_thread().await? {
            return Err(IgError::InCooldown { until_ms });
        }
        let owed = self.off_thread(|budget| budget.write_wait()).await?;
        if owed.is_zero() {
            return Ok(());
        }
        self.within_the_limit(owed)?;
        if let Some(announce) = &self.announce {
            announce(owed);
        }
        if self.cancel.sleep_or_cancel(owed).await {
            return Err(IgError::Canceled);
        }
        Ok(())
    }

    async fn clear(&self, write: bool, limited: bool) -> Result<(), IgError> {
        if self.cancel.is_canceled() {
            return Err(IgError::Canceled);
        }

        // **Nothing is spent during a cooldown.** The budget charges its
        // buckets without reading the `cooldowns` table, so this is the net
        // under the callers' own gates: every request passes through here.
        // (`SNOB_IGNORE_COOLDOWN` is read inside `SqliteRateBudget::cooldown`.)
        if let Some(until_ms) = self.cooldown_off_thread().await? {
            return Err(IgError::InCooldown { until_ms });
        }

        // Asked before the charge, which is committed whether or not the
        // request goes. Another process can still take a slot in between;
        // that longer wait is slept through.
        if limited && self.max_wait.is_some() {
            let owed = self
                .off_thread(move |budget| {
                    if write {
                        budget.write_wait()
                    } else {
                        budget.read_wait()
                    }
                })
                .await?;
            self.within_the_limit(owed)?;
        }

        let owed = self.charge(write).await?;
        // Counted at the reservation: the budget has been charged by now.
        self.spent.fetch_add(1, Ordering::Relaxed);

        if owed.is_zero() {
            return Ok(());
        }
        if let Some(announce) = &self.announce {
            announce(owed);
        }
        if self.cancel.sleep_or_cancel(owed).await {
            return Err(IgError::Canceled);
        }
        Ok(())
    }
}

/// Shared cancellation token.
#[derive(Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl CancelToken {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub fn is_canceled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// Resolves when the run is canceled, and never otherwise.
    pub async fn canceled(&self) {
        loop {
            // Register before checking the flag, or a notification arriving
            // between the two is lost.
            let pending = self.notify.notified();
            if self.is_canceled() {
                return;
            }
            pending.await;
        }
    }

    /// Returns `true` if canceled.
    pub async fn sleep_or_cancel(&self, duration: Duration) -> bool {
        if self.is_canceled() {
            return true;
        }
        tokio::select! {
            biased;
            () = self.canceled() => true,
            _ = tokio::time::sleep(duration) => false,
        }
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct KeptSitting {
    sitting: std::sync::Mutex<Option<Sitting>>,
}

#[cfg(test)]
impl KeptSitting {
    pub(crate) fn sitting(&self) -> Option<Sitting> {
        *self.sitting.lock().unwrap()
    }
}

#[cfg(test)]
impl RateBudget for KeptSitting {
    fn reserve(&self) -> Result<Duration, RateBudgetError> {
        Ok(Duration::ZERO)
    }
    fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
        Ok(Duration::ZERO)
    }
    fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
        Ok(None)
    }
    fn start_cooldown(
        &self,
        _reason: &str,
        _minimum: Duration,
    ) -> Result<EpochMs, RateBudgetError> {
        Ok(EpochMs::new(0))
    }
    fn keeps_sitting(&self) -> bool {
        true
    }
    fn sitting_page(&self, at: EpochMs, rule: SittingRule) -> Result<Sitting, RateBudgetError> {
        let mut sitting = self.sitting.lock().unwrap();
        let after = Sitting::after_page(*sitting, at, rule);
        *sitting = Some(after);
        Ok(after)
    }
    fn rest_owed(&self, now: EpochMs) -> Result<Option<EpochMs>, RateBudgetError> {
        Ok(self.sitting().and_then(|s| s.rest_owed(now)))
    }
    fn rest_paid(&self, until: EpochMs) -> Result<(), RateBudgetError> {
        let mut sitting = self.sitting.lock().unwrap();
        *sitting = sitting.map(|s| s.rested(until));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(at: EpochMs, status: u16, waited_ms: u64) -> AnswerTime {
        AnswerTime {
            at,
            operation: "followers".to_string(),
            status,
            waited: Duration::from_millis(waited_ms),
            load: None,
            via: snob_core::budget::Via::Browser,
        }
    }

    /// A budget that keeps the answers, as the account's database does,
    /// and hands back the waits it was given.
    struct KeptAnswers(Vec<Duration>);

    impl RateBudget for KeptAnswers {
        fn reserve(&self) -> Result<Duration, RateBudgetError> {
            Ok(Duration::ZERO)
        }
        fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
            Ok(Duration::ZERO)
        }
        fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
            Ok(None)
        }
        fn start_cooldown(&self, _: &str, _: Duration) -> Result<EpochMs, RateBudgetError> {
            Ok(EpochMs::new(0))
        }
        fn keeps_answers(&self) -> bool {
            true
        }
        fn recent_waits(&self, _: EpochMs, n: usize) -> Result<Vec<Duration>, RateBudgetError> {
            Ok(self.0.iter().take(n).copied().collect())
        }
    }

    /// The answers another process was given, kept by the budget, slow a
    /// pacer that was given none itself.
    #[tokio::test]
    async fn the_answers_the_budget_keeps_slow_a_new_pacer() {
        let slow = [4_000, 3_500, 3_800].map(Duration::from_millis).to_vec();
        let pacer = Pacer::new(Arc::new(KeptAnswers(slow)));
        let drawn = Duration::from_millis(1_500);
        let wait = pacer
            .slowed(&Pace::default(), drawn, EpochMs::new(1_700_000_000_000))
            .await;
        assert_eq!(wait, Duration::from_millis(7_600));
    }

    /// Kept in the process, only the healthy answers inside the window
    /// count: a refusal answers fast, and an answer from before a long
    /// rest says nothing about the server now.
    #[tokio::test]
    async fn the_process_counts_only_healthy_answers_inside_the_window() {
        let pacer = Pacer::unlimited();
        let now = EpochMs::new(1_700_000_000_000);
        let long_ago = now - Duration::from_secs(20 * 60);
        for _ in 0..3 {
            pacer.answered(timed(long_ago, 200, 9_000)).await;
        }
        for _ in 0..2 {
            pacer.answered(timed(now, 200, 5_000)).await;
            pacer.answered(timed(now, 429, 10)).await;
        }
        let drawn = Duration::from_millis(1_500);
        assert_eq!(pacer.slowed(&Pace::default(), drawn, now).await, drawn);

        pacer.answered(timed(now, 200, 5_000)).await;
        assert_eq!(
            pacer.slowed(&Pace::default(), drawn, now).await,
            Duration::from_secs(10)
        );
    }

    /// The process keeps the last few answers and no more.
    #[tokio::test]
    async fn the_process_keeps_the_latest_answers() {
        let pacer = Pacer::unlimited();
        for ms in 0..20 {
            pacer
                .answered(timed(EpochMs::new(ms), 200, ms as u64))
                .await;
        }
        let kept: Vec<_> = pacer
            .latest_answers()
            .iter()
            .map(|a| a.waited.as_millis())
            .collect();
        assert_eq!(kept, (12..20).collect::<Vec<u128>>());
    }

    /// The numbers the rule is held to, documented on each constant.
    #[test]
    fn the_slow_rule_is_the_documented_one() {
        let slow = Pace::default().slow;
        assert_eq!(slow.factor, 2);
        assert_eq!(slow.samples, 5);
        assert_eq!(slow.window, Duration::from_secs(600));
        assert_eq!(slow.ceiling, Duration::from_secs(60));
        assert!(slow.window < Duration::from_millis(SITTING_PAUSE_MS.1));
    }

    /// A budget that is always a quarter of an hour away, and counts what it
    /// was charged.
    #[derive(Default)]
    struct Rationing {
        charged: AtomicU32,
    }

    impl RateBudget for Rationing {
        fn reserve(&self) -> Result<Duration, RateBudgetError> {
            self.charged.fetch_add(1, Ordering::Relaxed);
            Ok(Duration::from_secs(900))
        }
        fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
            self.reserve()
        }
        fn read_wait(&self) -> Result<Duration, RateBudgetError> {
            Ok(Duration::from_secs(900))
        }
        fn write_wait(&self) -> Result<Duration, RateBudgetError> {
            Ok(Duration::from_secs(900))
        }
        fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
            Ok(None)
        }
        fn start_cooldown(
            &self,
            _reason: &str,
            _minimum: Duration,
        ) -> Result<EpochMs, RateBudgetError> {
            Ok(EpochMs::new(0))
        }
    }

    /// Past the limit, a read and a write are refused before the charge, so
    /// a refusal costs nothing; with no limit nothing is refused.
    #[tokio::test]
    async fn a_wait_past_the_limit_is_refused_before_anything_is_charged() {
        let budget = Arc::new(Rationing::default());
        let limited = || Pacer::new(budget.clone()).waiting_at_most(Some(Duration::from_secs(60)));

        let (read, write, rested) = (limited(), limited(), limited());
        for refused in [
            read.clear_to_send().await,
            write.clear_to_send_write().await,
            rested.rested_for_a_write().await,
        ] {
            assert!(
                matches!(refused, Err(IgError::WaitTooLong { .. })),
                "{refused:?}"
            );
        }
        assert_eq!(budget.charged.load(Ordering::Relaxed), 0);
        assert_eq!(read.spent(), 0);

        // The refusal stops the run, as Ctrl+C does: nothing after it goes.
        assert!(read.cancel_token().is_canceled());
        assert!(matches!(read.clear_to_send().await, Err(IgError::Canceled)));

        // What was already sent is paid for past the limit.
        let sent = Pacer::new(budget.clone()).waiting_at_most(Some(Duration::from_secs(60)));
        let paid = tokio::time::timeout(Duration::from_millis(50), sent.pay_for_sent()).await;
        assert!(
            paid.is_err(),
            "it waits for the charge rather than refusing"
        );
        assert_eq!(budget.charged.load(Ordering::Relaxed), 1);

        let generous = Pacer::new(budget).waiting_at_most(Some(Duration::from_secs(3600)));
        assert!(generous.within_the_limit(Duration::from_secs(900)).is_ok());
        assert!(Pacer::unlimited().within_the_limit(Duration::MAX).is_ok());
    }

    #[test]
    fn jitter_lands_inside_the_range() {
        for _ in 0..200 {
            let d = jitter((500, 2_000)).as_millis() as u64;
            assert!((500..=2_000).contains(&d), "{d} is out of range");
        }
    }

    #[test]
    fn a_degenerate_range_does_not_panic() {
        // fastrand panics on an empty range, and the test pace has zeroes.
        assert_eq!(jitter((0, 0)), Duration::ZERO);
        assert_eq!(jitter((100, 100)), Duration::from_millis(100));
        assert_eq!(jitter((100, 50)), Duration::from_millis(100));
    }

    #[test]
    fn the_default_pace_is_the_documented_one() {
        assert_eq!(STEP_MS, (1_000, 3_000), "one to three seconds a page");
        assert_eq!(
            COMMENT_STEP_MS,
            (2_000, 5_000),
            "two to five seconds a page of comments"
        );
        assert_eq!(
            DWELL_MS,
            (1_500, 4_000),
            "a second and a half to four before a list"
        );
        assert_eq!(PAGES_PER_SITTING, 40, "about 480 accounts a sitting");
        assert_eq!(
            SITTING_PAUSE_MS,
            (300_000, 900_000),
            "five to fifteen minutes of rest"
        );

        let p = Pace::default();
        assert_eq!(p.per_page, 12);
        assert_eq!(p.step_ms, STEP_MS);
        assert_eq!(p.dwell_ms, DWELL_MS);
        assert_eq!(p.sitting_pause_ms, SITTING_PAUSE_MS);
        assert_eq!(p.pages_per_sitting, PAGES_PER_SITTING);
        assert_eq!(p.network_retries, 3);
        assert_eq!(p.backoff_base_ms, 2_000);
    }

    /// Somebody else's lists are read at the same cadence, and a network
    /// failure on them is insisted on less.
    #[test]
    fn the_third_party_pace_differs_only_in_its_retries() {
        let own = Pace::default();
        let other = Pace::third_party();

        assert_eq!(other.per_page, own.per_page);
        assert_eq!(other.step_ms, own.step_ms);
        assert_eq!(other.dwell_ms, own.dwell_ms);
        assert_eq!(other.sitting_pause_ms, own.sitting_pause_ms);
        assert_eq!(other.pages_per_sitting, own.pages_per_sitting);

        assert_eq!(other.network_retries, 2);
        assert_eq!(other.backoff_base_ms, 3_000);
    }

    #[test]
    fn the_waits_land_inside_their_ranges() {
        let p = Pace::default();
        for _ in 0..200 {
            let step = p.step().as_millis() as u64;
            assert!((1_000..=3_000).contains(&step), "{step}");
            let dwell = p.dwell().as_millis() as u64;
            assert!((1_500..=4_000).contains(&dwell), "{dwell}");
            let rest = p.sitting_pause().as_millis() as u64;
            assert!((300_000..=900_000).contains(&rest), "{rest}");
        }
    }

    /// A sitting is counted across walks, fills at its fortieth page and
    /// owes a rest from it, stays owed until it is paid, and starts over
    /// after; a pacer over a budget that keeps nothing counts it the same.
    #[tokio::test]
    async fn a_sitting_fills_at_its_fortieth_page_and_starts_over_after_its_rest() {
        let pacer = Pacer::unlimited();
        let pace = Pace::default();
        let at = EpochMs::new(1_800_000_000_000);
        for _ in 1..PAGES_PER_SITTING {
            pacer.page_walked(&pace, at).await.unwrap();
        }
        assert_eq!(
            pacer.rest_owed(&pace, at).await.unwrap(),
            None,
            "mid-sitting"
        );

        pacer.page_walked(&pace, at).await.unwrap();
        let (until, left) = pacer.rest_owed(&pace, at).await.unwrap().expect("full");
        assert!(
            (300_000..=900_000).contains(&(left.as_millis() as u64)),
            "{left:?}"
        );
        pacer.page_walked(&pace, at).await.unwrap();
        assert_eq!(
            pacer.rest_owed(&pace, at).await.unwrap().map(|o| o.0),
            Some(until),
            "and it stays owed until paid"
        );

        pacer.rested(until).await.unwrap();
        assert_eq!(pacer.rest_owed(&pace, at).await.unwrap(), None);
        pacer.page_walked(&pace, at).await.unwrap();
        assert_eq!(
            pacer.rest_owed(&pace, at).await.unwrap(),
            None,
            "a new sitting"
        );
    }

    /// A run that stopped once the page that filled its sitting was walked
    /// leaves the rest owed in the budget: the next run, with a pacer of its
    /// own, finds what is left of it before its first page, however few it
    /// means to walk.
    #[tokio::test]
    async fn a_rest_owed_by_an_earlier_run_is_paid_before_the_next_page() {
        let budget = Arc::new(KeptSitting::default());
        let pace = Pace::default();
        let at = EpochMs::new(1_800_000_000_000);

        let earlier = Pacer::new(budget.clone());
        for _ in 0..PAGES_PER_SITTING {
            earlier.page_walked(&pace, at).await.unwrap();
        }
        drop(earlier);

        let next = Pacer::new(budget.clone());
        let minute_later = at + Duration::from_secs(60);
        let (until, left) = next
            .rest_owed(&pace, minute_later)
            .await
            .unwrap()
            .expect("the rest is the account's, not the run's");
        assert_eq!(Some(until), budget.sitting().unwrap().rest_until);
        assert_eq!(left, Duration::from_millis((until - minute_later) as u64));
        assert!(
            left <= Duration::from_secs(14 * 60),
            "a minute of it is gone"
        );

        next.rested(until).await.unwrap();
        assert_eq!(budget.sitting().unwrap().pages, 0);
        assert_eq!(next.rest_owed(&pace, minute_later).await.unwrap(), None);
    }

    /// A rest ending further off than the longest rest was drawn before the
    /// clock went back, and is waited for no longer than the longest rest.
    #[tokio::test]
    async fn a_rest_owed_is_never_longer_than_the_longest_rest() {
        let budget = Arc::new(KeptSitting::default());
        let pacer = Pacer::new(budget.clone());
        let pace = Pace::default();
        let at = EpochMs::new(1_800_000_000_000);
        for _ in 0..PAGES_PER_SITTING {
            pacer.page_walked(&pace, at).await.unwrap();
        }
        let set_back = at - Duration::from_secs(86_400);
        let (_, left) = pacer.rest_owed(&pace, set_back).await.unwrap().unwrap();
        assert_eq!(left, Duration::from_millis(SITTING_PAUSE_MS.1));
    }

    /// An action is counted only when one begins: a request spent is not one.
    #[tokio::test]
    async fn actions_are_counted_apart_from_requests() {
        let pacer = Pacer::unlimited();
        assert_eq!(pacer.actions(), 0);
        pacer.clear_to_send().await.unwrap();
        pacer.clear_to_send().await.unwrap();
        assert_eq!(pacer.actions(), 0, "requests begin no action");

        pacer.begin_action();
        assert_eq!(pacer.actions(), 1);
        assert_eq!(pacer.spent(), 2, "and an action spends nothing");
    }

    #[test]
    fn the_backoff_grows_and_does_not_overflow() {
        let p = Pace::default();
        assert_eq!(p.backoff(0), Duration::from_millis(2_000));
        assert_eq!(p.backoff(1), Duration::from_millis(4_000));
        assert_eq!(p.backoff(2), Duration::from_millis(8_000));
        // Even given an absurd number.
        assert!(p.backoff(99) <= Duration::from_millis(2_000 * 64));
    }

    #[tokio::test]
    async fn canceling_interrupts_a_long_wait() {
        let c = CancelToken::default();
        let copy = c.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            copy.cancel();
        });

        let start = std::time::Instant::now();
        let canceled = c.sleep_or_cancel(Duration::from_secs(30)).await;
        assert!(canceled);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the wait was not interrupted"
        );
    }

    #[tokio::test]
    async fn without_cancellation_the_wait_finishes_on_its_own() {
        let c = CancelToken::default();
        assert!(!c.sleep_or_cancel(Duration::from_millis(1)).await);
    }

    #[tokio::test]
    async fn canceling_before_waiting_is_still_noticed() {
        let c = CancelToken::default();
        c.cancel();
        assert!(c.sleep_or_cancel(Duration::from_secs(30)).await);
    }

    /// A canceled run is never cleared to send, even with nothing owed.
    ///
    /// The ordinary case is a budget that owes nothing, so a token read only
    /// inside the wait would let the request go out.
    #[tokio::test]
    async fn a_canceled_run_is_not_cleared_to_send() {
        let pacer = Pacer::unlimited();
        pacer
            .clear_to_send()
            .await
            .expect("nothing is canceled yet");
        assert_eq!(pacer.spent(), 1);

        pacer.cancel_token().cancel();

        let error = pacer.clear_to_send().await.unwrap_err();
        assert!(matches!(error, IgError::Canceled));
        assert_eq!(
            pacer.spent(),
            1,
            "a request that is refused is not charged for"
        );
    }

    use RateBudgetError;

    /// A budget that is in cooldown and counts every reservation it is asked
    /// for, so a test can assert that it was asked for none.
    struct Cooling {
        until_ms: EpochMs,
        reserved: AtomicU32,
    }

    impl RateBudget for Cooling {
        fn reserve(&self) -> Result<Duration, RateBudgetError> {
            self.reserved.fetch_add(1, Ordering::Relaxed);
            Ok(Duration::ZERO)
        }

        fn reserve_write(&self) -> Result<Duration, RateBudgetError> {
            self.reserved.fetch_add(1, Ordering::Relaxed);
            Ok(Duration::ZERO)
        }

        fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError> {
            Ok(Some(self.until_ms))
        }

        fn start_cooldown(
            &self,
            _reason: &str,
            _minimum: Duration,
        ) -> Result<EpochMs, RateBudgetError> {
            Ok(self.until_ms)
        }
    }

    /// Nothing is spent during a cooldown, whoever asks and whether or not they
    /// remembered to check first.
    ///
    /// This is the backstop rather than the gates: every caller in `snob-cli`
    /// asks `Pacer::cooldown` before it gets here, and one of them — the command
    /// written to be polled by a monitoring system — did not, and spent a
    /// request per configured account per poll against a door that was shut. The
    /// budget charges its buckets without ever reading the `cooldowns` table, so
    /// until `clear` read it the rule lived in eight places and held in seven.
    #[tokio::test]
    async fn nothing_is_cleared_to_send_during_a_cooldown() {
        let until_ms = EpochMs::new(1_722_700_000_000);
        let budget = Arc::new(Cooling {
            until_ms,
            reserved: AtomicU32::new(0),
        });
        let pacer = Pacer::new(Arc::clone(&budget) as Arc<dyn RateBudget>);

        let error = pacer.clear_to_send().await.unwrap_err();
        assert!(
            matches!(error, IgError::InCooldown { until_ms: u } if u == until_ms),
            "a read during a cooldown answers InCooldown, carrying when it lifts: {error:?}"
        );

        // A write pays out of a second bucket, so it gets its own arm here: a
        // backstop that covered only reads would leave the one request class
        // that earns the twelve-hour cooldown uncovered.
        let error = pacer.clear_to_send_write().await.unwrap_err();
        assert!(matches!(
            error,
            IgError::InCooldown { until_ms: u } if u == until_ms
        ));

        assert_eq!(
            budget.reserved.load(Ordering::Relaxed),
            0,
            "the refusal comes before the reservation, so neither bucket was charged"
        );
        assert_eq!(pacer.spent(), 0, "and nothing refused is reported as spent");
    }
}
