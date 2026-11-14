//! The request budget, as an interface: how to ask for a slot, whether the
//! account is in cooldown, and how long each kind of cooldown lasts.
//!
//! **A port.** `snob-ig` needs the trait and must not depend on the crate that
//! opens SQLite to get it; the implementation is
//! `snob_store::store::rate_budget`. The cooldown lengths and the sitting's
//! arithmetic are here because they are policy, not storage.

use std::time::Duration;

use crate::{EpochMs, Pk};

/// Cooldown after Instagram throttles the account.
pub const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(2 * 3600);

/// Cooldown after Instagram blocks an action on the account.
pub const ACTION_BLOCK_COOLDOWN: Duration = Duration::from_secs(12 * 3600);

/// Cooldown after Instagram asks for the account to be verified. Short,
/// because waiting does not clear a challenge, the user opening the link
/// does: it only keeps a scheduled run from knocking again at once.
pub const CHALLENGE_COOLDOWN: Duration = Duration::from_secs(30 * 60);

/// How long to wait before firing a request.
///
/// Every method with a default is one a test double need not answer; the real
/// budget, `SqliteRateBudget`, overrides them all.
pub trait RateBudget: Send + Sync {
    /// Reserves a request and returns how long to wait before sending it.
    /// **Committed even if the request is never made**: overcharging is the
    /// safe direction to be wrong in.
    fn reserve(&self) -> Result<Duration, RateBudgetError>;

    /// Reserves a **write** (a follow or an unfollow): everything a read pays
    /// and the write bucket on top.
    fn reserve_write(&self) -> Result<Duration, RateBudgetError>;

    /// How long [`Self::reserve_write`] would wait now, without spending. A
    /// write from the browser waits this out before it loads the profile it
    /// is built on, which the browser would not outlast.
    fn write_wait(&self) -> Result<Duration, RateBudgetError> {
        Ok(Duration::ZERO)
    }

    /// How long [`Self::reserve`] would wait now, without spending, so a
    /// caller with a limit on its wait can refuse before it is charged.
    fn read_wait(&self) -> Result<Duration, RateBudgetError> {
        Ok(Duration::ZERO)
    }

    /// Until when the account is in cooldown.
    fn cooldown(&self) -> Result<Option<EpochMs>, RateBudgetError>;

    /// Puts the account in cooldown and returns until when.
    fn start_cooldown(&self, reason: &str, minimum: Duration) -> Result<EpochMs, RateBudgetError>;

    /// How long until `accounts` more may be read off a list within
    /// [`accounts_per_day`], without spending.
    ///
    /// **The budget the account is judged on**: Meta's paper on the system
    /// behind the automated-activity warning (arXiv 2502.17693) weighs each
    /// request by how many accounts it returns.
    fn accounts_wait(&self, accounts: u32) -> Result<Duration, RateBudgetError> {
        let _ = accounts;
        Ok(Duration::ZERO)
    }

    /// Records that `accounts` were read off a list.
    fn spend_accounts(&self, accounts: u32) -> Result<(), RateBudgetError> {
        let _ = accounts;
        Ok(())
    }

    /// How many accounts may still be read today without waiting.
    fn accounts_left(&self) -> Result<u32, RateBudgetError> {
        Ok(u32::MAX)
    }

    /// The brake on every account, when one stands ([`common_brake`]).
    /// [`Self::cooldown`] already counts it; this says which accounts.
    fn brake(&self) -> Result<Option<Brake>, RateBudgetError> {
        Ok(None)
    }

    /// Whether this budget keeps the [`Sitting`], so that it outlives the
    /// process. When not, the pacer counts it in the process.
    fn keeps_sitting(&self) -> bool {
        false
    }

    /// Records a page of a list walked at `at` and answers the sitting it
    /// leaves, in one transaction: two processes walking the same account
    /// count each other's pages.
    fn sitting_page(&self, at: EpochMs, rule: SittingRule) -> Result<Sitting, RateBudgetError> {
        Ok(Sitting::after_page(None, at, rule))
    }

    /// Until when the rest the sitting owes runs, if still owed at `now`.
    fn rest_owed(&self, now: EpochMs) -> Result<Option<EpochMs>, RateBudgetError> {
        let _ = now;
        Ok(None)
    }

    /// Records that the rest running until `until` was paid. Any other rest
    /// (a new sitting filled since) is left as it is.
    fn rest_paid(&self, until: EpochMs) -> Result<(), RateBudgetError> {
        let _ = until;
        Ok(())
    }

    /// Whether this budget keeps the answers' times for every process of the
    /// account. When not, the pacer keeps the last few in the process.
    fn keeps_answers(&self) -> bool {
        false
    }

    fn answer_timed(&self, answer: &AnswerTime) -> Result<(), RateBudgetError> {
        let _ = answer;
        Ok(())
    }

    /// How long the healthy answers received after `since` took, newest
    /// first, at most `n`: what [`SlowRule::floor`] reads.
    fn recent_waits(&self, since: EpochMs, n: usize) -> Result<Vec<Duration>, RateBudgetError> {
        let _ = (since, n);
        Ok(Vec::new())
    }
}

/// How a request left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// `fetch()` from an instagram.com tab, timed inside the tab.
    Browser,
    /// `reqwest`, under `SNOB_NO_BROWSER`.
    Direct,
}

impl Via {
    /// The name it is stored and shown under.
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Browser => "browser",
            Via::Direct => "direct",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "browser" => Some(Via::Browser),
            "direct" => Some(Via::Direct),
            _ => None,
        }
    }
}

/// How long one answer of the API took to arrive, **to its headers**: the
/// server's thinking and one round trip, not the size of the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerTime {
    pub at: EpochMs,
    /// The call's name as the allowlist knows it, **never its path**: a path
    /// carries the account it was about.
    pub operation: String,
    pub status: u16,
    pub waited: Duration,
    /// `x-ig-capacity-level` and `x-ig-peak-time`, as `snob_ig` joins them.
    /// Written down, never acted on.
    pub load: Option<String>,
    pub via: Via,
}

impl AnswerTime {
    /// A 2xx: its time says how busy the server was, not how quickly it
    /// refused.
    pub fn healthy(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// When answers are slow enough to lengthen the waits between them; the
/// numbers are the pace's (`snob_ig::pace::Pace`). **It only ever lengthens a
/// wait.**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowRule {
    /// The floor is this many times the median recent answer. Zero turns
    /// the rule off.
    pub factor: u32,
    /// How many of the latest answers the median is over.
    pub samples: usize,
    /// How far back an answer still counts.
    pub window: Duration,
    /// The longest floor the rule sets.
    pub ceiling: Duration,
}

impl SlowRule {
    pub const OFF: SlowRule = SlowRule {
        factor: 0,
        samples: 0,
        window: Duration::ZERO,
        ceiling: Duration::ZERO,
    };

    /// The oldest moment an answer read at `now` still counts from.
    pub fn since(&self, now: EpochMs) -> EpochMs {
        now - self.window
    }

    /// The fewest answers the rule judges from: a majority of
    /// [`Self::samples`], so one slow answer is not a slow server.
    pub fn least(&self) -> usize {
        self.samples / 2 + 1
    }

    /// The shortest wait the answers in `recent` (newest first, healthy and
    /// inside the window) allow before the next request: [`Self::factor`]
    /// times the median of the first [`Self::samples`], at most
    /// [`Self::ceiling`]. `None` when off or under [`Self::least`] answers.
    pub fn floor(&self, recent: &[Duration]) -> Option<Duration> {
        if self.factor == 0 || self.samples == 0 {
            return None;
        }
        let mut latest: Vec<Duration> = recent.iter().take(self.samples).copied().collect();
        if latest.len() < self.least() {
            return None;
        }
        latest.sort_unstable();
        let median = latest[latest.len() / 2];
        Some(median.saturating_mul(self.factor).min(self.ceiling))
    }

    /// `drawn`, or the floor `recent` sets when that is longer.
    pub fn lengthen(&self, drawn: Duration, recent: &[Duration]) -> Duration {
        self.floor(recent).map_or(drawn, |floor| floor.max(drawn))
    }
}

/// The pages of lists walked on one account since its last rest, and the
/// rest they owe once they fill a sitting.
///
/// **A rest is owed, not taken on the spot**: the page that fills a sitting
/// writes down when its rest ends, and the next page of any walk of the
/// account pays what is left of it first. The arithmetic is here so the
/// database that keeps it and the pacer that counts it in a process are the
/// same rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sitting {
    pub pages: u32,
    pub last_page_at: EpochMs,
    /// When the rest it owes ends, once it is full; `None` before.
    pub rest_until: Option<EpochMs>,
}

/// What makes a sitting, from the pace (`snob_ig::pace::Pace`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SittingRule {
    /// How many pages fill it.
    pub pages: u32,
    /// The rest the page that fills it owes, drawn by the caller.
    pub rest: Duration,
    /// How long with no page a sitting that is not full ends on its own:
    /// **the longest rest**. A shorter gap would let a run stopped and
    /// started again walk a second sitting with no rest for the first.
    pub idle: Duration,
}

impl Sitting {
    /// The sitting after a page walked at `at`, from the one `before` it.
    ///
    /// - A rest that has ended by `at` ends its sitting: this page is the
    ///   first of the next.
    /// - A rest still owed stays owed, and the page counts in the full
    ///   sitting.
    /// - A sitting not full and left alone longer than [`SittingRule::idle`]
    ///   ends too. A clock set back is not idleness.
    /// - The page that brings the count to [`SittingRule::pages`] owes
    ///   [`SittingRule::rest`] from `at`.
    pub fn after_page(before: Option<Sitting>, at: EpochMs, rule: SittingRule) -> Sitting {
        let idle = i64::try_from(rule.idle.as_millis()).unwrap_or(i64::MAX);
        let (pages, owed) = match before {
            None => (0, None),
            Some(s) => match s.rest_until {
                Some(until) if at >= until => (0, None),
                Some(until) => (s.pages, Some(until)),
                None if at - s.last_page_at > idle => (0, None),
                None => (s.pages, None),
            },
        };
        let pages = pages.saturating_add(1);
        let rest_until = owed.or_else(|| (pages >= rule.pages.max(1)).then(|| at + rule.rest));
        Sitting {
            pages,
            last_page_at: at,
            rest_until,
        }
    }

    /// When the rest ends, if it is still owed at `now`.
    pub fn rest_owed(&self, now: EpochMs) -> Option<EpochMs> {
        self.rest_until.filter(|until| *until > now)
    }

    /// The sitting once the rest running until `until` is paid: a new one.
    /// Any other rest stays owed.
    #[must_use]
    pub fn rested(self, until: EpochMs) -> Sitting {
        if self.rest_until == Some(until) {
            Sitting {
                pages: 0,
                rest_until: None,
                ..self
            }
        } else {
            self
        }
    }

    /// How many pages into a sitting the next page walked at `now` would
    /// come after, as [`Self::after_page`] counts. For an estimate.
    pub fn pages_at(&self, now: EpochMs, idle: Duration) -> u32 {
        let rule = SittingRule {
            pages: u32::MAX,
            rest: Duration::ZERO,
            idle,
        };
        // `after_page` counts the page asked about, so it answers one or more.
        Self::after_page(Some(*self), now, rule).pages - 1
    }
}

/// How close together push-backs on two different accounts have to be for
/// every account to stop: within the hour, Instagram is answering the
/// machine and not either account.
pub const COMMON_BRAKE_WINDOW: Duration = Duration::from_secs(3600);

/// One push-back, as every account's budget records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushBack {
    pub pk: Pk,
    pub at: EpochMs,
    /// When the cooldown it started on that account ends.
    pub until: EpochMs,
}

/// Every account paused until `until`, because of push-backs on `accounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brake {
    pub until: EpochMs,
    /// In the order they were first pushed back on.
    pub accounts: Vec<Pk>,
}

/// The brake the push-backs impose at `now`, if any: two on **different**
/// accounts less than [`COMMON_BRAKE_WINDOW`] apart stop every account until
/// the later of their two cooldowns.
pub fn common_brake(pushbacks: &[PushBack], now: EpochMs) -> Option<Brake> {
    let window = COMMON_BRAKE_WINDOW.as_millis() as i64;
    let mut sorted: Vec<&PushBack> = pushbacks.iter().collect();
    sorted.sort_by_key(|p| p.at);

    let mut brake: Option<Brake> = None;
    for (i, a) in sorted.iter().enumerate() {
        // Sorted by time, so the first one outside the window ends the pairs.
        for b in sorted[i + 1..].iter().take_while(|b| b.at - a.at < window) {
            let until = a.until.max(b.until);
            if a.pk == b.pk || until <= now {
                continue;
            }
            let brake = brake.get_or_insert(Brake {
                until,
                accounts: Vec::new(),
            });
            brake.until = brake.until.max(until);
            for pk in [a.pk, b.pk] {
                if !brake.accounts.contains(&pk) {
                    brake.accounts.push(pk);
                }
            }
        }
    }
    brake
}

/// Accounts a day read off follower and following lists. A ceiling, not a
/// target, and the closest thing to a measured number anybody has published:
/// misiektoja/instagram_monitor 4.0 (September 2026) defaults to it, and
/// InstagramUnfollowers users were logged out for automated activity at about
/// 1,600 in one sitting that month.
const ACCOUNTS_PER_DAY: u32 = 2_000;

/// The same after Instagram has pushed back on this account recently.
const ACCOUNTS_PER_DAY_AFTER_PUSH_BACK: u32 = 1_000;

/// How long a push-back keeps the lower ceiling in force.
const PUSH_BACK_MEMORY: Duration = Duration::from_secs(7 * 24 * 3600);

/// The daily ceiling on accounts read, given when the last cooldown began.
pub fn accounts_per_day(last_push_back: Option<EpochMs>, now: EpochMs) -> u32 {
    match last_push_back {
        Some(at) if now - at < PUSH_BACK_MEMORY.as_millis() as i64 => {
            ACCOUNTS_PER_DAY_AFTER_PUSH_BACK
        }
        _ => ACCOUNTS_PER_DAY,
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RateBudgetError(pub String);

/// Grants everything and counts nothing. **Tests only**: using it against
/// Instagram skips rate control entirely.
#[doc(hidden)]
pub struct UnlimitedRateBudget;

impl RateBudget for UnlimitedRateBudget {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn pushed(pk: u64, at_min: i64, for_min: i64) -> PushBack {
        PushBack {
            pk: Pk::new(pk),
            at: EpochMs::new(at_min * MINUTE),
            until: EpochMs::new((at_min + for_min) * MINUTE),
        }
    }

    const SLOW: SlowRule = SlowRule {
        factor: 2,
        samples: 5,
        window: Duration::from_secs(600),
        ceiling: Duration::from_secs(60),
    };

    fn ms(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|v| Duration::from_millis(*v)).collect()
    }

    #[test]
    fn no_answers_set_no_floor() {
        assert_eq!(SLOW.floor(&[]), None);
    }

    #[test]
    fn fewer_answers_than_a_majority_set_no_floor() {
        assert_eq!(SLOW.floor(&ms(&[4_000, 4_000])), None);
    }

    #[test]
    fn the_floor_is_twice_the_median_of_the_latest_five() {
        let recent = ms(&[3_000, 200, 2_500, 2_800, 250, 9_000, 9_000]);
        assert_eq!(SLOW.floor(&recent), Some(Duration::from_millis(5_000)));
    }

    #[test]
    fn one_slow_answer_among_fast_ones_does_not_lengthen_a_step() {
        let recent = ms(&[300, 280, 5_000, 310, 290]);
        let drawn = Duration::from_millis(1_200);
        assert_eq!(SLOW.lengthen(drawn, &recent), drawn);
    }

    #[test]
    fn fast_answers_leave_the_drawn_wait_alone() {
        let recent = ms(&[400, 450, 380, 420, 410]);
        for drawn in [1_000, 2_000, 3_000].map(Duration::from_millis) {
            assert_eq!(SLOW.lengthen(drawn, &recent), drawn);
        }
    }

    #[test]
    fn slow_answers_lengthen_the_wait_up_to_the_ceiling() {
        let drawn = Duration::from_millis(1_500);
        let slow = ms(&[3_000, 3_200, 2_900]);
        assert_eq!(SLOW.lengthen(drawn, &slow), Duration::from_millis(6_000));
        let stuck = ms(&[45_000, 50_000, 47_000]);
        assert_eq!(SLOW.lengthen(drawn, &stuck), Duration::from_secs(60));
    }

    #[test]
    fn a_wait_is_never_shortened() {
        let drawn = Duration::from_secs(4);
        let recent = ms(&[1_100, 1_000, 1_200, 900, 1_000]);
        assert_eq!(SLOW.floor(&recent), Some(Duration::from_millis(2_000)));
        assert_eq!(SLOW.lengthen(drawn, &recent), drawn);
    }

    #[test]
    fn the_rule_turned_off_sets_no_floor() {
        assert_eq!(SlowRule::OFF.floor(&ms(&[9_000, 9_000, 9_000])), None);
    }

    #[test]
    fn an_answer_is_healthy_only_when_it_is_a_2xx() {
        let answer = |status| AnswerTime {
            at: EpochMs::new(0),
            operation: "followers".to_string(),
            status,
            waited: Duration::ZERO,
            load: None,
            via: Via::Browser,
        };
        assert!(answer(200).healthy());
        assert!(!answer(429).healthy());
        assert!(!answer(302).healthy());
    }

    #[test]
    fn via_round_trips_through_its_name() {
        for via in [Via::Browser, Via::Direct] {
            assert_eq!(Via::parse(via.as_str()), Some(via));
        }
        assert_eq!(Via::parse("carrier pigeon"), None);
    }

    #[test]
    fn two_push_backs_on_one_account_are_no_brake() {
        let now = EpochMs::new(1_000 * MINUTE);
        let pushbacks = [pushed(1, 990, 120), pushed(1, 995, 240)];
        assert_eq!(common_brake(&pushbacks, now), None);
    }

    #[test]
    fn two_accounts_an_hour_and_a_minute_apart_are_no_brake() {
        let now = EpochMs::new(1_000 * MINUTE);
        let pushbacks = [pushed(1, 900, 240), pushed(2, 961, 240)];
        assert_eq!(common_brake(&pushbacks, now), None);
    }

    #[test]
    fn a_brake_that_has_ended_is_none() {
        let now = EpochMs::new(1_000 * MINUTE);
        let pushbacks = [pushed(1, 700, 120), pushed(2, 730, 180)];
        assert_eq!(common_brake(&pushbacks, now), None);
    }

    /// The account with the shorter cooldown waits for the longer one, and so
    /// does every account that was not pushed back on at all.
    #[test]
    fn two_accounts_within_the_hour_stop_until_the_longer_cooldown() {
        let now = EpochMs::new(1_000 * MINUTE);
        let pushbacks = [pushed(2, 990, 720), pushed(1, 960, 120)];
        assert_eq!(
            common_brake(&pushbacks, now),
            Some(Brake {
                until: EpochMs::new((990 + 720) * MINUTE),
                accounts: vec![Pk::new(1), Pk::new(2)],
            })
        );
    }

    #[test]
    fn the_window_is_the_documented_one() {
        assert_eq!(COMMON_BRAKE_WINDOW, Duration::from_secs(3600));
    }

    /// The daily ceiling on accounts, and the week it stays halved for.
    #[test]
    fn the_account_ceiling_halves_for_a_week_after_a_push_back() {
        let now = EpochMs::new(10 * 86_400_000);
        let day = 86_400_000_i64;
        assert_eq!(accounts_per_day(None, now), 2_000);
        assert_eq!(
            accounts_per_day(Some(EpochMs::new(now.get() - day)), now),
            1_000
        );
        assert_eq!(
            accounts_per_day(Some(EpochMs::new(now.get() - 7 * day + 1)), now),
            1_000
        );
        assert_eq!(
            accounts_per_day(Some(EpochMs::new(now.get() - 7 * day)), now),
            2_000
        );
    }

    const RULE: SittingRule = SittingRule {
        pages: 3,
        rest: Duration::from_secs(600),
        idle: Duration::from_secs(900),
    };

    /// Walks `pages` pages a second apart from `from`, and answers the
    /// sitting they leave.
    fn walked(before: Option<Sitting>, from: i64, pages: i64) -> Sitting {
        let mut sitting = before;
        for n in 0..pages {
            sitting = Some(Sitting::after_page(
                sitting,
                EpochMs::new(from + n * 1_000),
                RULE,
            ));
        }
        sitting.expect("at least one page")
    }

    /// The page that fills a sitting owes its rest from when it was walked,
    /// and the rest stays owed however many pages come before it ends.
    #[test]
    fn the_page_that_fills_a_sitting_owes_its_rest() {
        let two = walked(None, 0, 2);
        assert_eq!((two.pages, two.rest_until), (2, None));

        let full = walked(Some(two), 2_000, 1);
        assert_eq!(full.pages, 3);
        assert_eq!(full.rest_until, Some(EpochMs::new(2_000 + 600_000)));
        assert_eq!(
            full.rest_owed(EpochMs::new(3_000)),
            Some(EpochMs::new(602_000))
        );

        let unwaited = walked(Some(full), 3_000, 1);
        assert_eq!(unwaited.pages, 4, "a page walked anyway still counts");
        assert_eq!(
            unwaited.rest_until, full.rest_until,
            "and owes the same rest"
        );
    }

    /// Once the rest has ended, or once it is paid, the next page is the
    /// first of a new sitting.
    #[test]
    fn a_rest_ended_or_paid_starts_the_sitting_again() {
        let full = walked(None, 0, 3);
        let until = full.rest_until.expect("full");
        assert_eq!(full.rest_owed(until), None, "a rest has ended at its end");

        let after = Sitting::after_page(Some(full), until, RULE);
        assert_eq!((after.pages, after.rest_until), (1, None));

        let paid = full.rested(until);
        assert_eq!((paid.pages, paid.rest_until), (0, None));
        let after = Sitting::after_page(Some(paid), EpochMs::new(3_000), RULE);
        assert_eq!(after.pages, 1);

        let other = full.rested(EpochMs::new(until.get() + 1));
        assert_eq!(other, full, "another rest is not this one paid");
    }

    /// A sitting left alone for longer than the longest rest ends; one left
    /// alone for less, or a clock set back, counts on.
    #[test]
    fn a_sitting_left_alone_past_the_longest_rest_ends() {
        let two = walked(None, 0, 2);
        let soon = Sitting::after_page(Some(two), EpochMs::new(1_000 + 900_000), RULE);
        assert_eq!(
            soon.pages, 3,
            "fifteen minutes to the millisecond is not past"
        );

        let late = Sitting::after_page(Some(two), EpochMs::new(1_000 + 900_001), RULE);
        assert_eq!((late.pages, late.rest_until), (1, None));

        let back = Sitting::after_page(Some(two), EpochMs::new(-50_000), RULE);
        assert_eq!(back.pages, 3, "a clock set back rests sooner, not later");

        assert_eq!(two.pages_at(EpochMs::new(2_000), RULE.idle), 2);
        assert_eq!(two.pages_at(EpochMs::new(1_000 + 900_001), RULE.idle), 0);
    }

    /// A sitting of zero pages is a sitting of one, rather than a rest owed
    /// before anything was walked or never.
    #[test]
    fn a_sitting_of_no_pages_is_one_of_one() {
        let rule = SittingRule { pages: 0, ..RULE };
        let first = Sitting::after_page(None, EpochMs::new(0), rule);
        assert_eq!(first.rest_until, Some(EpochMs::new(600_000)));
    }

    /// The three cooldowns, written out rather than derived.
    ///
    /// `AGENTS.md` promises that an action block earns twelve hours rather than
    /// the throttle's two. Everything else refers to the constants by name, so
    /// without this both could become one second with the suite green.
    ///
    /// It sits beside the constants rather than in a crate that only reads
    /// them.
    #[test]
    fn the_cooldowns_are_the_documented_ones() {
        assert_eq!(RATE_LIMIT_COOLDOWN, Duration::from_secs(2 * 3600));
        assert_eq!(ACTION_BLOCK_COOLDOWN, Duration::from_secs(12 * 3600));
        assert_eq!(CHALLENGE_COOLDOWN, Duration::from_secs(30 * 60));
        assert!(
            ACTION_BLOCK_COOLDOWN > RATE_LIMIT_COOLDOWN,
            "an action block is not a throttle and must not be treated as the lighter one"
        );
    }
}
