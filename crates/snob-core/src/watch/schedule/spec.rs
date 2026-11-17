//! A schedule as it is written, before it is built: `--every`, `--on`/`--at`,
//! `--cron` and the jitter, in the words they were given in. What the flags,
//! `watch.toml` and an automation's trigger all hold, and **the one place they
//! are built into a [`Schedule`]**.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Schedule, ScheduleError, Weekday, parse_time};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "text")]
    pub jitter: Option<Duration>,
}

impl Spec {
    /// Whether it names no schedule at all: no `every`, `on`, `at` or `cron`.
    pub fn names_none(&self) -> bool {
        self.every.is_none() && self.on.is_empty() && self.at.is_empty() && self.cron.is_none()
    }

    /// The schedule it says, or why it says none. A cron expression beside
    /// days or times is refused: only the cron would be used.
    pub fn build(&self) -> Result<Schedule, ScheduleError> {
        if self.cron.is_some() && !(self.on.is_empty() && self.at.is_empty()) {
            return Err(ScheduleError::Unreadable(
                "\"cron\" and days or times are two ways of saying the same thing; keep one"
                    .to_string(),
            ));
        }
        let mut schedule = if let Some(expression) = &self.cron {
            Schedule::cron(expression)?
        } else if self.at.is_empty()
            && !self.on.is_empty()
            && let Some(every) = self.every
        {
            // Days and an interval and no time of day: before the calendar,
            // which would refuse days alone for naming no minute.
            Schedule::days(&days_named(&self.on)?, every)?
        } else if !self.at.is_empty() || !self.on.is_empty() {
            calendar_from(&self.on, &self.at)?
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
}

/// The two halves of a calendar, parsed and checked together.
pub fn calendar_from<S: AsRef<str>>(days: &[S], times: &[S]) -> Result<Schedule, ScheduleError> {
    let days = days_named(days)?;
    let times = times
        .iter()
        .map(|time| parse_time(time.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    Schedule::calendar(&days, &times)
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
