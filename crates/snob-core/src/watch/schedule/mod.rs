//! When the monitor runs.
//!
//! `--every` is a floor measured from the previous run; `--on`/`--at` and
//! `--cron` both build a `Calendar`, the set of minutes a run may happen in.
//! Given both, the next run is the first moment that satisfies both
//! (`--every 2w --on mon --at 09:00`).
//!
//! **One evaluator, two syntaxes**: a cron expression is parsed into the same
//! `Calendar` that `--on mon --at 09:00` produces, which is why there is a
//! parser here and not a dependency. Nothing here reads a clock.
//!
//! This file holds the [`Schedule`] and what makes one legal; `parse` reads
//! the two syntaxes, `calendar` is what they are read into, and `next` is the
//! evaluator.

mod calendar;
mod next;
mod parse;
mod spec;

use std::time::Duration;

use chrono::TimeZone;

use crate::Epoch;

use calendar::{Calendar, Times};
use next::next_local_midnight;
use parse::{FieldSet, parse_cron};

pub use next::{due, due_asked_since, due_moment, next_after, wake_at};
pub use parse::{Weekday, parse_time};
pub use spec::{Spec, calendar_from};

/// The shortest gap between two runs.
///
/// Fifteen minutes of an account's life is the drift this tool already treats
/// as one instant (`SAME_MOMENT_GAP_SECS` in the CLI, the same length and
/// deliberately another constant): a second run closer than that spends
/// requests to be told what the first was told.
pub const MIN_GAP_SECS: i64 = 15 * 60;

/// The longest interval `--every` accepts: a year, past which a calendar says
/// it better. The bound is also what keeps an absurd interval from a
/// hand-edited `watch.toml` out of the arithmetic.
pub const MAX_INTERVAL_SECS: i64 = 366 * 24 * 3_600;

/// How far past its due moment a run may be pushed, so that every unattended
/// copy of this tool does not wake on the same round minute: fifteen minutes,
/// or a tenth of the interval when that is smaller, and at least one minute.
fn default_jitter(every: Option<Duration>) -> Duration {
    let ceiling = Duration::from_secs(15 * 60);
    match every {
        Some(every) => (every / 10).clamp(Duration::from_secs(60), ceiling),
        None => ceiling,
    }
}

/// When the monitor runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    /// Minutes the run may happen in. `None` allows any minute.
    calendar: Option<Calendar>,
    /// Never sooner than this after the previous run. `None` sets no floor.
    every: Option<Duration>,
    jitter: Duration,
}

/// What is wrong with a schedule, said so somebody can fix it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    #[error("a schedule needs an interval or a calendar: try --every 6h, or --on mon --at 09:00")]
    Empty,
    #[error(
        "{interval} is too often. The monitor does not run twice inside {minutes} minutes: a \
         second run that close cannot see anything the first did not, and it spends requests to \
         find that out."
    )]
    TooOften { interval: String, minutes: i64 },
    #[error(
        "{interval} is longer than this can schedule. The longest interval is {days} days; past \
         that, use a calendar."
    )]
    TooRare { interval: String, days: i64 },
    #[error("{0}")]
    Unreadable(String),
}

impl Schedule {
    /// Whether the moments this fires at are named, not measured. A fresh
    /// install asks: an interval with no past is due at once unless a last
    /// run is invented, and on a calendar an invented one would step over
    /// any moment in the next quarter of an hour.
    pub fn is_on_a_calendar(&self) -> bool {
        self.calendar.is_some()
    }

    /// Every so often, counted from the previous run.
    pub fn every(interval: Duration) -> Result<Self, ScheduleError> {
        Self {
            calendar: None,
            every: Some(interval),
            jitter: default_jitter(Some(interval)),
        }
        .validated()
    }

    /// On these weekdays, at these times of day. Empty days means every day.
    pub fn calendar(days: &[Weekday], times: &[(u32, u32)]) -> Result<Self, ScheduleError> {
        if times.is_empty() {
            return Err(ScheduleError::Unreadable(
                "a calendar needs a time of day: try --at 09:00".to_string(),
            ));
        }

        for &(hour, minute) in times {
            if hour > 23 || minute > 59 {
                return Err(ScheduleError::Unreadable(format!(
                    "{hour:02}:{minute:02} is not a time of day"
                )));
            }
        }

        let days_of_week = if days.is_empty() {
            FieldSet::all(0..=6)
        } else {
            FieldSet::weekdays(days)
        };

        let mut exact: Vec<(u32, u32)> = times.to_vec();
        exact.sort_unstable();
        exact.dedup();

        Self {
            calendar: Some(Calendar {
                times: Times::Exact(exact),
                days_of_month: FieldSet::all(1..=31),
                months: FieldSet::all(1..=12),
                days_of_week,
            }),
            every: None,
            jitter: default_jitter(None),
        }
        .fitted()
        .validated()
    }

    /// On these weekdays, as often as the interval allows: the shape
    /// `--every 2w --on mon` asks for, which cron cannot express. Every
    /// minute of an allowed day is allowed, and the interval keeps two runs
    /// apart; the jitter comes out zero ([`Schedule::room_for_jitter`]).
    ///
    /// **Empty days are refused**, not read as every day: that would be
    /// [`Schedule::every`] answering `true` to `is_on_a_calendar`.
    pub fn days(days: &[Weekday], interval: Duration) -> Result<Self, ScheduleError> {
        if days.is_empty() {
            return Err(ScheduleError::Unreadable(
                "which days? try --on mon,thu, or --every 6h for no particular day".to_string(),
            ));
        }

        Self {
            calendar: Some(Calendar {
                times: Times::Crossed {
                    minutes: FieldSet::all(0..=59),
                    hours: FieldSet::all(0..=23),
                },
                days_of_month: FieldSet::all(1..=31),
                months: FieldSet::all(1..=12),
                days_of_week: FieldSet::weekdays(days),
            }),
            every: Some(interval),
            jitter: default_jitter(Some(interval)),
        }
        .fitted()
        .validated()
    }

    /// A five-field cron expression.
    pub fn cron(expression: &str) -> Result<Self, ScheduleError> {
        Self {
            calendar: Some(parse_cron(expression)?),
            every: None,
            jitter: default_jitter(None),
        }
        .fitted()
        .validated()
    }

    /// Brings the jitter down to what this schedule can absorb, so the
    /// default is bounded as an explicit `--jitter` is.
    fn fitted(self) -> Self {
        let jitter = self.jitter;
        self.with_jitter(jitter)
    }

    /// Adds a floor to a calendar, or a calendar to a floor: the next run is
    /// the first moment that satisfies both.
    pub fn and_every(mut self, interval: Duration) -> Result<Self, ScheduleError> {
        self.every = Some(interval);
        if self.calendar.is_none() {
            self.jitter = default_jitter(Some(interval));
        }
        // The interval changes what separates two runs, so the room between
        // them is measured again.
        self.fitted().validated()
    }

    /// Sets how far a run may be pushed past its due moment, bounded to what
    /// the schedule can absorb ([`Schedule::room_for_jitter`]), by both
    /// halves when both are set: `--every 2w --on mon --at 09:00 --jitter 5d`
    /// must not wake on a Friday.
    #[must_use]
    pub fn with_jitter(mut self, jitter: Duration) -> Self {
        self.jitter = jitter.min(self.room_for_jitter());
        self
    }

    /// The most a run may be pushed later without costing the next one.
    ///
    /// **The gap between two moments is not the room inside it.** The floor
    /// is measured from where a run really landed, so a run at moment `m`
    /// pushed to `m + s` still reaches `m + gap` only if `s <= gap - step`.
    /// Without the step taken off, `--cron "*/15 * * * *"` under the default
    /// jitter would run every thirty minutes.
    fn room_for_jitter(&self) -> Duration {
        const A_DAY: i64 = 24 * 3_600;

        let step = self.step_secs();

        match &self.calendar {
            Some(calendar) => {
                // One moment a day has the next at least a day away.
                let gap = calendar.tightest_gap().unwrap_or(A_DAY);
                let mut room = gap.saturating_sub(step).max(0);
                // **And not past midnight, when the days are restricted**:
                // `--on mon --at 09:00 --jitter 20h` must not run on Tuesday.
                // The last moment of a day has that midnight in front of it
                // whatever the gap behind it.
                if calendar.days_are_restricted() {
                    room = room.min(calendar.room_before_midnight());
                }
                Duration::from_secs(room.unsigned_abs())
            }
            // No grid to miss, so the interval is the only bound. (The
            // fallback is unreachable: `validated` refuses neither half.)
            None => self.every.unwrap_or(Duration::from_secs(A_DAY as u64)),
        }
    }

    /// The room the moment `due_at` really has, in real seconds.
    ///
    /// [`Schedule::room_for_jitter`] counts in seconds of a 24-hour day, an
    /// hour too generous on the day a zone springs forward through:
    /// `--at 09:00 --jitter 24h` could wake a run past the next day's moment.
    /// This asks the calendar in the zone: the next moment the schedule
    /// names, less the floor between two runs, and when the days are
    /// restricted no further than the local midnight after `due_at`. An
    /// interval alone has no grid to miss, and is unbounded here.
    fn room_at<Tz: TimeZone>(&self, due_at: Epoch, zone: &Tz) -> Duration {
        let Some(calendar) = &self.calendar else {
            return Duration::MAX;
        };

        // A calendar that names nothing after this moment has nothing to lose.
        let mut room = match next_after(self, Some(due_at), due_at, zone) {
            Some(next) => (next - due_at).saturating_sub(self.step_secs()).max(0),
            None => i64::MAX,
        };

        if calendar.days_are_restricted()
            && let Some(midnight) = next_local_midnight(due_at, zone)
        {
            room = room.min((midnight - due_at).max(0));
        }
        Duration::from_secs(room.unsigned_abs())
    }

    pub fn jitter(&self) -> Duration {
        self.jitter
    }

    /// What separates two runs, in seconds: the interval when there is one,
    /// and never less than [`MIN_GAP_SECS`].
    fn step_secs(&self) -> i64 {
        self.every
            .map(|every| i64::try_from(every.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0)
            .max(MIN_GAP_SECS)
    }

    /// [`Schedule::step_secs`], as a length of time.
    fn step(&self) -> Duration {
        Duration::from_secs(self.step_secs().unsigned_abs())
    }

    /// Refuses a schedule that would run more often than the floor allows, or
    /// names nothing. **Both halves are checked**, and every constructor
    /// calls this: `--cron "*/5 * * * *"` and `--at 09:00,09:05` run as often
    /// as `--every 5m`.
    fn validated(self) -> Result<Self, ScheduleError> {
        if self.calendar.is_none() && self.every.is_none() {
            return Err(ScheduleError::Empty);
        }
        if let Some(every) = self.every {
            // Compared as `u64`: a cast to `i64` wraps, and
            // `--every 18446744073709551615` would be refused as too often.
            if every.as_secs() < MIN_GAP_SECS as u64 {
                return Err(ScheduleError::TooOften {
                    interval: crate::duration::format(every),
                    minutes: MIN_GAP_SECS / 60,
                });
            }
            if every.as_secs() > MAX_INTERVAL_SECS as u64 {
                return Err(ScheduleError::TooRare {
                    interval: crate::duration::format(every),
                    days: MAX_INTERVAL_SECS / (24 * 3_600),
                });
            }
        }

        // A calendar with an interval on it is bounded by the interval, just
        // checked; alone, by its own tightest gap.
        if self.every.is_none()
            && let Some(calendar) = &self.calendar
            && let Some(gap) = calendar.tightest_gap()
            && gap < MIN_GAP_SECS
        {
            return Err(ScheduleError::TooOften {
                interval: crate::duration::format(Duration::from_secs(gap.unsigned_abs())),
                minutes: MIN_GAP_SECS / 60,
            });
        }
        Ok(self)
    }
}

/// Whether a run is owed, and when the next one is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Nothing yet. Sleep until this instant.
    At(Epoch),
    /// Run now. `missed` counts the scheduled runs being folded into this one.
    Now { missed: u32 },
    /// The schedule names no moment at all (`0 0 31 2 *`), as [`next_after`]
    /// answers `None`. **A variant, not a sentinel instant**, which a caller
    /// would sleep towards for ever.
    Never,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Datelike, Timelike, Utc};

    /// Midnight UTC on Monday 2026-08-17. Every timestamp below is an offset
    /// from it, so the weekday arithmetic is checkable by hand.
    pub(super) const MONDAY_0000: i64 = 1_786_924_800;

    /// The constant has to be the day it claims to be.
    #[test]
    fn the_fixture_really_is_a_monday() {
        let day = Utc.timestamp_opt(MONDAY_0000, 0).unwrap();
        assert_eq!(day.weekday(), chrono::Weekday::Mon);
        assert_eq!((day.hour(), day.minute()), (0, 0));
    }

    /// A moment, as an offset from the fixture Monday. The offset is a length
    /// of time and stays a number; what comes back is a moment and says so.
    pub(super) fn at(offset_secs: i64) -> Epoch {
        Epoch::new(MONDAY_0000 + offset_secs)
    }

    pub(super) fn hours(n: i64) -> i64 {
        n * 3_600
    }

    /// A length of time, for the offsets these tests add to a moment. Written
    /// out rather than left as a number, because a moment takes a length of
    /// time and not a count -- which is the whole of what [`Epoch`] is for.
    pub(super) fn secs(n: i64) -> Duration {
        Duration::from_secs(n.max(0) as u64)
    }

    /// A jitter larger than the room it is jittering within is not a jitter.
    ///
    /// The room is the gap **less the floor between two runs**, not the gap.
    /// The floor is measured from where a run really landed, so every second of
    /// jitter comes out of the next gap — which is why the two calendar
    /// expectations here are not round numbers.
    #[test]
    fn jitter_is_bounded_by_what_the_schedule_can_absorb() {
        let every = Duration::from_secs(hours(6) as u64);
        let schedule = Schedule::every(every)
            .unwrap()
            .with_jitter(Duration::from_secs(30 * 24 * 3_600));
        assert_eq!(
            schedule.jitter(),
            every,
            "an interval has no grid to miss: what moves is the whole schedule"
        );

        let calendar = Schedule::calendar(&[], &[(9, 0), (21, 0)])
            .unwrap()
            .with_jitter(Duration::from_secs(30 * 24 * 3_600));
        assert_eq!(
            calendar.jitter(),
            Duration::from_secs((hours(12) - MIN_GAP_SECS) as u64),
            "a full twelve hours would put the nine o'clock run inside the floor \
             of the nine in the evening"
        );

        // With both halves set, what separates two runs is the interval, and
        // here it is longer than the grid: a Monday in every two, on a weekly
        // grid, has no room at all. A day of jitter on a fortnightly Monday
        // would put the next run past the Monday it was due on, so the
        // fortnight would quietly become three weeks.
        let both = Schedule::calendar(&[Weekday::Mon], &[(9, 0)])
            .unwrap()
            .and_every(Duration::from_secs(14 * 24 * 3_600))
            .unwrap()
            .with_jitter(Duration::from_secs(5 * 24 * 3_600));
        assert_eq!(both.jitter(), Duration::ZERO);
    }

    /// Jitter cannot move a run onto a day the calendar forbids.
    ///
    /// With one moment a day `tightest_gap` is `None` and the room falls back
    /// to a whole day, however narrow the day fields are — so
    /// `--on mon --at 09:00 --jitter 20h` must not keep all twenty hours and
    /// walk a run due Monday morning on Tuesday at three. That is the failure
    /// `with_jitter`'s own doc names as the reason the bound exists, arriving
    /// through the one-moment-a-day branch.
    #[test]
    fn jitter_cannot_move_a_run_onto_a_day_the_calendar_forbids() {
        let schedule = Schedule::calendar(&[Weekday::Mon], &[(9, 0)])
            .unwrap()
            .with_jitter(Duration::from_secs(20 * 3_600));

        let due_at = at(hours(9));
        let woken = next::jittered(due_at, schedule.jitter(), 0.999_999);

        let landed = DateTime::from_timestamp(woken.get(), 0).unwrap();
        assert_eq!(
            landed.weekday(),
            chrono::Weekday::Mon,
            "due Monday at nine, woke {landed} — a day this schedule does not name"
        );

        // Every day allowed is the other case, and it needs no cap: the day a
        // run spills into is one the calendar names.
        let daily = Schedule::calendar(&[], &[(9, 0)])
            .unwrap()
            .with_jitter(Duration::from_secs(20 * 3_600));
        assert_eq!(
            daily.jitter(),
            Duration::from_secs(20 * 3_600),
            "nothing to protect when tomorrow is allowed too"
        );
    }

    /// An interval nothing could serve is refused, and says which way it is
    /// wrong.
    ///
    /// `every = "9223372036854775807"` can arrive from a hand-edited
    /// `watch.toml`; `MAX_INTERVAL_SECS` says why it has to be refused, and it
    /// is refused as too long rather than as too often.
    #[test]
    fn an_interval_too_long_to_add_up_is_refused_as_too_long() {
        for seconds in [u64::MAX, i64::MAX as u64, MAX_INTERVAL_SECS as u64 + 1] {
            let refused = Schedule::every(Duration::from_secs(seconds));
            assert!(
                matches!(refused, Err(ScheduleError::TooRare { .. })),
                "{seconds}s: {refused:?}"
            );
        }

        // And the largest one that is allowed still answers without overflowing.
        let schedule = Schedule::every(Duration::from_secs(MAX_INTERVAL_SECS as u64)).unwrap();
        assert_eq!(
            due(&schedule, Some(at(0)), at(1), &Utc),
            Due::At(at(MAX_INTERVAL_SECS))
        );
    }

    /// The floor applies to every way of asking, not only to `--every`.
    ///
    /// `--cron "*/5 * * * *"` and `--at 09:00,09:05` run just as often as
    /// `--every 5m`, and the error message states the rule for all three.
    #[test]
    fn nothing_gets_past_the_minimum_gap() {
        for expression in ["* * * * *", "*/5 * * * *", "0,10 * * * *"] {
            assert!(
                Schedule::cron(expression).is_err(),
                "\"{expression}\" runs more often than the floor allows"
            );
        }
        assert!(Schedule::calendar(&[], &[(9, 0), (9, 5)]).is_err());
        assert!(Schedule::every(Duration::from_secs(300)).is_err());
    }

    /// And what is spaced widely enough still goes through, including the
    /// shapes people actually write.
    #[test]
    fn an_ordinary_schedule_is_not_caught_by_the_floor() {
        for expression in ["0 9 * * *", "0 */6 * * *", "0 9 * * 1,4", "0,30 * * * *"] {
            assert!(
                Schedule::cron(expression).is_ok(),
                "\"{expression}\" should be allowed"
            );
        }
        assert!(Schedule::calendar(&[], &[(9, 0), (21, 30)]).is_ok());
        assert!(Schedule::every(Duration::from_secs(MIN_GAP_SECS as u64)).is_ok());
    }

    #[test]
    fn the_default_jitter_scales_with_the_interval_and_is_capped() {
        assert_eq!(
            default_jitter(Some(Duration::from_secs(hours(6) as u64))),
            Duration::from_secs(900),
            "a tenth of six hours is over the cap, so it is the cap"
        );
        assert_eq!(
            default_jitter(Some(Duration::from_secs(hours(1) as u64))),
            Duration::from_secs(360)
        );
        assert_eq!(
            default_jitter(Some(Duration::from_secs(MIN_GAP_SECS as u64))),
            Duration::from_secs(90)
        );
    }

    /// An interval below the floor is refused when it is set, not silently
    /// stretched later -- and the refusal says what it costs.
    #[test]
    fn an_interval_below_the_minimum_gap_is_refused_with_a_reason() {
        let error = Schedule::every(Duration::from_secs(300)).unwrap_err();
        assert!(matches!(error, ScheduleError::TooOften { .. }));
        assert!(error.to_string().contains("15 minutes"), "{error}");
    }

    #[test]
    fn a_schedule_with_neither_half_is_refused() {
        assert_eq!(
            Schedule::calendar(&[Weekday::Mon], &[]).unwrap_err(),
            ScheduleError::Unreadable("a calendar needs a time of day: try --at 09:00".to_string())
        );
    }
}
