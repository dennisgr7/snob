//! A schedule as its dialog chooses it: how often it repeats (a year, a
//! month, a week, a day) and what each of those asks, or `watch.toml`'s
//! fields typed as they are written (Advanced). **What the choices cannot
//! show opens in Advanced, and a schedule opened and saved unchanged is the
//! one it opened with, byte for byte** ([`ScheduleForm::spec`]).
//!
//! "Every N days" and "every N weeks" keep the engine's meaning: a floor
//! from the last run, not a day it is anchored to.
//!
//! What is wrong with what is chosen is a [`Wrong`] beside the [`Field`] it
//! is about, for the app to say in its own words; what the engine refuses
//! is its `ScheduleError`, carried as it is. No window and no clock.

use std::collections::BTreeSet;
use std::time::Duration;

use snob_core::Epoch;
use snob_core::duration;
use snob_core::watch::schedule::{
    MIN_GAP_SECS, MonthDay, Nth, ScheduleError, Spec, Weekday, month_token, next_after,
    parse_month, parse_time,
};

/// How often a schedule repeats, as the dialog's first choice says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frequency {
    Year,
    Month,
    Week,
    Day,
    Advanced,
}

impl Frequency {
    pub const ALL: [Self; 5] = [
        Self::Year,
        Self::Month,
        Self::Week,
        Self::Day,
        Self::Advanced,
    ];
}

/// What a day's schedule repeats by: times of day, or a length of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayMode {
    AtTimes,
    Repeating,
}

/// The unit "every so often" is counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Minutes,
    Hours,
    Days,
    Weeks,
}

impl Unit {
    pub const ALL: [Self; 4] = [Self::Minutes, Self::Hours, Self::Days, Self::Weeks];

    pub fn secs(self) -> u64 {
        match self {
            Self::Minutes => 60,
            Self::Hours => 3_600,
            Self::Days => 86_400,
            Self::Weeks => 604_800,
        }
    }

    /// The largest unit `every` is a whole number of, and how many.
    pub fn of(every: Duration) -> Option<(u64, Self)> {
        let secs = every.as_secs();
        if secs == 0 || every.subsec_nanos() != 0 {
            return None;
        }
        [Self::Weeks, Self::Days, Self::Hours, Self::Minutes]
            .into_iter()
            .find(|unit| secs.is_multiple_of(unit.secs()))
            .map(|unit| (secs / unit.secs(), unit))
    }
}

/// How a month's days are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    Date,
    Weekday,
}

/// The days of a month a schedule runs on: dates, or one weekday of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DayPick {
    Dates(BTreeSet<u32>),
    Nth(Nth, Weekday),
}

impl DayPick {
    /// Dates alone, or one weekday of the month alone; nothing else.
    fn of(days: &[MonthDay]) -> Option<Self> {
        match days {
            [MonthDay::Nth(nth, weekday)] => Some(Self::Nth(*nth, *weekday)),
            _ => days
                .iter()
                .map(|day| match day {
                    MonthDay::Date(date) => Some(*date),
                    MonthDay::Nth(..) => None,
                })
                .collect::<Option<BTreeSet<u32>>>()
                .map(Self::Dates),
        }
    }

    fn tokens(&self) -> Vec<String> {
        match self {
            Self::Dates(dates) => dates.iter().map(u32::to_string).collect(),
            Self::Nth(nth, weekday) => vec![MonthDay::Nth(*nth, *weekday).token()],
        }
    }
}

/// A time of day: hour and minute.
pub type Time = (u32, u32);

/// The months "every `every` months from `first`" lands on: none when it
/// is every month, which is how a schedule says every month.
pub fn months_of(every: u32, first: u32) -> Vec<u32> {
    if every <= 1 {
        return Vec::new();
    }
    (1..=12)
        .filter(|month| (month + 12 - first.clamp(1, 12)).is_multiple_of(every))
        .collect()
}

/// How many months apart a schedule may run: what divides a year, so the
/// months it lands on are the same each year.
pub const EVERY_MONTHS: [u32; 5] = [1, 2, 3, 4, 6];

/// A schedule the choices can show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    /// Every so often, counted from the last run.
    Interval { count: u64, unit: Unit },
    /// At these times, every day or every few days (one time then).
    Daily { every_days: u64, times: Vec<Time> },
    /// On these weekdays at these times, or every few weeks on one weekday
    /// at one time or at any.
    Weekly {
        every_weeks: u64,
        weekdays: BTreeSet<Weekday>,
        times: Vec<Time>,
    },
    /// On these days of every month, or of every few months from `first`.
    Monthly {
        every_months: u32,
        first: u32,
        day: DayPick,
        times: Vec<Time>,
    },
    /// On these days of these months.
    Yearly {
        months: BTreeSet<u32>,
        day: DayPick,
        times: Vec<Time>,
    },
}

impl Shape {
    /// The shape `spec` has, when the choices can show it: **only when the
    /// shape's own schedule is the one `spec` builds**, so what is shown
    /// means what is kept.
    pub fn of(spec: &Spec) -> Option<Self> {
        let shape = Self::read(spec)?;
        (shape.spec(spec.jitter).build() == spec.build()).then_some(shape)
    }

    fn read(spec: &Spec) -> Option<Self> {
        if spec.cron.is_some() {
            return None;
        }
        let times = times_of(&spec.at)?;
        let weekdays = spec
            .on
            .iter()
            .map(|day| Weekday::parse(day))
            .collect::<Option<BTreeSet<_>>>()?;
        let months = spec
            .months
            .iter()
            .map(|month| parse_month(month))
            .collect::<Option<BTreeSet<_>>>()?;
        let days = spec
            .days
            .iter()
            .map(|day| MonthDay::parse(day))
            .collect::<Option<Vec<_>>>()?;
        let of_a_month = !months.is_empty() || !days.is_empty();

        if weekdays.is_empty() && times.is_empty() && !of_a_month {
            let (count, unit) = Unit::of(spec.every?)?;
            return Some(Self::Interval { count, unit });
        }
        if !of_a_month && weekdays.is_empty() {
            return match spec.every {
                None => Some(Self::Daily {
                    every_days: 1,
                    times,
                }),
                Some(every) => {
                    let every_days = whole(every, Unit::Days)?;
                    (every_days >= 2 && times.len() == 1)
                        .then_some(Self::Daily { every_days, times })
                }
            };
        }
        if !of_a_month {
            return match spec.every {
                None => (!times.is_empty()).then_some(Self::Weekly {
                    every_weeks: 1,
                    weekdays,
                    times,
                }),
                Some(every) => {
                    let every_weeks = whole(every, Unit::Weeks)?;
                    (every_weeks >= 2 && weekdays.len() == 1 && times.len() <= 1).then_some(
                        Self::Weekly {
                            every_weeks,
                            weekdays,
                            times,
                        },
                    )
                }
            };
        }
        if spec.every.is_some() || !weekdays.is_empty() || times.is_empty() || days.is_empty() {
            return None;
        }
        let day = DayPick::of(&days)?;
        if months.is_empty() || months.len() == 12 {
            return Some(Self::Monthly {
                every_months: 1,
                first: 1,
                day,
                times,
            });
        }
        let listed: Vec<u32> = months.iter().copied().collect();
        let first = listed[0];
        for every_months in EVERY_MONTHS.into_iter().skip(1) {
            if months_of(every_months, first) == listed {
                return Some(Self::Monthly {
                    every_months,
                    first,
                    day,
                    times,
                });
            }
        }
        Some(Self::Yearly { months, day, times })
    }

    /// The schedule it says, in the tokens `watch.toml` writes (`mon`,
    /// `09:00`, `jan`, `first mon`), with `jitter`.
    pub fn spec(&self, jitter: Option<Duration>) -> Spec {
        let at = |times: &[Time]| -> Vec<String> {
            times
                .iter()
                .map(|(hour, minute)| format!("{hour:02}:{minute:02}"))
                .collect()
        };
        let span = |count: u64, unit: Unit| Some(Duration::from_secs(count * unit.secs()));
        let on = |weekdays: &BTreeSet<Weekday>| -> Vec<String> {
            weekdays.iter().map(|day| day.token().to_string()).collect()
        };
        let named = |months: &[u32]| -> Vec<String> {
            months
                .iter()
                .map(|month| month_token(*month).to_string())
                .collect()
        };
        let spec = match self {
            Self::Interval { count, unit } => Spec {
                every: span(*count, *unit),
                ..Spec::default()
            },
            Self::Daily { every_days, times } => Spec {
                every: (*every_days > 1)
                    .then(|| span(*every_days, Unit::Days))
                    .flatten(),
                at: at(times),
                ..Spec::default()
            },
            Self::Weekly {
                every_weeks,
                weekdays,
                times,
            } => Spec {
                every: (*every_weeks > 1)
                    .then(|| span(*every_weeks, Unit::Weeks))
                    .flatten(),
                on: on(weekdays),
                at: at(times),
                ..Spec::default()
            },
            Self::Monthly {
                every_months,
                first,
                day,
                times,
            } => Spec {
                at: at(times),
                months: named(&months_of(*every_months, *first)),
                days: day.tokens(),
                ..Spec::default()
            },
            Self::Yearly { months, day, times } => Spec {
                at: at(times),
                months: named(&months.iter().copied().collect::<Vec<_>>()),
                days: day.tokens(),
                ..Spec::default()
            },
        };
        Spec { jitter, ..spec }
    }
}

/// How many `unit`s `every` is, when it is a whole number of them.
fn whole(every: Duration, unit: Unit) -> Option<u64> {
    let secs = every.as_secs();
    (every.subsec_nanos() == 0 && secs.is_multiple_of(unit.secs())).then(|| secs / unit.secs())
}

/// Times of day, read, in order and once each.
fn times_of<S: AsRef<str>>(texts: &[S]) -> Option<Vec<Time>> {
    let times: BTreeSet<Time> = texts
        .iter()
        .map(|text| parse_time(text.as_ref()).ok())
        .collect::<Option<_>>()?;
    Some(times.into_iter().collect())
}

/// Where a [`Wrong`] is shown: under the field it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Count,
    Weekdays,
    Months,
    Dates,
    Times,
    Every,
    On,
    At,
    MonthsTyped,
    DaysTyped,
    Cron,
    Jitter,
    /// The schedule as a whole: under every field.
    Whole,
}

/// What keeps what is chosen from being a schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wrong {
    NoTime,
    /// Text that is not a time of day.
    Time(String),
    /// Several times where the schedule runs every few days or weeks.
    OneTime,
    NoWeekday,
    /// Several weekdays where the schedule runs every few weeks.
    OneWeekday,
    NoDate,
    NoMonth,
    /// Text that is not a whole number of 1 or more.
    Count(String),
    /// Text that is not a length of time.
    Duration(String),
    Weekday(String),
    Month(String),
    MonthDay(String),
    CronAndCalendar,
    DaysAndWeekdays,
    /// A cron expression the engine cannot read.
    Cron(String),
    /// Nothing chosen ever comes round: the 30th of February.
    Never,
    Engine(ScheduleError),
}

/// A [`Wrong`] and the field it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub field: Field,
    pub wrong: Wrong,
}

fn refused(field: Field, wrong: Wrong) -> Refused {
    Refused { field, wrong }
}

/// `Err` of `wrong` at `field` unless `holds`.
fn need(holds: bool, field: Field, wrong: Wrong) -> Result<(), Refused> {
    if holds {
        Ok(())
    } else {
        Err(refused(field, wrong))
    }
}

/// The largest count typed: the largest no unit overflows. **How long an
/// interval may be is the engine's to say** (`ScheduleError::TooRare`), so a
/// count past a year is refused in its words, and every interval it accepts
/// can be typed in minutes.
pub const MOST_COUNT: u64 = u64::MAX / 604_800;

/// The choices of the dialog, apart from what is typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    pub frequency: Frequency,
    pub day_mode: DayMode,
    pub unit: Unit,
    pub weekdays: BTreeSet<Weekday>,
    /// How many months apart, one of [`EVERY_MONTHS`].
    pub every_months: u32,
    /// The month every few months starts from, 1 to 12.
    pub first_month: u32,
    pub months: BTreeSet<u32>,
    pub by: By,
    pub dates: BTreeSet<u32>,
    pub nth: Nth,
    pub nth_weekday: Weekday,
    /// Kept from the schedule opened: only Advanced types it.
    pub jitter: Option<Duration>,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            frequency: Frequency::Day,
            day_mode: DayMode::AtTimes,
            unit: Unit::Hours,
            weekdays: BTreeSet::from([Weekday::Mon]),
            every_months: 1,
            first_month: 1,
            months: BTreeSet::from([1]),
            by: By::Date,
            dates: BTreeSet::from([1]),
            nth: Nth::First,
            nth_weekday: Weekday::Mon,
            jitter: None,
        }
    }
}

/// `watch.toml`'s fields as they are typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Advanced {
    pub every: String,
    pub on: String,
    pub at: String,
    pub months: String,
    pub days: String,
    pub cron: String,
    pub jitter: String,
}

/// The words typed, apart: by commas or spaces, whichever they were typed
/// with.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The days of a month typed, apart by commas alone: `first mon` is one.
fn listed(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|day| !day.is_empty())
        .map(str::to_string)
        .collect()
}

fn nothing_as_none(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

impl Advanced {
    /// The fields of a schedule already written.
    pub fn of(spec: &Spec) -> Self {
        Self {
            every: spec.every.map(duration::format).unwrap_or_default(),
            on: spec.on.join(", "),
            at: spec.at.join(", "),
            months: spec.months.join(", "),
            days: spec.days.join(", "),
            cron: spec.cron.clone().unwrap_or_default(),
            jitter: spec.jitter.map(duration::format).unwrap_or_default(),
        }
    }

    /// The schedule as typed, each word read as the engine reads it.
    pub fn spec(&self) -> Result<Spec, Refused> {
        let span = |text: &str, field: Field| match nothing_as_none(text) {
            Some(text) => duration::parse(&text)
                .map(Some)
                .map_err(|_| refused(field, Wrong::Duration(text))),
            None => Ok(None),
        };
        let every = span(&self.every, Field::Every)?;
        let on = words(&self.on);
        if let Some(wrong) = on.iter().find(|day| Weekday::parse(day).is_none()) {
            return Err(refused(Field::On, Wrong::Weekday(wrong.clone())));
        }
        let days = listed(&self.days);
        if let Some(wrong) = days.iter().find(|day| MonthDay::parse(day).is_none()) {
            return Err(refused(Field::DaysTyped, Wrong::MonthDay(wrong.clone())));
        }
        let months = words(&self.months);
        if let Some(wrong) = months.iter().find(|month| parse_month(month).is_none()) {
            return Err(refused(Field::MonthsTyped, Wrong::Month(wrong.clone())));
        }
        let at = words(&self.at);
        if let Some(wrong) = at.iter().find(|time| parse_time(time).is_err()) {
            return Err(refused(Field::At, Wrong::Time(wrong.clone())));
        }
        let cron = nothing_as_none(&self.cron);
        let calendar = !(on.is_empty() && at.is_empty() && months.is_empty() && days.is_empty());
        need(
            cron.is_none() || !calendar,
            Field::Cron,
            Wrong::CronAndCalendar,
        )?;
        need(
            on.is_empty() || days.is_empty(),
            Field::DaysTyped,
            Wrong::DaysAndWeekdays,
        )?;
        let needs_a_time =
            !months.is_empty() || !days.is_empty() || (!on.is_empty() && every.is_none());
        need(!(needs_a_time && at.is_empty()), Field::At, Wrong::NoTime)?;
        let jitter = span(&self.jitter, Field::Jitter)?;
        Ok(Spec {
            every,
            on,
            at,
            months,
            days,
            cron,
            jitter,
        })
    }
}

/// What is typed in the dialog: the count of its stepper, its times of
/// day, and the fields of Advanced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typed {
    pub count: String,
    pub times: Vec<String>,
    pub advanced: Advanced,
}

/// What the form opened with, which an unchanged form saves as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Opened {
    spec: Spec,
    frequency: Frequency,
    shape: Option<Shape>,
    jitter: Option<Duration>,
    advanced: Advanced,
}

/// A schedule as its dialog holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleForm {
    pub choices: Choices,
    pub typed: Typed,
    /// Opened in Advanced because the choices could not show it.
    pub kept: bool,
    opened: Box<Opened>,
}

impl ScheduleForm {
    /// The form a schedule opens in: its shape's choices, or Advanced when
    /// the choices cannot show it; times of day to type when it names
    /// nothing yet.
    pub fn of(spec: &Spec) -> Self {
        let mut choices = Choices {
            jitter: spec.jitter,
            ..Choices::default()
        };
        let mut typed = Typed {
            count: "1".to_string(),
            times: vec![String::new()],
            advanced: Advanced::of(spec),
        };
        let shape = (!spec.names_none()).then(|| Shape::of(spec)).flatten();
        let kept = !spec.names_none() && shape.is_none();
        match &shape {
            Some(shape) => choices.show(shape, &mut typed),
            None if kept => choices.frequency = Frequency::Advanced,
            None => {}
        }
        let opened = Opened {
            spec: spec.clone(),
            frequency: choices.frequency,
            shape,
            jitter: spec.jitter,
            advanced: typed.advanced.clone(),
        };
        Self {
            choices,
            typed,
            kept,
            opened: Box::new(opened),
        }
    }

    /// The schedule chosen: **the one opened, as it was written, when
    /// what is chosen still says it**; else as the choices write it.
    pub fn spec(&self) -> Result<Spec, Refused> {
        let opened = &self.opened;
        let unchanged = match self.choices.frequency {
            Frequency::Advanced => self.typed.advanced == opened.advanced,
            frequency => {
                frequency == opened.frequency
                    && self.choices.jitter == opened.jitter
                    && opened.shape.is_some()
                    && self.shape().ok() == opened.shape
            }
        };
        if unchanged {
            return Ok(opened.spec.clone());
        }
        self.chosen()
    }

    /// The schedule as the choices or Advanced's fields write it.
    fn chosen(&self) -> Result<Spec, Refused> {
        match self.choices.frequency {
            Frequency::Advanced => self.typed.advanced.spec(),
            _ => self.shape().map(|shape| shape.spec(self.choices.jitter)),
        }
    }

    /// The shape the choices make, or what keeps them from making one, the
    /// first from the top of the dialog. Advanced has none.
    pub fn shape(&self) -> Result<Shape, Refused> {
        let choices = &self.choices;
        match choices.frequency {
            Frequency::Advanced => Err(refused(Field::Whole, Wrong::Engine(ScheduleError::Empty))),
            Frequency::Day => {
                let count = self.count()?;
                if choices.day_mode == DayMode::Repeating {
                    return Ok(Shape::Interval {
                        count,
                        unit: choices.unit,
                    });
                }
                let times = self.times()?;
                need(!times.is_empty(), Field::Times, Wrong::NoTime)?;
                need(count == 1 || times.len() == 1, Field::Times, Wrong::OneTime)?;
                Ok(Shape::Daily {
                    every_days: count,
                    times,
                })
            }
            Frequency::Week => {
                let count = self.count()?;
                need(
                    !choices.weekdays.is_empty(),
                    Field::Weekdays,
                    Wrong::NoWeekday,
                )?;
                need(
                    count == 1 || choices.weekdays.len() == 1,
                    Field::Weekdays,
                    Wrong::OneWeekday,
                )?;
                let times = self.times()?;
                if count == 1 {
                    need(!times.is_empty(), Field::Times, Wrong::NoTime)?;
                } else {
                    need(times.len() <= 1, Field::Times, Wrong::OneTime)?;
                }
                Ok(Shape::Weekly {
                    every_weeks: count,
                    weekdays: choices.weekdays.clone(),
                    times,
                })
            }
            Frequency::Month => {
                let day = self.day()?;
                let times = self.times()?;
                need(!times.is_empty(), Field::Times, Wrong::NoTime)?;
                let every_months = if EVERY_MONTHS.contains(&choices.every_months) {
                    choices.every_months
                } else {
                    1
                };
                Ok(Shape::Monthly {
                    every_months,
                    first: if every_months == 1 {
                        1
                    } else {
                        choices.first_month.clamp(1, 12)
                    },
                    day,
                    times,
                })
            }
            Frequency::Year => {
                need(!choices.months.is_empty(), Field::Months, Wrong::NoMonth)?;
                let day = self.day()?;
                let times = self.times()?;
                need(!times.is_empty(), Field::Times, Wrong::NoTime)?;
                Ok(Shape::Yearly {
                    months: choices.months.clone(),
                    day,
                    times,
                })
            }
        }
    }

    /// The count typed: a whole number of 1 or more.
    fn count(&self) -> Result<u64, Refused> {
        let text = self.typed.count.trim();
        text.parse::<u64>()
            .ok()
            .filter(|count| (1..=MOST_COUNT).contains(count))
            .ok_or_else(|| refused(Field::Count, Wrong::Count(text.to_string())))
    }

    /// The count typed as a number, or 1 when it is none.
    fn count_or_one(&self) -> u64 {
        self.count().unwrap_or(1)
    }

    /// The times typed, read; a row left empty says nothing.
    fn times(&self) -> Result<Vec<Time>, Refused> {
        let mut times = BTreeSet::new();
        for text in &self.typed.times {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let time = parse_time(text)
                .map_err(|_| refused(Field::Times, Wrong::Time(text.to_string())))?;
            times.insert(time);
        }
        Ok(times.into_iter().collect())
    }

    fn day(&self) -> Result<DayPick, Refused> {
        let choices = &self.choices;
        match choices.by {
            By::Date => {
                need(!choices.dates.is_empty(), Field::Dates, Wrong::NoDate)?;
                Ok(DayPick::Dates(choices.dates.clone()))
            }
            By::Weekday => Ok(DayPick::Nth(choices.nth, choices.nth_weekday)),
        }
    }

    /// What keeps the schedule chosen from being used at `now`, said where
    /// the app's words can say it, else the engine's reason; nothing when
    /// it can run.
    pub fn problem(&self, now: Epoch) -> Option<Refused> {
        let spec = match self.spec() {
            Ok(spec) => spec,
            Err(refused) => return Some(refused),
        };
        match spec.build() {
            // Whether a day ever comes does not depend on the zone.
            Ok(schedule) => next_after(&schedule, None, now, &chrono::Utc)
                .is_none()
                .then(|| refused(self.never_field(&spec), Wrong::Never)),
            Err(why) => Some(
                self.chosen()
                    .err()
                    .unwrap_or_else(|| self.refused_by_engine(why, &spec)),
            ),
        }
    }

    fn refused_by_engine(&self, why: ScheduleError, spec: &Spec) -> Refused {
        let advanced = self.choices.frequency == Frequency::Advanced;
        let too_short = spec
            .every
            .is_some_and(|every| every.as_secs() < MIN_GAP_SECS.unsigned_abs());
        let field = match (&why, advanced) {
            (ScheduleError::Empty, _) => Field::Whole,
            (_, true) if spec.cron.is_some() => Field::Cron,
            (ScheduleError::TooRare { .. }, true) => Field::Every,
            (ScheduleError::TooOften { .. }, true) if too_short => Field::Every,
            (ScheduleError::TooOften { .. }, true) => Field::At,
            (ScheduleError::Unreadable(_), _) => Field::Whole,
            (ScheduleError::TooRare { .. }, false) => Field::Count,
            (ScheduleError::TooOften { .. }, false)
                if self.choices.frequency == Frequency::Day
                    && self.choices.day_mode == DayMode::Repeating =>
            {
                Field::Count
            }
            (ScheduleError::TooOften { .. }, false) => Field::Times,
        };
        let wrong = match (&why, &spec.cron) {
            (ScheduleError::Unreadable(_), Some(cron)) => Wrong::Cron(cron.clone()),
            _ => Wrong::Engine(why),
        };
        refused(field, wrong)
    }

    fn never_field(&self, spec: &Spec) -> Field {
        match self.choices.frequency {
            Frequency::Month | Frequency::Year => Field::Dates,
            Frequency::Advanced if spec.cron.is_some() => Field::Cron,
            Frequency::Advanced => Field::DaysTyped,
            Frequency::Week | Frequency::Day => Field::Whole,
        }
    }

    /// How often it repeats. **Advanced is filled from the choices** when
    /// they make a schedule, so what was chosen can be written further;
    /// the choices take Advanced's jitter back when it is one.
    pub fn choose(&mut self, frequency: Frequency) {
        let was = self.choices.frequency;
        if frequency == was {
            return;
        }
        if frequency == Frequency::Advanced
            && let Ok(spec) = self.chosen()
        {
            self.typed.advanced = Advanced::of(&spec);
        } else if was == Frequency::Advanced
            && let Ok(spec) = self.typed.advanced.spec()
        {
            self.choices.jitter = spec.jitter;
        }
        self.choices.frequency = frequency;
    }

    /// Whether the count is a floor from the last run, which the dialog
    /// says: every so often, or every few days or weeks.
    pub fn counts_from_the_last_run(&self) -> bool {
        match self.choices.frequency {
            Frequency::Day => {
                self.choices.day_mode == DayMode::Repeating || self.count_or_one() > 1
            }
            Frequency::Week => self.count_or_one() > 1,
            Frequency::Month | Frequency::Year | Frequency::Advanced => false,
        }
    }

    /// The months every few months lands on, 1 to 12.
    pub fn lands_on(&self) -> Vec<u32> {
        let every = self.choices.every_months;
        match months_of(every, self.choices.first_month) {
            none if none.is_empty() => (1..=12).collect(),
            months => months,
        }
    }

    /// Whether a date chosen is one some of the months it runs in lack.
    pub fn some_months_lack_a_date(&self) -> bool {
        let months: Vec<u32> = match self.choices.frequency {
            Frequency::Month => self.lands_on(),
            Frequency::Year => self.choices.months.iter().copied().collect(),
            Frequency::Week | Frequency::Day | Frequency::Advanced => return false,
        };
        let Some(shortest) = months.into_iter().map(days_in).min() else {
            return false;
        };
        self.choices.by == By::Date && self.choices.dates.iter().any(|date| *date > shortest)
    }
}

/// The days `month` has in most years: the 29th of February is skipped
/// three years in four.
fn days_in(month: u32) -> u32 {
    match month {
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Puts `item` in `set`, or takes it out.
pub fn toggle<T: Ord>(set: &mut BTreeSet<T>, item: T) {
    if !set.remove(&item) {
        set.insert(item);
    }
}

impl Choices {
    fn show_day(&mut self, day: &DayPick) {
        match day {
            DayPick::Dates(dates) => {
                self.by = By::Date;
                self.dates = dates.clone();
            }
            DayPick::Nth(nth, weekday) => {
                self.by = By::Weekday;
                self.nth = *nth;
                self.nth_weekday = *weekday;
            }
        }
    }

    /// The choices that show `shape`, and its count and times typed.
    fn show(&mut self, shape: &Shape, typed: &mut Typed) {
        let written = |times: &[Time]| -> Vec<String> {
            if times.is_empty() {
                vec![String::new()]
            } else {
                times
                    .iter()
                    .map(|(hour, minute)| format!("{hour:02}:{minute:02}"))
                    .collect()
            }
        };
        match shape {
            Shape::Interval { count, unit } => {
                self.frequency = Frequency::Day;
                self.day_mode = DayMode::Repeating;
                self.unit = *unit;
                typed.count = count.to_string();
            }
            Shape::Daily { every_days, times } => {
                self.frequency = Frequency::Day;
                self.day_mode = DayMode::AtTimes;
                typed.count = every_days.to_string();
                typed.times = written(times);
            }
            Shape::Weekly {
                every_weeks,
                weekdays,
                times,
            } => {
                self.frequency = Frequency::Week;
                self.weekdays = weekdays.clone();
                typed.count = every_weeks.to_string();
                typed.times = written(times);
            }
            Shape::Monthly {
                every_months,
                first,
                day,
                times,
            } => {
                self.frequency = Frequency::Month;
                self.every_months = *every_months;
                self.first_month = *first;
                self.show_day(day);
                typed.times = written(times);
            }
            Shape::Yearly { months, day, times } => {
                self.frequency = Frequency::Year;
                self.months = months.clone();
                self.show_day(day);
                typed.times = written(times);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use snob_core::watch::schedule::MAX_INTERVAL_SECS;

    const DAY: u64 = 86_400;

    fn strings(all: &[&str]) -> Vec<String> {
        all.iter().map(|one| (*one).to_string()).collect()
    }

    fn spec_at(times: &[&str]) -> Spec {
        Spec {
            at: strings(times),
            ..Spec::default()
        }
    }

    /// The form of `spec`, its times typed anew.
    fn typed_in(spec: &Spec, times: &[&str]) -> ScheduleForm {
        let mut form = ScheduleForm::of(spec);
        form.typed.times = strings(times);
        form
    }

    #[test]
    fn each_frequency_writes_its_spec_and_opens_in_it_again() {
        let mut form = ScheduleForm::of(&Spec::default());
        assert_eq!(form.choices.frequency, Frequency::Day);
        form.typed.times = strings(&["18:00", "9:00"]);
        assert_eq!(form.spec(), Ok(spec_at(&["09:00", "18:00"])));

        form.choose(Frequency::Week);
        form.choices.weekdays = BTreeSet::from([Weekday::Mon, Weekday::Thu]);
        let weekly = form.spec().unwrap();
        assert_eq!(weekly.on, ["mon", "thu"]);
        assert_eq!(weekly.at, ["09:00", "18:00"]);

        form.choose(Frequency::Month);
        form.choices.by = By::Weekday;
        form.typed.times = strings(&["09:00"]);
        let monthly = form.spec().unwrap();
        assert_eq!(monthly.days, ["first mon"]);
        assert!(monthly.months.is_empty(), "every month");

        form.choices.every_months = 3;
        form.choices.first_month = 2;
        assert_eq!(form.spec().unwrap().months, ["feb", "may", "aug", "nov"]);

        form.choose(Frequency::Year);
        form.choices.by = By::Date;
        form.choices.months = BTreeSet::from([3, 9, 12]);
        form.choices.dates = BTreeSet::from([1, 15]);
        let yearly = form.spec().unwrap();
        assert_eq!(
            (yearly.months, yearly.days),
            (strings(&["mar", "sep", "dec"]), strings(&["1", "15"]))
        );

        form.choose(Frequency::Day);
        form.choices.day_mode = DayMode::Repeating;
        form.typed.count = "6".to_string();
        assert_eq!(
            form.spec().unwrap().every,
            Some(Duration::from_secs(6 * 3_600))
        );
    }

    #[test]
    fn every_shape_the_form_builds_reads_back_as_the_same_shape() {
        let shapes = [
            Shape::Interval {
                count: 30,
                unit: Unit::Minutes,
            },
            Shape::Interval {
                count: 2,
                unit: Unit::Weeks,
            },
            Shape::Daily {
                every_days: 1,
                times: vec![(9, 0), (18, 30)],
            },
            Shape::Daily {
                every_days: 3,
                times: vec![(7, 15)],
            },
            Shape::Weekly {
                every_weeks: 1,
                weekdays: BTreeSet::from([Weekday::Mon, Weekday::Fri]),
                times: vec![(12, 18)],
            },
            Shape::Weekly {
                every_weeks: 2,
                weekdays: BTreeSet::from([Weekday::Mon]),
                times: vec![],
            },
            Shape::Monthly {
                every_months: 1,
                first: 1,
                day: DayPick::Nth(Nth::Last, Weekday::Fri),
                times: vec![(9, 0)],
            },
            Shape::Monthly {
                every_months: 4,
                first: 3,
                day: DayPick::Dates(BTreeSet::from([1, 31])),
                times: vec![(8, 0)],
            },
            Shape::Yearly {
                months: BTreeSet::from([1, 2, 7]),
                day: DayPick::Dates(BTreeSet::from([15])),
                times: vec![(8, 0)],
            },
        ];
        for shape in shapes {
            let spec = shape.spec(None);
            assert_eq!(Shape::of(&spec), Some(shape.clone()), "{spec:?}");
            let form = ScheduleForm::of(&spec);
            assert_eq!(form.shape(), Ok(shape), "{spec:?}");
            assert!(!form.kept);
        }
    }

    #[test]
    fn a_schedule_opens_in_the_frequency_that_shows_it() {
        let opens_in = |spec: Spec| ScheduleForm::of(&spec).choices.frequency;
        let every = |secs: u64| Some(Duration::from_secs(secs));
        assert_eq!(
            opens_in(Spec {
                every: every(6 * 3_600),
                ..Spec::default()
            }),
            Frequency::Day
        );
        assert_eq!(opens_in(spec_at(&["09:00"])), Frequency::Day);
        assert_eq!(
            opens_in(Spec {
                on: strings(&["mon"]),
                every: every(14 * DAY),
                ..Spec::default()
            }),
            Frequency::Week
        );
        assert_eq!(
            opens_in(Spec {
                days: strings(&["first mon"]),
                months: strings(&["jan", "jul"]),
                at: strings(&["09:00"]),
                ..Spec::default()
            }),
            Frequency::Month,
            "January and July are every six months"
        );
        assert_eq!(
            opens_in(Spec {
                days: strings(&["1"]),
                months: strings(&["jan", "feb"]),
                at: strings(&["09:00"]),
                ..Spec::default()
            }),
            Frequency::Year
        );
        for unshown in [
            Spec {
                cron: Some("0 9 * * 1".to_string()),
                ..Spec::default()
            },
            Spec {
                every: every(2 * DAY),
                at: strings(&["09:00", "18:00"]),
                ..Spec::default()
            },
            Spec {
                every: every(14 * DAY),
                on: strings(&["mon", "thu"]),
                at: strings(&["09:00"]),
                ..Spec::default()
            },
            Spec {
                days: strings(&["1", "first mon"]),
                at: strings(&["09:00"]),
                ..Spec::default()
            },
            Spec {
                months: strings(&["dec"]),
                at: strings(&["09:00"]),
                ..Spec::default()
            },
            Spec {
                every: every(90),
                ..Spec::default()
            },
        ] {
            let form = ScheduleForm::of(&unshown);
            assert_eq!(form.choices.frequency, Frequency::Advanced, "{unshown:?}");
            assert!(form.kept);
        }
        let empty = ScheduleForm::of(&Spec::default());
        assert_eq!(empty.choices.frequency, Frequency::Day);
        assert!(!empty.kept, "nothing to keep");
    }

    #[test]
    fn a_schedule_the_choices_cannot_show_opens_in_advanced_and_is_saved_unchanged() {
        let cron = Spec {
            cron: Some("0  9 * * 1-5".to_string()),
            jitter: Some(Duration::from_secs(300)),
            ..Spec::default()
        };
        let form = ScheduleForm::of(&cron);
        assert_eq!(form.choices.frequency, Frequency::Advanced);
        assert_eq!(form.spec(), Ok(cron.clone()), "byte for byte");

        // Written by hand, in words the choices would write otherwise.
        let by_hand = Spec {
            on: strings(&["Monday", "thu"]),
            at: strings(&["9:00"]),
            ..Spec::default()
        };
        let form = ScheduleForm::of(&by_hand);
        assert_eq!(form.choices.frequency, Frequency::Week);
        assert_eq!(form.spec(), Ok(by_hand.clone()), "unchanged, as written");
        let mut changed = form.clone();
        changed.choices.weekdays.insert(Weekday::Fri);
        assert_eq!(changed.spec().unwrap().on, ["mon", "thu", "fri"]);
    }

    #[test]
    fn every_interval_the_engine_takes_opens_in_the_choices_and_saves_unchanged() {
        let now = Epoch::new(1_790_000_000);
        let most = MAX_INTERVAL_SECS.unsigned_abs();
        for (secs, count, unit) in [
            (10_230 * 60, 10_230, Unit::Minutes),
            (most - 60, most / 60 - 1, Unit::Minutes),
            (most, most / DAY, Unit::Days),
            (52 * 7 * DAY, 52, Unit::Weeks),
        ] {
            let spec = Spec {
                every: Some(Duration::from_secs(secs)),
                ..Spec::default()
            };
            let form = ScheduleForm::of(&spec);
            assert_eq!(
                (
                    form.choices.frequency,
                    form.choices.day_mode,
                    form.choices.unit
                ),
                (Frequency::Day, DayMode::Repeating, unit),
                "{spec:?}"
            );
            assert_eq!(form.typed.count, count.to_string());
            assert_eq!(form.spec(), Ok(spec.clone()));
            assert_eq!(form.problem(now), None, "{spec:?}");
        }

        let mut form = ScheduleForm::of(&Spec::default());
        form.choices.day_mode = DayMode::Repeating;
        form.choices.unit = Unit::Minutes;
        form.typed.count = (most / 60 + 1).to_string();
        assert!(
            matches!(
                form.problem(now),
                Some(Refused {
                    field: Field::Count,
                    wrong: Wrong::Engine(ScheduleError::TooRare { .. })
                })
            ),
            "past a year is the engine's to say"
        );
        form.typed.count = u64::MAX.to_string();
        assert_eq!(
            form.problem(now),
            Some(refused(Field::Count, Wrong::Count(u64::MAX.to_string()))),
            "a count that cannot be a length of time"
        );
    }

    #[test]
    fn every_few_months_lands_on_the_months_from_the_first() {
        assert_eq!(months_of(1, 5), Vec::<u32>::new(), "every month");
        assert_eq!(months_of(2, 1), [1, 3, 5, 7, 9, 11]);
        assert_eq!(months_of(3, 2), [2, 5, 8, 11]);
        assert_eq!(months_of(4, 12), [4, 8, 12]);
        assert_eq!(months_of(6, 9), [3, 9]);
        let mut form = ScheduleForm::of(&Spec::default());
        form.choose(Frequency::Month);
        assert_eq!(form.lands_on().len(), 12);
        form.choices.every_months = 6;
        form.choices.first_month = 7;
        assert_eq!(form.lands_on(), [1, 7]);
    }

    #[test]
    fn what_cannot_be_a_schedule_is_refused_beside_the_field_it_is_about() {
        let now = Epoch::new(1_790_000_000);
        let wrong = |form: &ScheduleForm| form.problem(now);
        let mut form = ScheduleForm::of(&Spec::default());
        assert_eq!(
            wrong(&form),
            Some(refused(Field::Times, Wrong::NoTime)),
            "nothing typed yet"
        );
        form.typed.times = strings(&["soon"]);
        assert_eq!(
            wrong(&form),
            Some(refused(Field::Times, Wrong::Time("soon".to_string())))
        );
        form.typed.times = strings(&["09:00"]);
        assert_eq!(wrong(&form), None);
        form.typed.count = "0".to_string();
        assert_eq!(
            wrong(&form),
            Some(refused(Field::Count, Wrong::Count("0".to_string())))
        );
        form.typed.count = "2".to_string();
        form.typed.times = strings(&["09:00", "18:00"]);
        assert_eq!(wrong(&form), Some(refused(Field::Times, Wrong::OneTime)));

        form.typed.times = strings(&["09:00"]);
        form.choose(Frequency::Week);
        form.choices.weekdays.clear();
        assert_eq!(
            wrong(&form),
            Some(refused(Field::Weekdays, Wrong::NoWeekday))
        );
        form.choices.weekdays = BTreeSet::from([Weekday::Mon, Weekday::Tue]);
        assert_eq!(
            wrong(&form),
            Some(refused(Field::Weekdays, Wrong::OneWeekday))
        );
        form.typed.count = "60".to_string();
        form.choices.weekdays = BTreeSet::from([Weekday::Mon]);
        assert!(matches!(
            wrong(&form),
            Some(Refused {
                field: Field::Count,
                wrong: Wrong::Engine(ScheduleError::TooRare { .. })
            })
        ));

        form.choose(Frequency::Year);
        form.choices.months.clear();
        assert_eq!(wrong(&form), Some(refused(Field::Months, Wrong::NoMonth)));
        form.choices.months = BTreeSet::from([2]);
        form.choices.dates = BTreeSet::from([30]);
        assert_eq!(wrong(&form), Some(refused(Field::Dates, Wrong::Never)));
        form.choices.dates.clear();
        assert_eq!(wrong(&form), Some(refused(Field::Dates, Wrong::NoDate)));

        form.choose(Frequency::Day);
        form.choices.day_mode = DayMode::Repeating;
        form.typed.count = "5".to_string();
        form.choices.unit = Unit::Minutes;
        assert!(matches!(
            wrong(&form),
            Some(Refused {
                field: Field::Count,
                wrong: Wrong::Engine(ScheduleError::TooOften { .. })
            })
        ));
    }

    #[test]
    fn advanced_says_which_word_it_cannot_read() {
        let now = Epoch::new(1_790_000_000);
        let mut form = ScheduleForm::of(&Spec::default());
        form.choose(Frequency::Advanced);
        let typed = |change: fn(&mut Advanced), form: &mut ScheduleForm| {
            form.typed.advanced = Advanced::default();
            change(&mut form.typed.advanced);
            form.problem(now)
        };
        for (change, field, wrong) in [
            (
                (|a: &mut Advanced| a.every = "soon".into()) as fn(&mut Advanced),
                Field::Every,
                Wrong::Duration("soon".into()),
            ),
            (
                |a| {
                    a.on = "mon, someday".into();
                    a.at = "09:00".into();
                },
                Field::On,
                Wrong::Weekday("someday".into()),
            ),
            (
                |a| {
                    a.days = "1, fifth mon".into();
                    a.at = "09:00".into();
                },
                Field::DaysTyped,
                Wrong::MonthDay("fifth mon".into()),
            ),
            (
                |a| {
                    a.months = "smarch".into();
                    a.at = "09:00".into();
                },
                Field::MonthsTyped,
                Wrong::Month("smarch".into()),
            ),
            (
                |a| {
                    a.cron = "0 9 * * *".into();
                    a.at = "09:00".into();
                },
                Field::Cron,
                Wrong::CronAndCalendar,
            ),
            (
                |a| {
                    a.on = "mon".into();
                    a.days = "15".into();
                    a.at = "09:00".into();
                },
                Field::DaysTyped,
                Wrong::DaysAndWeekdays,
            ),
            (|a| a.days = "first mon".into(), Field::At, Wrong::NoTime),
            (
                |a| a.cron = "every morning".into(),
                Field::Cron,
                Wrong::Cron("every morning".into()),
            ),
            (|a| a.cron = "0 0 31 2 *".into(), Field::Cron, Wrong::Never),
        ] {
            assert_eq!(typed(change, &mut form), Some(refused(field, wrong)));
        }
        let fine = typed(
            |a| {
                a.days = "first mon, 15".into();
                a.months = "jan jul".into();
                a.at = "09:00".into();
                a.jitter = "5m".into();
            },
            &mut form,
        );
        assert_eq!(fine, None);
    }

    #[test]
    fn advanced_is_filled_from_the_choices_and_gives_its_jitter_back() {
        let mut form = typed_in(&Spec::default(), &["09:00"]);
        form.choose(Frequency::Advanced);
        assert_eq!(form.typed.advanced.at, "09:00");
        form.typed.advanced.jitter = "10m".to_string();
        form.choose(Frequency::Day);
        assert_eq!(form.choices.jitter, Some(Duration::from_secs(600)));
        assert_eq!(form.spec().unwrap().jitter, Some(Duration::from_secs(600)));
    }

    #[test]
    fn the_jitter_written_by_hand_survives_every_frequency() {
        let jitter = Some(Duration::from_secs(420));
        let spec = Spec {
            at: strings(&["09:00"]),
            jitter,
            ..Spec::default()
        };
        let mut form = ScheduleForm::of(&spec);
        for frequency in [Frequency::Week, Frequency::Month, Frequency::Year] {
            form.choose(frequency);
            assert_eq!(form.spec().unwrap().jitter, jitter, "{frequency:?}");
        }
    }

    #[test]
    fn the_count_is_a_floor_from_the_last_run_where_the_dialog_says_so() {
        let mut form = typed_in(&Spec::default(), &["09:00"]);
        assert!(!form.counts_from_the_last_run());
        form.typed.count = "3".to_string();
        assert!(form.counts_from_the_last_run());
        form.choose(Frequency::Month);
        assert!(!form.counts_from_the_last_run());
        form.choices.dates = BTreeSet::from([1, 29]);
        assert!(form.some_months_lack_a_date(), "February");
        form.choices.every_months = 6;
        form.choices.first_month = 1;
        form.choices.dates = BTreeSet::from([30]);
        assert!(!form.some_months_lack_a_date(), "January and July");
        form.choices.dates = BTreeSet::from([31]);
        form.choices.first_month = 3;
        assert!(form.some_months_lack_a_date(), "September");
        form.choose(Frequency::Year);
        form.choices.months = BTreeSet::from([1]);
        form.choices.dates = BTreeSet::from([30]);
        assert!(!form.some_months_lack_a_date(), "January has a 30th");
        form.choices.months.insert(2);
        assert!(form.some_months_lack_a_date(), "February has none");
    }

    /// Up to `most` of `all`, in any order, some twice, each in any of the
    /// spellings the engine reads.
    fn tokens(all: &'static [&'static [&'static str]], most: usize) -> BoxedStrategy<Vec<String>> {
        proptest::collection::vec((0..all.len(), 0..4usize), 0..=most)
            .prop_map(move |picked| {
                picked
                    .into_iter()
                    .map(|(item, spelling)| {
                        let spellings = all[item];
                        spellings[spelling % spellings.len()].to_string()
                    })
                    .collect()
            })
            .boxed()
    }

    const WEEKDAYS: &[&[&str]] = &[
        &["sun", "Sunday", "SUN", "0"],
        &["mon", "Monday", "MON", "1"],
        &["tue", "Tuesday", "Tues"],
        &["wed", "Wednesday", "weds"],
        &["thu", "Thursday", "THURS", "4"],
        &["fri", "Friday", "Fri"],
        &["sat", "saturday", "6"],
    ];

    const MONTHS: &[&[&str]] = &[
        &["jan", "January", "JAN", "1"],
        &["feb", "February", "2"],
        &["mar", "March"],
        &["apr", "April", "4"],
        &["may", "MAY"],
        &["jun", "June"],
        &["jul", "July", "7"],
        &["aug", "August"],
        &["sep", "sept", "September", "9"],
        &["oct", "October"],
        &["nov", "November", "11"],
        &["dec", "December", "DEC"],
    ];

    const DATES: &[&[&str]] = &[
        &["1", "01"],
        &["2"],
        &["15"],
        &["28"],
        &["29"],
        &["30"],
        &["31"],
    ];

    fn a_time() -> impl Strategy<Value = String> {
        (
            0u32..24,
            proptest::sample::select(vec![0u32, 5, 18, 30, 45]),
            any::<bool>(),
        )
            .prop_map(|(hour, minute, padded)| {
                if padded {
                    format!("{hour:02}:{minute:02}")
                } else {
                    format!("{hour}:{minute:02}")
                }
            })
    }

    /// Specs of every kind the dialog meets, written as a person may write
    /// them, the engine's own refusals left out.
    fn specs() -> impl Strategy<Value = Spec> {
        let most = MAX_INTERVAL_SECS.unsigned_abs();
        let every = prop_oneof![
            (1u64..=most / 60).prop_map(|n| n * 60),
            (1u64..=most / 3_600).prop_map(|n| n * 3_600),
            (1u64..=most / DAY).prop_map(|n| n * DAY),
            (1u64..=most / (7 * DAY)).prop_map(|n| n * 7 * DAY),
            (1u64..96).prop_map(|n| n * 15 * 60),
            (900u64..=most),
        ]
        .prop_map(Duration::from_secs);
        let days = prop_oneof![
            tokens(DATES, 4),
            (
                proptest::sample::select(vec![
                    "first", "second", "third", "fourth", "last", "1st", "First", "LAST",
                ]),
                proptest::sample::select(vec!["mon", "Monday", "tue", "sat", "SUN"]),
                proptest::sample::select(vec![" ", "-"]),
            )
                .prop_map(|(nth, day, between)| vec![format!("{nth}{between}{day}")]),
            Just(strings(&["1", "last fri"])),
        ];
        let months = prop_oneof![
            3 => tokens(MONTHS, 4),
            1 => Just(MONTHS.iter().map(|month| month[0].to_string()).collect()),
        ];
        let calendar = (
            proptest::option::weighted(0.3, every.clone()),
            tokens(WEEKDAYS, 4),
            proptest::collection::vec(a_time(), 0..3),
            months,
            proptest::option::weighted(0.5, days),
        )
            .prop_map(|(every, on, at, months, days)| {
                let days = days.unwrap_or_default();
                Spec {
                    every,
                    on: if days.is_empty() { on } else { Vec::new() },
                    at,
                    months,
                    days,
                    ..Spec::default()
                }
            });
        let interval = every.prop_map(|every| Spec {
            every: Some(every),
            ..Spec::default()
        });
        let cron = proptest::sample::select(vec![
            "0 9 * * 1",
            "30 6 1 * *",
            "0 */6 * * *",
            "15 8 * 3,9 1-5",
        ])
        .prop_map(|cron| Spec {
            cron: Some(cron.to_string()),
            ..Spec::default()
        });
        let jitter = proptest::option::of(
            proptest::sample::select(vec![60u64, 300, 900, 3_600]).prop_map(Duration::from_secs),
        );
        (prop_oneof![3 => calendar, 1 => interval, 1 => cron], jitter)
            .prop_map(|(spec, jitter)| Spec { jitter, ..spec })
            .prop_filter("the engine builds it", |spec| spec.build().is_ok())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 512,
            max_global_rejects: 1 << 16,
            ..ProptestConfig::default()
        })]

        /// Opened and saved unchanged, a schedule is the one it was, byte
        /// for byte; and what the dialog shows of it means the same.
        #[test]
        fn a_schedule_opened_and_saved_unchanged_is_the_same_spec(spec in specs()) {
            let form = ScheduleForm::of(&spec);
            prop_assert_eq!(form.spec(), Ok(spec.clone()));
            let shown = form.chosen();
            prop_assert!(shown.is_ok(), "{:?} shows {:?}", spec, shown);
            let shown = shown.unwrap();
            prop_assert_eq!(shown.build(), spec.build());
            if form.kept {
                prop_assert_eq!(shown, spec, "Advanced's fields write it back as it was");
            }
        }
    }
}
