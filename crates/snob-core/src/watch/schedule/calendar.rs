//! The set of minutes a run is allowed to happen in: what both syntaxes are
//! read into, and what the schedule asks of it (whether a moment is allowed,
//! how close two allowed moments can be, how much of a day is left after the
//! last).

use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Timelike};

use super::parse::FieldSet;

const A_DAY_SECS: i64 = 24 * 3_600;

/// Which minutes a run is allowed to happen in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Calendar {
    pub(super) times: Times,
    pub(super) days_of_month: FieldSet,
    pub(super) months: FieldSet,
    /// Sunday is 0, the way cron numbers them.
    pub(super) days_of_week: FieldSet,
}

/// Which times of day a calendar allows.
///
/// **Two readings, and they differ**: cron's hour and minute fields are
/// crossed (`0 9,21 * * *`), while `--at 09:00,21:30` names two moments, not
/// the four their product would give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Times {
    /// The `(hour, minute)` pairs somebody named.
    Exact(Vec<(u32, u32)>),
    /// The two fields of a cron expression, to be crossed.
    Crossed { minutes: FieldSet, hours: FieldSet },
}

impl Times {
    fn allows(&self, hour: u32, minute: u32) -> bool {
        match self {
            Self::Exact(times) => times.contains(&(hour, minute)),
            Self::Crossed { minutes, hours } => hours.contains(hour) && minutes.contains(minute),
        }
    }

    /// Every allowed moment of the day, as seconds since midnight.
    fn moments(&self) -> Vec<i64> {
        let seconds = |hour: u32, minute: u32| i64::from(hour) * 3_600 + i64::from(minute) * 60;
        match self {
            Self::Exact(times) => times.iter().map(|&(h, m)| seconds(h, m)).collect(),
            Self::Crossed { minutes, hours } => (0..24u32)
                .filter(|&hour| hours.contains(hour))
                .flat_map(|hour| {
                    (0..60u32)
                        .filter(|&minute| minutes.contains(minute))
                        .map(move |minute| seconds(hour, minute))
                })
                .collect(),
        }
    }
}

impl Calendar {
    /// Whether this moment is one the calendar allows.
    pub(super) fn allows<Tz: TimeZone>(&self, at: &DateTime<Tz>) -> bool {
        self.times.allows(at.hour(), at.minute()) && self.allows_day(at)
    }

    /// [`allows`](Self::allows), for a reading of the wall clock that may be
    /// one the clocks jumped over, which no moment reads.
    pub(super) fn allows_local(&self, local: &NaiveDateTime) -> bool {
        self.times.allows(local.hour(), local.minute()) && self.allows_day(local)
    }

    /// Whether this falls on a day the calendar names, whatever the time.
    ///
    /// **The day rule is cron's**: when both day fields are restricted, a day
    /// matching **either** is allowed (`0 9 13 * 5` is the 13th and every
    /// Friday, not Friday the 13th); when one is `*`, the other decides.
    pub(super) fn allows_day(&self, at: &impl Datelike) -> bool {
        if !self.months.contains(at.month()) {
            return false;
        }

        let dom = self.days_of_month.contains(at.day());
        let dow = self
            .days_of_week
            .contains(at.weekday().num_days_from_sunday());

        match (self.dom_restricted(), self.dow_restricted()) {
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }

    fn dom_restricted(&self) -> bool {
        !self.days_of_month.is_all(1..=31)
    }

    fn dow_restricted(&self) -> bool {
        !self.days_of_week.is_all(0..=6)
    }

    /// The shortest gap between two moments this calendar allows, in seconds;
    /// `None` when it allows one minute a day.
    ///
    /// Only the minutes and hours are looked at: the day fields can only make
    /// a gap longer.
    pub(super) fn tightest_gap(&self) -> Option<i64> {
        let mut allowed = self.times.moments();
        allowed.sort_unstable();
        allowed.dedup();

        let (first, last) = (*allowed.first()?, *allowed.last()?);
        if allowed.len() < 2 {
            return None;
        }
        // 23:59 and 00:00 are a minute apart, but only when a named day can
        // follow another: `--on mon --at 00:00,23:55` has a week between them.
        let wrap = self
            .days_can_be_consecutive()
            .then(|| A_DAY_SECS - last + first);
        allowed
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .chain(wrap)
            .min()
    }

    /// Whether the day or month fields name a subset of the days. The jitter
    /// ceiling asks: a run pushed past midnight is harmless only when the
    /// next day is one the calendar allows too.
    pub(super) fn days_are_restricted(&self) -> bool {
        self.dom_restricted() || self.dow_restricted() || !self.months.is_all(1..=12)
    }

    /// How much of the day is left after its last moment, in seconds.
    pub(super) fn room_before_midnight(&self) -> i64 {
        let latest = self.times.moments().into_iter().max().unwrap_or(0);
        A_DAY_SECS - latest
    }

    /// Whether some day this calendar allows can be immediately followed by
    /// another.
    fn days_can_be_consecutive(&self) -> bool {
        match (self.dom_restricted(), self.dow_restricted()) {
            (false, false) => true,
            // The union of both (`allows_day`), taken as possible and not
            // worked out: the answer feeds a refusal and a jitter ceiling,
            // and both have to err towards the tighter number.
            (true, true) => true,
            (true, false) => {
                (1..31).any(|d| self.days_of_month.contains(d) && self.days_of_month.contains(d + 1))
                    // A month's last day, whichever it is, followed by the
                    // first of the next.
                    || (self.days_of_month.contains(1)
                        && (28..=31).any(|d| self.days_of_month.contains(d)))
            }
            (false, true) => (0..7)
                .any(|d| self.days_of_week.contains(d) && self.days_of_week.contains((d + 1) % 7)),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::super::parse::parse_cron;
    use super::super::tests::{at, hours};
    use super::super::{Schedule, ScheduleError, Weekday};
    use crate::Epoch;

    /// The midnight wrap is only a gap when there is a next day. Counted
    /// unconditionally, `--on mon --at 00:00,23:55` would be refused as "5m is
    /// too often" about two runs 23h55m apart.
    #[test]
    fn the_wrap_is_not_a_gap_when_the_days_are_not_consecutive() {
        assert!(
            Schedule::calendar(&[Weekday::Mon], &[(0, 0), (23, 55)]).is_ok(),
            "a whole week apart is not too often"
        );
        assert!(
            Schedule::cron("0,55 0,23 * * 1").is_ok(),
            "the same schedule written as cron"
        );

        // Consecutive days, and it is a gap again: Monday 23:55 to Tuesday
        // 00:00 is five minutes.
        assert!(matches!(
            Schedule::calendar(&[Weekday::Mon, Weekday::Tue], &[(0, 0), (23, 55)]),
            Err(ScheduleError::TooOften { .. })
        ));
        assert!(matches!(
            Schedule::calendar(&[], &[(0, 0), (23, 55)]),
            Err(ScheduleError::TooOften { .. })
        ));
    }

    /// The gap across midnight is a gap. `23:55` and `00:00` are five minutes
    /// apart, and measuring only the forward differences within a day would
    /// call them nearly twenty-four hours and let them through.
    #[test]
    fn the_wrap_around_midnight_counts_as_a_gap() {
        assert!(Schedule::calendar(&[], &[(23, 55), (0, 0)]).is_err());
    }

    /// The rule every reimplementation gets wrong. `0 9 13 * 5` is "the 13th,
    /// and every Friday" -- not "Friday the 13th".
    #[test]
    fn two_restricted_day_fields_combine_with_or() {
        let calendar = parse_cron("0 9 13 * 5").unwrap();
        let day = |at: Epoch| calendar.allows(&Utc.timestamp_opt(at.get(), 0).unwrap());
        let date = |at: Epoch| calendar.allows_day(&Utc.timestamp_opt(at.get(), 0).unwrap());

        // Friday 2026-08-21 at 09:00 -- a Friday that is not the 13th.
        assert!(day(at(4 * hours(24) + hours(9))));
        // Monday 2026-08-17 at 09:00 -- neither.
        assert!(!day(at(hours(9))));

        // The day half on its own says the same about the days, at any time.
        assert!(date(at(4 * hours(24) + hours(15))));
        assert!(!date(at(hours(15))));
    }

    /// And when one of them is `*`, only the other decides. Read as OR, `*`
    /// would match every day and the weekday field would do nothing.
    #[test]
    fn one_unrestricted_day_field_leaves_the_other_in_charge() {
        let calendar = parse_cron("0 9 * * 1").unwrap();
        let day = |at: Epoch| calendar.allows(&Utc.timestamp_opt(at.get(), 0).unwrap());
        let date = |at: Epoch| calendar.allows_day(&Utc.timestamp_opt(at.get(), 0).unwrap());

        assert!(day(at(hours(9))), "Monday");
        assert!(!day(at(hours(24) + hours(9))), "Tuesday");
        assert!(date(at(hours(15))), "Monday, whatever the hour");
        assert!(
            !date(at(hours(24) + hours(15))),
            "Tuesday, whatever the hour"
        );
    }
}
