//! A schedule as it is written, before it is built: `--every`, `--on`/`--at`,
//! `--days`/`--months`, `--cron` and the jitter, in the words they were given
//! in. What the flags,
//! `watch.toml` and an automation's trigger all hold, and **the one place they
//! are built into a [`Schedule`]**.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Days, MonthDay, Schedule, ScheduleError, Weekday, parse_month, parse_time};

/// The schedule as written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    #[serde(default, skip_serializing_if = "Option::is_none", with = "text")]
    pub every: Option<Duration>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<String>,
    /// Months, `jan`; none is every month.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub months: Vec<String>,
    /// Days of the month: dates, `15`, or weekdays of the month, `first mon`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub days: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "text")]
    pub jitter: Option<Duration>,
}

impl Spec {
    /// Whether it names no schedule at all: no `every`, `on`, `at`, `months`,
    /// `days` or `cron`.
    pub fn names_none(&self) -> bool {
        self.every.is_none()
            && self.on.is_empty()
            && self.at.is_empty()
            && self.months.is_empty()
            && self.days.is_empty()
            && self.cron.is_none()
    }

    /// Whether it names any part of a calendar but cron: `on`, `at`,
    /// `months` or `days`.
    fn names_a_calendar(&self) -> bool {
        !(self.on.is_empty()
            && self.at.is_empty()
            && self.months.is_empty()
            && self.days.is_empty())
    }

    /// The schedule it says, or why it says none. A cron expression beside
    /// days, months or times is refused: only the cron would be used. So are
    /// days of the week beside days of the month.
    pub fn build(&self) -> Result<Schedule, ScheduleError> {
        if self.cron.is_some() && self.names_a_calendar() {
            return Err(ScheduleError::Unreadable(
                "\"cron\" and days, months or times are two ways of saying the same thing; keep one"
                    .to_string(),
            ));
        }
        if !self.on.is_empty() && !self.days.is_empty() {
            return Err(ScheduleError::Unreadable(
                "days of the week and days of the month are two ways of choosing days; keep one"
                    .to_string(),
            ));
        }
        let mut schedule = if let Some(expression) = &self.cron {
            Schedule::cron(expression)?
        } else if self.at.is_empty()
            && !self.on.is_empty()
            && self.months.is_empty()
            && self.days.is_empty()
            && let Some(every) = self.every
        {
            // Days and an interval and no time of day: before the calendar,
            // which would refuse days alone for naming no minute.
            Schedule::days(&days_named(&self.on)?, every)?
        } else if self.names_a_calendar() {
            self.calendar_of()?
        } else if let Some(every) = self.every {
            Schedule::every(every)?
        } else {
            return Err(ScheduleError::Empty);
        };
        // An interval beside a calendar is a floor on it, not a second
        // schedule.
        if let Some(every) = self.every
            && (self.cron.is_some() || !self.at.is_empty())
        {
            schedule = schedule.and_every(every)?;
        }
        if let Some(jitter) = self.jitter {
            schedule = schedule.with_jitter(jitter);
        }
        Ok(schedule)
    }

    /// The calendar `on`, `days`, `months` and `at` name, parsed and checked
    /// together.
    fn calendar_of(&self) -> Result<Schedule, ScheduleError> {
        let weekdays = days_named(&self.on)?;
        let of_month = self
            .days
            .iter()
            .map(|day| {
                MonthDay::parse(day).ok_or_else(|| {
                    ScheduleError::Unreadable(format!(
                        "\"{day}\" is not a day of the month (try 1, 15 or first mon)"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let months = self
            .months
            .iter()
            .map(|month| {
                parse_month(month).ok_or_else(|| {
                    ScheduleError::Unreadable(format!("\"{month}\" is not a month (try jan, jul)"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Schedule::on_days(
            Days {
                weekdays: &weekdays,
                of_month: &of_month,
                months: &months,
            },
            &times_named(&self.at)?,
        )
    }
}

/// The two halves of a calendar, parsed and checked together.
pub fn calendar_from<S: AsRef<str>>(days: &[S], times: &[S]) -> Result<Schedule, ScheduleError> {
    Schedule::calendar(&days_named(days)?, &times_named(times)?)
}

fn times_named<S: AsRef<str>>(times: &[S]) -> Result<Vec<(u32, u32)>, ScheduleError> {
    times.iter().map(|time| parse_time(time.as_ref())).collect()
}

fn days_named<S: AsRef<str>>(days: &[S]) -> Result<Vec<Weekday>, ScheduleError> {
    days.iter()
        .map(|day| {
            let day = day.as_ref();
            Weekday::parse(day).ok_or_else(|| {
                ScheduleError::Unreadable(format!("\"{day}\" is not a day (try mon, thu)"))
            })
        })
        .collect()
}

/// A duration kept as a person writes it (`6h`), as `watch.toml` keeps one.
mod text {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<Duration>, to: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(duration) => to.serialize_str(&crate::duration::format(*duration)),
            None => to.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Option<Duration>, D::Error> {
        use serde::de::Error;
        Option::<String>::deserialize(from)?
            .map(|text| crate::duration::parse(&text).map_err(D::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_builds_the_schedule_each_syntax_says() {
        let every = Spec {
            every: Some(Duration::from_secs(6 * 3600)),
            ..Spec::default()
        };
        assert!(every.build().is_ok());
        let calendar = Spec {
            on: vec!["mon".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        assert!(calendar.build().unwrap().is_on_a_calendar());
        let days = Spec {
            on: vec!["mon".into()],
            every: Some(Duration::from_secs(14 * 86_400)),
            ..Spec::default()
        };
        assert!(days.build().is_ok(), "days and an interval, no time");
        assert_eq!(Spec::default().build(), Err(ScheduleError::Empty));
        assert!(Spec::default().names_none());
        let both = Spec {
            cron: Some("0 9 * * 1".into()),
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        assert!(both.build().is_err(), "two syntaxes at once");
        let unknown = Spec {
            on: vec!["someday".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        assert!(unknown.build().is_err());

        let month_days = Spec {
            days: vec!["1".into(), "first mon".into()],
            months: vec!["jan".into(), "jul".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        assert!(month_days.build().unwrap().is_on_a_calendar());
        assert!(!month_days.names_none());
        let months_alone = Spec {
            months: vec!["dec".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        assert!(months_alone.build().is_ok(), "every day of December");
        let cron_and_months = Spec {
            cron: Some("0 9 * * 1".into()),
            months: vec!["jan".into()],
            ..Spec::default()
        };
        assert!(cron_and_months.build().is_err(), "two syntaxes at once");
        // Days and an interval run without a time of day; a month among
        // them makes it a calendar, which needs one.
        let days_in_a_month = Spec {
            on: vec!["mon".into()],
            every: Some(Duration::from_secs(7 * 86_400)),
            months: vec!["jan".into()],
            ..Spec::default()
        };
        let error = days_in_a_month.build().unwrap_err().to_string();
        assert!(error.contains("months need a time of day"), "{error}");
        for (days, months, why) in [
            (vec!["32"], vec![], "not a day of the month"),
            (vec!["fifth mon"], vec![], "not a day of the month"),
            (vec!["1"], vec!["smarch"], "not a month"),
        ] {
            let wrong = Spec {
                days: days.into_iter().map(String::from).collect(),
                months: months.into_iter().map(String::from).collect(),
                at: vec!["09:00".into()],
                ..Spec::default()
            };
            let error = wrong.build().unwrap_err().to_string();
            assert!(error.contains(why), "{error}");
        }
    }

    #[test]
    fn days_of_the_week_beside_days_of_the_month_are_refused() {
        let both = Spec {
            on: vec!["mon".into()],
            days: vec!["15".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        let error = both.build().unwrap_err().to_string();
        assert!(error.contains("two ways of choosing days"), "{error}");
        let both = Schedule::on_days(
            Days {
                weekdays: &[Weekday::Mon],
                of_month: &[MonthDay::Date(15)],
                months: &[],
            },
            &[(9, 0)],
        );
        assert!(both.is_err(), "refused when built directly too");
    }

    /// Fields left empty are left out, so a schedule written before months
    /// and days existed is written exactly as it was, and read back.
    #[test]
    fn months_and_days_are_written_only_when_named() {
        let spec = Spec {
            on: vec!["mon".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        let text = serde_json::to_string(&spec).unwrap();
        assert_eq!(text, r#"{"on":["mon"],"at":["09:00"]}"#);
        let named = Spec {
            days: vec!["first mon".into()],
            months: vec!["jan".into()],
            at: vec!["09:00".into()],
            ..Spec::default()
        };
        let text = serde_json::to_string(&named).unwrap();
        assert_eq!(
            text,
            r#"{"at":["09:00"],"months":["jan"],"days":["first mon"]}"#
        );
        assert_eq!(serde_json::from_str::<Spec>(&text).unwrap(), named);
    }

    #[test]
    fn a_spec_keeps_its_durations_as_a_person_writes_them() {
        let spec = Spec {
            every: Some(Duration::from_secs(2 * 86_400)),
            jitter: Some(Duration::from_secs(600)),
            ..Spec::default()
        };
        let text = serde_json::to_string(&spec).unwrap();
        assert_eq!(text, r#"{"every":"2d","jitter":"10m"}"#);
        assert_eq!(serde_json::from_str::<Spec>(&text).unwrap(), spec);
        assert!(serde_json::from_str::<Spec>(r#"{"every":"often"}"#).is_err());
    }
}
