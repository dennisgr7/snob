//! What a step, a problem and a refusal are called, in the words of the
//! system's language: the names the panel, the canvas, an automation's page
//! and the dialogs share, so a thing is the same thing everywhere. And the
//! days, months and moments every screen says alike: **"now" is always
//! handed in**, never read from a clock here, so a picture of a screen says
//! the same whatever the day it is taken.

use std::collections::BTreeSet;

use chrono::{Datelike as _, NaiveDate, NaiveDateTime, Timelike as _};
use snob_core::Epoch;
use snob_core::automation::{Automation, Problem, Read, Stage, Step, Unjoined, WATCH_TOML};
use snob_core::duration;
use snob_core::watch::schedule::{
    MonthDay, Nth, ScheduleError, Spec, Weekday, parse_month, parse_time,
};
use snob_engine::automations::said::Said;

use crate::backend::Next;
use crate::state::schedule::{DayPick, Shape, Time, Unit, Wrong, months_of};
use crate::ui::automations::Item;
use crate::ui::look::Look;
use crate::ui::now::on_the_clock;

/// What an automation is called in lists and titles: **`watch.toml`'s has
/// the name of the command that runs it**, whatever the file calls it.
pub fn automation_name(automation: &Automation, look: &Look) -> String {
    if automation.id == WATCH_TOML {
        look.tr("automation-watch-name")
    } else {
        automation.name.clone()
    }
}

/// What a step is.
pub fn step_name(step: &Step, look: &Look) -> String {
    look.tr(Item::of(step).id())
}

/// What a kind of step is for: the word before it in a flow.
pub fn stage_name(stage: Stage, look: &Look) -> String {
    look.tr(match stage {
        Stage::Trigger => "stage-trigger",
        Stage::Action => "stage-action",
        Stage::Filter => "stage-filter",
        Stage::Output => "stage-output",
    })
}

/// A schedule as a person says it: "Weekdays at 12:18", "The first Monday
/// of the month at 09:00", "Every 2 weeks, on Mondays". **One the choices
/// cannot show is said a part at a time**, in the app's words where they
/// can be read, and a cron expression as its line.
pub fn schedule_words(spec: &Spec, look: &Look) -> String {
    if spec.names_none() {
        return look.tr("node-no-schedule");
    }
    match Shape::of(spec) {
        Some(shape) => shape_words(&shape, look),
        None => schedule_parts(spec, look),
    }
}

fn shape_words(shape: &Shape, look: &Look) -> String {
    match shape {
        Shape::Interval { count, unit } => interval_words(*count, *unit, look),
        Shape::Daily { every_days, times } => {
            let at = at_words(times, look);
            if *every_days == 1 {
                look.tr_with("when-every-day", &[("at", &at)])
            } else {
                look.tr_with(
                    "when-every-days",
                    &[("count", &every_days.to_string()), ("at", &at)],
                )
            }
        }
        Shape::Weekly {
            every_weeks,
            weekdays,
            times,
        } => {
            let at = at_words(times, look);
            let days = weekdays_plural(weekdays, look);
            let count = every_weeks.to_string();
            match (*every_weeks, times.is_empty()) {
                (1, _) => weekly_words(weekdays, &at, look),
                (_, true) => {
                    look.tr_with("when-every-weeks", &[("count", &count), ("days", &days)])
                }
                (_, false) => look.tr_with(
                    "when-every-weeks-at",
                    &[("count", &count), ("days", &days), ("at", &at)],
                ),
            }
        }
        Shape::Monthly {
            every_months,
            first,
            day,
            times,
        } => month_day_words(day, &months_of(*every_months, *first), times, look),
        Shape::Yearly { months, day, times } => {
            let months: Vec<u32> = months.iter().copied().collect();
            month_day_words(day, &months, times, look)
        }
    }
}

/// Days of the week at times, every week: weekdays, weekends, every day,
/// or the days named.
fn weekly_words(weekdays: &BTreeSet<Weekday>, at: &str, look: &Look) -> String {
    use Weekday::{Fri, Mon, Sat, Sun, Thu, Tue, Wed};
    let id = if *weekdays == BTreeSet::from([Mon, Tue, Wed, Thu, Fri]) {
        "when-weekdays"
    } else if *weekdays == BTreeSet::from([Sat, Sun]) {
        "when-weekends"
    } else if weekdays.len() == 7 {
        "when-every-day"
    } else {
        let days = weekdays_plural(weekdays, look);
        return look.tr_with("when-on-days", &[("days", &days), ("at", at)]);
    };
    look.tr_with(id, &[("at", at)])
}

/// Days of a month, of every month or of the months named.
fn month_day_words(day: &DayPick, months: &[u32], times: &[Time], look: &Look) -> String {
    let at = at_words(times, look);
    let named: Vec<String> = months
        .iter()
        .map(|month| month_words(*month, look))
        .collect();
    let months = and_list(&named, look);
    let every = named.is_empty();
    match day {
        DayPick::Dates(dates) => {
            let said: Vec<String> = dates.iter().map(|date| date_words(*date, look)).collect();
            let count = dates.len() as u64;
            let dates = and_list(&said, look);
            if every {
                look.tr_count("when-dates", count, &[("dates", &dates), ("at", &at)])
            } else {
                look.tr_count(
                    "when-dates-in",
                    count,
                    &[("dates", &dates), ("months", &months), ("at", &at)],
                )
            }
        }
        DayPick::Nth(nth, weekday) => {
            let (nth, weekday) = (nth_words(*nth, look), weekday_words(*weekday, look));
            if every {
                look.tr_with(
                    "when-nth",
                    &[("nth", &nth), ("weekday", &weekday), ("at", &at)],
                )
            } else {
                look.tr_with(
                    "when-nth-in",
                    &[
                        ("nth", &nth),
                        ("weekday", &weekday),
                        ("months", &months),
                        ("at", &at),
                    ],
                )
            }
        }
    }
}

/// Every so often: "Every 6 hours", "Once a day".
fn interval_words(count: u64, unit: Unit, look: &Look) -> String {
    let id = match unit {
        Unit::Minutes => "when-minutes",
        Unit::Hours => "when-hours",
        Unit::Days => "when-days",
        Unit::Weeks => "when-weeks",
    };
    look.tr_count(id, count, &[])
}

/// "at 09:00 and 18:00". **One time is said with its hour**: some
/// languages say the hour one in the singular.
fn at_words(times: &[Time], look: &Look) -> String {
    let said: Vec<(String, u32)> = times
        .iter()
        .map(|(hour, minute)| clock(*hour, *minute, look))
        .collect();
    let hour = match said.as_slice() {
        [(_, hour)] => u64::from(*hour),
        _ => 0,
    };
    let times: Vec<String> = said.into_iter().map(|(words, _)| words).collect();
    look.tr_count("when-at", hour, &[("times", &and_list(&times, look))])
}

/// Days of the week as said of every one of them, Monday first:
/// "Mondays and Thursdays".
fn weekdays_plural(weekdays: &BTreeSet<Weekday>, look: &Look) -> String {
    let said: Vec<String> = MONDAY_FIRST
        .into_iter()
        .filter(|day| weekdays.contains(day))
        .map(|day| weekday_plural_words(day, look))
        .collect();
    and_list(&said, look)
}

/// The days of the week in the order a week is read in.
pub const MONDAY_FIRST: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

/// A date of the month as a day is named by it: "15th". The four ids are
/// CLDR's ordinal categories, which English tells apart and a language
/// that does not says alike.
fn date_words(date: u32, look: &Look) -> String {
    let id = match (date % 10, date % 100) {
        (1, tens) if tens != 11 => "day-nth-one",
        (2, tens) if tens != 12 => "day-nth-two",
        (3, tens) if tens != 13 => "day-nth-few",
        _ => "day-nth-other",
    };
    look.tr_with(id, &[("day", &date.to_string())])
}

/// A schedule the choices cannot show, a part at a time: "Every 6 hours ·
/// On Mon and Thu", in the app's words where a part can be read and as it
/// was written where it cannot.
fn schedule_parts(spec: &Spec, look: &Look) -> String {
    let each = |words: &[String], said: &dyn Fn(&str) -> Option<String>| -> String {
        let all: Vec<String> = words
            .iter()
            .map(|word| said(word).unwrap_or_else(|| word.clone()))
            .collect();
        and_list(&all, look)
    };
    let mut parts = Vec::new();
    if let Some(every) = spec.every {
        parts.push(match Unit::of(every) {
            Some((count, unit)) => interval_words(count, unit, look),
            None => look.tr_with("node-every", &[("every", &duration::format(every))]),
        });
    }
    if !spec.on.is_empty() {
        let days = each(&spec.on, &|day| {
            Weekday::parse(day).map(|day| weekday_short_words(day, look))
        });
        parts.push(look.tr_with("node-on", &[("days", &days)]));
    }
    if !spec.days.is_empty() {
        let days = each(&spec.days, &|day| match MonthDay::parse(day)? {
            MonthDay::Date(date) => Some(date_words(date, look)),
            MonthDay::Nth(nth, weekday) => Some(look.tr_with(
                "day-nth-weekday",
                &[
                    ("nth", &nth_words(nth, look)),
                    ("weekday", &weekday_words(weekday, look)),
                ],
            )),
        });
        parts.push(look.tr_with("node-days", &[("days", &days)]));
    }
    if !spec.months.is_empty() {
        let months = each(&spec.months, &|month| {
            parse_month(month).map(|month| month_words(month, look))
        });
        parts.push(look.tr_with("node-months", &[("months", &months)]));
    }
    if !spec.at.is_empty() {
        let times = each(&spec.at, &|time| {
            parse_time(time)
                .ok()
                .map(|(hour, minute)| clock(hour, minute, look).0)
        });
        parts.push(look.tr_with("node-at", &[("times", &times)]));
    }
    if let Some(cron) = &spec.cron {
        parts.push(look.tr_with("node-cron", &[("expression", cron)]));
    }
    parts.join(" \u{b7} ")
}

/// Why the engine refuses a schedule, in the app's words. **Never its own
/// sentence**, which is English: each of its reasons has a message here,
/// and one it adds later is a build that fails until it has one.
pub fn schedule_wrong_words(why: &ScheduleError, look: &Look) -> String {
    match why {
        ScheduleError::Empty => look.tr("schedule-wrong-empty"),
        ScheduleError::TooOften { minutes, .. } => look.tr_with(
            "schedule-wrong-too-often",
            &[("minutes", &minutes.to_string())],
        ),
        ScheduleError::TooRare { days, .. } => {
            look.tr_with("schedule-wrong-too-rare", &[("days", &days.to_string())])
        }
        ScheduleError::Unreadable(_) => look.tr("schedule-wrong-unreadable"),
    }
}

/// What keeps a schedule's dialog from saving, in the app's words.
pub fn wrong_words(wrong: &Wrong, look: &Look) -> String {
    match wrong {
        Wrong::NoTime => look.tr("schedule-wrong-no-time"),
        Wrong::Time(text) => look.tr_with("schedule-wrong-time", &[("text", text)]),
        Wrong::OneTime => look.tr("schedule-wrong-one-time"),
        Wrong::NoWeekday => look.tr("schedule-wrong-no-weekday"),
        Wrong::OneWeekday => look.tr("schedule-wrong-one-weekday"),
        Wrong::NoDate => look.tr("schedule-wrong-no-date"),
        Wrong::NoMonth => look.tr("schedule-wrong-no-month"),
        Wrong::Count(text) => look.tr_with("schedule-wrong-count", &[("text", text)]),
        Wrong::Duration(text) => look.tr_with("schedule-wrong-duration", &[("text", text)]),
        Wrong::Weekday(text) => look.tr_with("schedule-wrong-weekday", &[("text", text)]),
        Wrong::Month(text) => look.tr_with("schedule-wrong-month", &[("text", text)]),
        Wrong::MonthDay(text) => look.tr_with("schedule-wrong-month-day", &[("text", text)]),
        Wrong::CronAndCalendar => look.tr("schedule-wrong-cron-and-calendar"),
        Wrong::DaysAndWeekdays => look.tr("schedule-wrong-days-and-weekdays"),
        Wrong::Cron(text) => look.tr_with("schedule-wrong-cron", &[("text", text)]),
        Wrong::Never => look.tr("schedule-wrong-never"),
        Wrong::Engine(why) => schedule_wrong_words(why, look),
    }
}

/// When an automation runs next, as a person says it against `now`:
/// "Today at 09:00", "Tomorrow at 09:00", a date; "By hand" when only
/// somebody running it starts it.
pub fn next_run_words(next: &Next, now: Epoch, look: &Look) -> String {
    match next {
        Next::Now => look.tr("next-run-now"),
        Next::At(at) => moment_words(*at, now, look),
        Next::ByHand => look.tr("automation-card-by-hand"),
        Next::Never => look.tr("next-run-never"),
    }
}

/// What a step says of itself beyond its name, when it says anything.
pub fn step_detail(step: &Step, look: &Look) -> Option<String> {
    match step {
        Step::Schedule { when } => Some(schedule_words(when, look)),
        Step::Manual {} => Some(look.tr("node-manual")),
        Step::Read { account, .. } => Some(match account {
            None => look.tr("node-no-account"),
            Some(account) => match account.name() {
                Some(name) => format!("@{name}"),
                None => look.tr("node-own-account"),
            },
        }),
        Step::Filter { .. } => None,
        Step::Webhook(webhook) => Some(if webhook.url.trim().is_empty() {
            look.tr("node-no-address")
        } else {
            webhook.url.clone()
        }),
    }
}

/// Why an arrow was not drawn.
pub fn unjoined_words(why: Unjoined, look: &Look) -> String {
    look.tr(match why {
        Unjoined::Missing => "unjoined-missing",
        Unjoined::Itself => "unjoined-itself",
        Unjoined::Already => "unjoined-already",
        Unjoined::WrongWay => "unjoined-wrong-way",
        Unjoined::Circle => "unjoined-circle",
    })
}

/// What keeps an automation from running.
pub fn problem_words(problem: &Problem, look: &Look) -> String {
    match problem {
        Problem::NoName => look.tr("problem-no-name"),
        Problem::NoTrigger => look.tr("problem-no-trigger"),
        Problem::NoAction => look.tr("problem-no-action"),
        Problem::Schedule { why, .. } => look.tr_with(
            "problem-schedule",
            &[("why", &schedule_wrong_words(why, look))],
        ),
        Problem::NoAccount { .. } => look.tr("problem-no-account"),
        Problem::BadName { name, .. } => look.tr_with("problem-bad-name", &[("name", name)]),
        Problem::NoConsent { name, .. } => look.tr_with("problem-no-consent", &[("name", name)]),
        Problem::OwnAccount { .. } => look.tr("problem-own-account"),
        Problem::RelativeDir { .. } => look.tr("problem-relative-dir"),
        Problem::NoAddress { .. } => look.tr("problem-no-address"),
        Problem::TwoWebhooks { .. } => look.tr("problem-two-webhooks"),
        Problem::Taken { name, what, by, .. } => look.tr_with(
            "problem-taken",
            &[("name", name), ("what", what), ("by", by)],
        ),
        Problem::NotInWatchToml { .. } => look.tr("problem-not-in-watch-toml"),
    }
}

/// How a run came out, in a word.
pub fn outcome_words(outcome: Option<&str>, look: &Look) -> String {
    look.tr(match outcome {
        Some("ok") => "run-ok",
        Some("error") => "run-error",
        Some("rate_limited") => "run-rate-limited",
        Some("no_session") => "run-no-session",
        Some("challenge") => "run-challenge",
        Some("interrupted") => "run-interrupted",
        Some(_) => "run-error",
        None => "run-unfinished",
    })
}

/// A day as a person names it against today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Day {
    Today,
    Yesterday,
    Tomorrow,
    /// One of the five days before yesterday, named by its weekday.
    ThisWeek {
        weekday: Weekday,
    },
    ThisYear {
        month: u32,
        day: u32,
    },
    Other {
        year: i32,
        month: u32,
        day: u32,
    },
}

/// Which day `at` is, said against `now`.
pub fn day_of(at: Epoch, now: Epoch) -> Day {
    day_between(on_the_clock(at).date(), on_the_clock(now).date())
}

fn day_between(at: NaiveDate, today: NaiveDate) -> Day {
    match (today - at).num_days() {
        0 => Day::Today,
        1 => Day::Yesterday,
        -1 => Day::Tomorrow,
        2..=6 => Day::ThisWeek {
            weekday: Weekday::ALL[at.weekday().num_days_from_sunday() as usize],
        },
        _ if at.year() == today.year() => Day::ThisYear {
            month: at.month(),
            day: at.day(),
        },
        _ => Day::Other {
            year: at.year(),
            month: at.month(),
            day: at.day(),
        },
    }
}

/// A day in words as said inside a sentence: "today", "Monday", "March 5",
/// "March 5, 2025", lower case where the language writes it so. A caller
/// that shows it alone capitalizes it.
pub fn day_words(day: &Day, look: &Look) -> String {
    match *day {
        Day::Today => look.tr("day-today"),
        Day::Yesterday => look.tr("day-yesterday"),
        Day::Tomorrow => look.tr("day-tomorrow"),
        Day::ThisWeek { weekday } => weekday_words(weekday, look),
        Day::ThisYear { month, day } => look.tr_with(
            "date-this-year",
            &[
                ("month", &month_words(month, look)),
                ("day", &day.to_string()),
            ],
        ),
        Day::Other { year, month, day } => look.tr_with(
            "date-other-year",
            &[
                ("month", &month_words(month, look)),
                ("day", &day.to_string()),
                ("year", &year.to_string()),
            ],
        ),
    }
}

/// The time of day `at` in words, in 12 or 24 hours as the language's file
/// says (`clock-hours`).
pub fn clock_words(at: Epoch, look: &Look) -> String {
    let at = on_the_clock(at);
    clock(at.hour(), at.minute(), look).0
}

/// A time of day, and its hour as it is said: some languages say the hour
/// one in the singular.
fn clock(hour: u32, minute: u32, look: &Look) -> (String, u32) {
    let minute = format!("{minute:02}");
    if look.tr("clock-hours") == "12" {
        let (half, said) = match hour {
            0 => ("clock-am", 12),
            1..=11 => ("clock-am", hour),
            12 => ("clock-pm", 12),
            _ => ("clock-pm", hour - 12),
        };
        let words = look.tr_with(half, &[("hour", &said.to_string()), ("minute", &minute)]);
        (words, said)
    } else {
        let words = look.tr_with(
            "clock-24",
            &[("hour", &format!("{hour:02}")), ("minute", &minute)],
        );
        (words, hour)
    }
}

/// A moment as a person says it against `now`: "Today at 15:05",
/// "Monday at 09:30", "March 5, 2025 at 23:59".
pub fn moment_words(at: Epoch, now: Epoch, look: &Look) -> String {
    moment_between(on_the_clock(at), on_the_clock(now), look)
}

fn moment_between(at: NaiveDateTime, now: NaiveDateTime, look: &Look) -> String {
    let day = day_between(at.date(), now.date());
    let (time, hour) = clock(at.hour(), at.minute(), look);
    let hour = u64::from(hour);
    match day {
        Day::Today => look.tr_count("moment-today", hour, &[("time", &time)]),
        Day::Yesterday => look.tr_count("moment-yesterday", hour, &[("time", &time)]),
        Day::Tomorrow => look.tr_count("moment-tomorrow", hour, &[("time", &time)]),
        Day::ThisWeek { weekday } => look.tr_count(
            "moment-this-week",
            hour,
            &[("time", &time), ("weekday", &weekday_words(weekday, look))],
        ),
        Day::ThisYear { .. } => look.tr_count(
            "moment-this-year",
            hour,
            &[("time", &time), ("date", &day_words(&day, look))],
        ),
        Day::Other { .. } => look.tr_count(
            "moment-other-year",
            hour,
            &[("time", &time), ("date", &day_words(&day, look))],
        ),
    }
}

/// A month, 1 to 12, by its whole name.
pub fn month_words(month: u32, look: &Look) -> String {
    look.tr(match month {
        1 => "month-jan",
        2 => "month-feb",
        3 => "month-mar",
        4 => "month-apr",
        5 => "month-may",
        6 => "month-jun",
        7 => "month-jul",
        8 => "month-aug",
        9 => "month-sep",
        10 => "month-oct",
        11 => "month-nov",
        12 => "month-dec",
        _ => return month.to_string(),
    })
}

/// A month, 1 to 12, by its short name.
pub fn month_short_words(month: u32, look: &Look) -> String {
    look.tr(match month {
        1 => "month-short-jan",
        2 => "month-short-feb",
        3 => "month-short-mar",
        4 => "month-short-apr",
        5 => "month-short-may",
        6 => "month-short-jun",
        7 => "month-short-jul",
        8 => "month-short-aug",
        9 => "month-short-sep",
        10 => "month-short-oct",
        11 => "month-short-nov",
        12 => "month-short-dec",
        _ => return month.to_string(),
    })
}

/// A day of the week by its whole name.
pub fn weekday_words(weekday: Weekday, look: &Look) -> String {
    look.tr(match weekday {
        Weekday::Mon => "weekday-mon",
        Weekday::Tue => "weekday-tue",
        Weekday::Wed => "weekday-wed",
        Weekday::Thu => "weekday-thu",
        Weekday::Fri => "weekday-fri",
        Weekday::Sat => "weekday-sat",
        Weekday::Sun => "weekday-sun",
    })
}

/// A day of the week as said of every one of them: "Mondays".
pub fn weekday_plural_words(weekday: Weekday, look: &Look) -> String {
    look.tr(match weekday {
        Weekday::Mon => "weekday-plural-mon",
        Weekday::Tue => "weekday-plural-tue",
        Weekday::Wed => "weekday-plural-wed",
        Weekday::Thu => "weekday-plural-thu",
        Weekday::Fri => "weekday-plural-fri",
        Weekday::Sat => "weekday-plural-sat",
        Weekday::Sun => "weekday-plural-sun",
    })
}

/// A day of the week by its short name.
pub fn weekday_short_words(weekday: Weekday, look: &Look) -> String {
    look.tr(match weekday {
        Weekday::Mon => "weekday-short-mon",
        Weekday::Tue => "weekday-short-tue",
        Weekday::Wed => "weekday-short-wed",
        Weekday::Thu => "weekday-short-thu",
        Weekday::Fri => "weekday-short-fri",
        Weekday::Sat => "weekday-short-sat",
        Weekday::Sun => "weekday-short-sun",
    })
}

/// Which one of a month's weekdays: "first", "last".
pub fn nth_words(nth: Nth, look: &Look) -> String {
    look.tr(match nth {
        Nth::First => "nth-first",
        Nth::Second => "nth-second",
        Nth::Third => "nth-third",
        Nth::Fourth => "nth-fourth",
        Nth::Last => "nth-last",
    })
}

/// Several things in one phrase: "a, b and c".
pub fn and_list(items: &[String], look: &Look) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => {
            look.tr_with("list-and", &[("first", &first.join(", ")), ("last", last)])
        }
    }
}

/// What a card of the list says an automation does: its steps after the
/// triggers, a read with whose account it reads, joined by arrows.
pub fn card_steps_words(automation: &Automation, look: &Look) -> String {
    let steps: Vec<String> = crate::state::automations::card_steps(automation)
        .into_iter()
        .map(|node| {
            let name = step_name(&node.step, look);
            match (&node.step, step_detail(&node.step, look)) {
                (Step::Read { .. }, Some(detail)) => format!("{name} ({detail})"),
                _ => name,
            }
        })
        .collect();
    if steps.is_empty() {
        return look.tr("automation-card-no-steps");
    }
    steps.join(" \u{2192} ")
}

/// When a card says an automation runs: its schedules, "By hand", or that
/// it has none.
pub fn card_when_words(automation: &Automation, look: &Look) -> String {
    use crate::state::automations::{Runs, card_runs};
    match card_runs(automation) {
        Runs::On(specs) => specs
            .into_iter()
            .map(|spec| schedule_words(spec, look))
            .collect::<Vec<_>>()
            .join(" \u{b7} "),
        Runs::ByHand => look.tr("automation-card-by-hand"),
        Runs::Never => look.tr("node-no-schedule"),
    }
}

/// What a run found, in the app's language: a line a thing, its accounts
/// named under it. **`None` when anything in it is not known here**, or it
/// says nothing, and the run's own sentences are said instead.
pub fn found_lines(found: &[Said], now: Epoch, look: &Look) -> Option<Vec<String>> {
    let mut lines = Vec::new();
    for said in found {
        lines.extend(said_lines(said, now, look)?);
    }
    (!lines.is_empty()).then_some(lines)
}

/// Whose account a finding is about.
fn whose(account: Option<&str>, own: bool, look: &Look) -> String {
    match account {
        Some(name) if !own => format!("@{name}"),
        _ => look.tr("node-own-account"),
    }
}

/// What stands before an account that went: a minus sign.
const GONE: &str = "\u{2212} ";

/// The accounts named under a line by their usernames, `sign` before
/// each, and how many more there were than are named.
fn named(sign: &str, names: &[String], count: usize, lines: &mut Vec<String>, look: &Look) {
    for name in names {
        lines.push(format!("  {sign}@{name}"));
    }
    if count > names.len() {
        let more = (count - names.len()) as u64;
        lines.push(format!("  {}", look.tr_count("run-said-more", more, &[])));
    }
}

fn said_lines(said: &Said, now: Epoch, look: &Look) -> Option<Vec<String>> {
    let mut lines = Vec::new();
    match said {
        Said::Found {
            what,
            account,
            count,
            changes,
            stored_since,
            picture,
            names,
            ..
        } => {
            let read = Read::all().into_iter().find(|read| read.as_str() == what)?;
            let what = look.tr(Item::Read(read).id());
            let whose = whose(account.as_deref(), false, look);
            let values = [("what", what.as_str()), ("whose", whose.as_str())];
            if let Some(picture) = picture {
                lines.push(look.tr_with(
                    match (picture.saved, picture.new) {
                        (true, true) => "run-said-picture-new",
                        (true, false) => "run-said-picture-same",
                        (false, _) => "run-said-picture-read",
                    },
                    &values,
                ));
                return Some(lines);
            }
            match changes {
                Some(changes) if changes.baseline => {
                    lines.push(look.tr_count("run-said-baseline", *count as u64, &values));
                }
                Some(changes) => {
                    let since = changes
                        .since
                        .map(|since| moment_words(since, now, look))
                        .unwrap_or_default();
                    let (added, removed) = (changes.added.to_string(), changes.removed.to_string());
                    lines.push(look.tr_with(
                        "run-said-since",
                        &[
                            ("what", &what),
                            ("whose", &whose),
                            ("added", &added),
                            ("removed", &removed),
                            ("since", &since),
                        ],
                    ));
                    named("+ ", &changes.added_names, changes.added, &mut lines, look);
                    named(
                        GONE,
                        &changes.removed_names,
                        changes.removed,
                        &mut lines,
                        look,
                    );
                }
                None => {
                    lines.push(look.tr_count("run-said-found", *count as u64, &values));
                    named("", names, *count, &mut lines, look);
                }
            }
            if let Some(at) = stored_since {
                let when = moment_words(*at, now, look);
                lines.push(look.tr_with("run-said-stored", &[("when", &when)]));
            }
        }
        Said::Tick {
            account,
            pk,
            own,
            refused,
            nothing_stored,
            followers,
            following,
            renamed,
            posts,
        } => {
            let whose = match account {
                None if !own => look.tr_with("run-said-account", &[("pk", &pk.to_string())]),
                _ => whose(account.as_deref(), *own, look),
            };
            if *refused {
                lines.push(look.tr_with("run-said-refused", &[("whose", &whose)]));
                return Some(lines);
            }
            if *nothing_stored {
                lines.push(look.tr_with("run-said-nothing-stored", &[("whose", &whose)]));
                return Some(lines);
            }
            for (list, said) in [
                ("run-list-followers", followers),
                ("run-list-following", following),
            ] {
                let Some(said) = said else { continue };
                let list = look.tr(list);
                let total = said.total as u64;
                let values = [("list", list.as_str()), ("whose", whose.as_str())];
                match said.basis.as_str() {
                    "baseline" => {
                        lines.push(look.tr_count("run-said-list-baseline", total, &values));
                    }
                    "unchanged" => {
                        lines.push(look.tr_count("run-said-list-same", total, &values));
                    }
                    "compare" => {
                        let (added, removed) = (said.added.to_string(), said.removed.to_string());
                        lines.push(look.tr_count(
                            "run-said-list-changed",
                            total,
                            &[
                                ("list", &list),
                                ("whose", &whose),
                                ("added", &added),
                                ("removed", &removed),
                            ],
                        ));
                        named("+ ", &said.added_names, said.added, &mut lines, look);
                        named(GONE, &said.removed_names, said.removed, &mut lines, look);
                    }
                    _ => return None,
                }
            }
            if *renamed > 0 {
                lines.push(look.tr_count("run-said-renamed", *renamed as u64, &[]));
            }
            if let Some(posts) = posts {
                let values = [("whose", whose.as_str())];
                match (posts.skipped.as_deref(), posts.basis.as_deref()) {
                    (Some("read_recently"), _) => {
                        lines.push(look.tr_with("run-said-posts-recent", &values));
                    }
                    (Some(_), _) => lines.push(look.tr_with("run-said-posts-skipped", &values)),
                    (None, Some("baseline")) => {
                        lines.push(look.tr_with("run-said-posts-baseline", &values));
                    }
                    (None, Some("compared")) if posts.new == 0 => {
                        lines.push(look.tr_with("run-said-posts-none", &values));
                    }
                    (None, Some("compared")) => {
                        let new = posts.new as u64;
                        lines.push(look.tr_count("run-said-posts-new", new, &values));
                    }
                    (None, None) => {}
                    (None, Some(_)) => return None,
                }
                if !posts.complete {
                    lines.push(look.tr("run-said-posts-incomplete"));
                }
                if posts.saved > 0 {
                    lines.push(look.tr_count("run-said-saved", posts.saved as u64, &[]));
                }
            }
        }
        Said::Failed { code } => {
            let outcome = outcome_words(Some(code.as_str()), look);
            lines.push(look.tr_with("run-said-failed", &[("outcome", &outcome)]));
        }
        Said::CannotRun => lines.push(look.tr("run-said-cannot-run")),
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Settings;
    use snob_engine::automations::said::{ListSaid, PostsSaid};

    fn speaking(locale: &str) -> Look {
        Look::of(
            Settings {
                locale: locale.to_string(),
                ..Settings::default()
            },
            false,
        )
    }

    fn at(day: &str, time: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(&format!("{day} {time}"), "%Y-%m-%d %H:%M").unwrap()
    }

    fn list(basis: &str, total: usize, added: &[&str], removed: &[&str]) -> ListSaid {
        let names = |names: &[&str]| names.iter().map(|name| (*name).to_string()).collect();
        ListSaid {
            basis: basis.to_string(),
            total,
            added: added.len(),
            removed: removed.len(),
            added_names: names(added),
            removed_names: names(removed),
        }
    }

    fn tick(followers: ListSaid, posts: Option<PostsSaid>) -> Said {
        Said::Tick {
            account: Some("someone".to_string()),
            pk: snob_core::Pk::new(7),
            own: false,
            refused: false,
            nothing_stored: false,
            followers: Some(followers),
            following: None,
            renamed: 1,
            posts,
        }
    }

    #[test]
    fn what_a_run_found_is_said_in_both_languages_with_the_names_it_kept() {
        let now = Epoch::new(1_790_000_000);
        let mut followers = list("compare", 120, &["came"], &["went"]);
        followers.added = 3;
        let posts = PostsSaid {
            basis: Some("compared".to_string()),
            skipped: None,
            new: 1,
            complete: false,
            saved: 2,
        };
        let found = [tick(followers, Some(posts)), Said::CannotRun];
        assert_eq!(
            found_lines(&found, now, &speaking("en")).unwrap(),
            [
                "Followers \u{b7} @someone: 3 came, 1 went, 120 accounts in all.",
                "  + @came",
                "  and 2 more",
                "  \u{2212} @went",
                "1 now goes by another name.",
                "Posts \u{b7} @someone: 1 new.",
                "Only the newest posts were read: an older one may have gone unreported.",
                "2 files saved.",
                "It could not run: something in its steps keeps it from running. Edit it to see what.",
            ]
        );
        let spanish = found_lines(&found, now, &speaking("es")).unwrap();
        assert_eq!(
            spanish[0],
            "Seguidores \u{b7} @someone: entraron 3 y salieron 1; 120 cuentas en total." // i18n-allow
        );
        assert_eq!(spanish[2], "  y 2 m\u{e1}s"); // i18n-allow
        assert_eq!(spanish[5], "Publicaciones \u{b7} @someone: 1 nueva."); // i18n-allow
    }

    #[test]
    fn what_an_action_found_is_said_with_its_count_and_whose_it_is() {
        let now = Epoch::new(1_790_000_000);
        let found = Said::Found {
            what: "fans".to_string(),
            account: None,
            count: 1,
            total: 4,
            changes: None,
            stored_since: Some(Epoch::new(1_790_000_000 - 86_400)),
            picture: None,
            names: vec!["only_one".to_string()],
        };
        assert_eq!(
            found_lines(&[found], now, &speaking("en")).unwrap(),
            [
                "Fans \u{b7} Your account: 1 account.",
                "  @only_one",
                "Worked out from lists read earlier (Yesterday at 14:13).",
            ]
        );
        let failed = Said::Failed {
            code: "rate_limited".to_string(),
        };
        assert_eq!(
            found_lines(&[failed], now, &speaking("en")).unwrap(),
            ["Part of the run: Paused by Instagram."]
        );
    }

    #[test]
    fn a_finding_this_build_does_not_know_or_nothing_found_is_said_in_the_runs_own_sentences() {
        let now = Epoch::new(1_790_000_000);
        let en = speaking("en");
        let unknown_action = Said::Found {
            what: "something_new".to_string(),
            account: None,
            count: 0,
            total: 0,
            changes: None,
            stored_since: None,
            picture: None,
            names: Vec::new(),
        };
        assert_eq!(found_lines(&[unknown_action], now, &en), None);
        let unknown_basis = tick(list("guessed", 1, &[], &[]), None);
        assert_eq!(found_lines(&[unknown_basis], now, &en), None);
        assert_eq!(found_lines(&[], now, &en), None, "nothing to say");
    }

    #[test]
    fn a_card_says_the_steps_after_its_triggers_and_when_it_runs() {
        use snob_core::automation::{Account, Only, Webhook};
        let en = speaking("en");
        let mut automation = Automation::new("a1", "Daily");
        assert_eq!(
            card_steps_words(&automation, &en),
            "Nothing to do yet. Edit it to add steps."
        );
        assert_eq!(card_when_words(&automation, &en), "No schedule yet");
        let manual = automation.insert(Step::Manual {}, (0, 0));
        let read = automation.insert(
            Step::Read {
                account: Some(Account::own(None)),
                what: Read::Lists,
            },
            (200, 0),
        );
        let filter = automation.insert(
            Step::Filter {
                only: Only::Changes,
            },
            (400, 0),
        );
        let hook = automation.insert(Step::Webhook(Webhook::default()), (600, 0));
        for (from, to) in [(manual, read), (read, filter), (filter, hook)] {
            automation.connect(from, to).unwrap();
        }
        assert_eq!(
            card_steps_words(&automation, &en),
            "Followers and following (Your account) \u{2192} Only what changed \u{2192} Send to a webhook"
        );
        assert_eq!(card_when_words(&automation, &en), "By hand");
        assert_eq!(card_when_words(&automation, &speaking("es")), "A mano");
    }

    #[test]
    fn a_moment_is_today_yesterday_a_weekday_this_week_or_a_date_in_both_languages() {
        // A Thursday.
        let now = at("2026-09-17", "10:00");
        let (en, es) = (speaking("en"), speaking("es"));
        for (moment, english, spanish) in [
            (
                at("2026-09-17", "15:05"),
                "Today at 15:05",
                "Hoy a las 15:05", // i18n-allow
            ),
            (
                at("2026-09-16", "01:05"),
                "Yesterday at 01:05",
                "Ayer a la 01:05", // i18n-allow
            ),
            (
                at("2026-09-14", "09:30"),
                "Monday at 09:30",
                "El lunes a las 09:30", // i18n-allow
            ),
            (
                at("2026-09-11", "00:00"),
                "Friday at 00:00",
                "El viernes a las 00:00", // i18n-allow
            ),
            (
                at("2026-09-10", "12:00"),
                "September 10 at 12:00",
                "El 10 de septiembre a las 12:00", // i18n-allow
            ),
            (
                at("2025-12-31", "23:59"),
                "December 31, 2025 at 23:59",
                "El 31 de diciembre de 2025 a las 23:59", // i18n-allow
            ),
        ] {
            assert_eq!(moment_between(moment, now, &en), english);
            assert_eq!(moment_between(moment, now, &es), spanish);
        }
    }

    #[test]
    fn a_day_is_named_against_the_day_handed_in() {
        let today = at("2026-01-02", "00:00").date();
        let day =
            |date: &str| day_between(NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(), today);
        assert_eq!(day("2026-01-02"), Day::Today);
        assert_eq!(day("2026-01-01"), Day::Yesterday);
        assert_eq!(
            day("2025-12-31"),
            Day::ThisWeek {
                weekday: Weekday::Wed
            }
        );
        assert_eq!(
            day("2025-12-26"),
            Day::Other {
                year: 2025,
                month: 12,
                day: 26
            },
            "a week ago is a date"
        );
        assert_eq!(
            day("2026-03-01"),
            Day::ThisYear { month: 3, day: 1 },
            "a day ahead is a date"
        );
        let wednesday = "mi\u{e9}rcoles";
        assert_eq!(day_words(&day("2025-12-31"), &speaking("es")), wednesday);
        assert_eq!(day_words(&day("2026-01-01"), &speaking("en")), "yesterday");
    }

    #[cfg(feature = "testing")]
    #[test]
    fn a_test_build_reads_moments_in_utc() {
        // 2026-09-17 at 23:30 and 2026-09-18 at 00:30, UTC.
        let (late, early) = (Epoch::new(1_789_687_800), Epoch::new(1_789_691_400));
        assert_eq!(day_of(late, early), Day::Yesterday);
        assert_eq!(clock_words(early, &speaking("es")), "00:30");
        assert_eq!(clock_words(early, &speaking("en")), "00:30");
        assert_eq!(
            moment_words(late, early, &speaking("en")),
            "Yesterday at 23:30"
        );
        assert_eq!(
            moment_words(late, early, &speaking("es")),
            "Ayer a las 23:30" // i18n-allow
        );
    }

    #[test]
    fn several_things_are_said_as_one_phrase() {
        let items = |all: &[&str]| all.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let (en, es) = (speaking("en"), speaking("es"));
        assert_eq!(and_list(&items(&[]), &en), "");
        assert_eq!(and_list(&items(&["Mon"]), &en), "Mon");
        assert_eq!(and_list(&items(&["Mon", "Thu"]), &en), "Mon and Thu");
        assert_eq!(
            and_list(&items(&["lun", "mar", "jue"]), &es), // i18n-allow
            "lun, mar y jue"                               // i18n-allow
        );
    }

    fn spec_of(change: impl FnOnce(&mut Spec)) -> Spec {
        let mut spec = Spec::default();
        change(&mut spec);
        spec
    }

    fn texts(all: &[&str]) -> Vec<String> {
        all.iter().map(|one| (*one).to_string()).collect()
    }

    fn days(n: u64) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs(n * 86_400))
    }

    #[test]
    fn a_schedule_reads_as_a_person_says_it_in_english_and_spanish() {
        let (en, es) = (speaking("en"), speaking("es"));
        for (spec, english, spanish) in [
            (
                spec_of(|s| {
                    s.on = texts(&["mon", "tue", "wed", "thu", "fri"]);
                    s.at = texts(&["12:18"]);
                }),
                "Weekdays at 12:18",
                "D\u{ed}as laborables a las 12:18", // i18n-allow
            ),
            (
                spec_of(|s| s.at = texts(&["18:00", "09:00"])),
                "Every day at 09:00 and 18:00",
                "Todos los d\u{ed}as a las 09:00 y 18:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.days = texts(&["first mon"]);
                    s.at = texts(&["09:00"]);
                }),
                "The first Monday of the month at 09:00",
                "El primer lunes de cada mes a las 09:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.days = texts(&["1", "15"]);
                    s.months = texts(&["mar", "sep"]);
                    s.at = texts(&["08:00"]);
                }),
                "On the 1st and 15th of March and September at 08:00",
                "Los d\u{ed}as 1 y 15 de marzo y septiembre a las 08:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.days = texts(&["last fri"]);
                    s.months = texts(&["jan", "apr", "jul", "oct"]);
                    s.at = texts(&["17:45"]);
                }),
                "The last Friday of January, April, July and October at 17:45",
                "El \u{fa}ltimo viernes de enero, abril, julio y octubre a las 17:45", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.every = days(14);
                    s.on = texts(&["mon"]);
                }),
                "Every 2 weeks, on Mondays",
                "Cada 2 semanas, los lunes", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.every = days(21);
                    s.on = texts(&["sat"]);
                    s.at = texts(&["10:30"]);
                }),
                "Every 3 weeks, on Saturdays at 10:30",
                "Cada 3 semanas, los s\u{e1}bados a las 10:30", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.on = texts(&["sun", "sat"]);
                    s.at = texts(&["10:00"]);
                }),
                "Weekends at 10:00",
                "Fines de semana a las 10:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.on = texts(&["thu", "mon"]);
                    s.at = texts(&["07:00"]);
                }),
                "On Mondays and Thursdays at 07:00",
                "Los lunes y jueves a las 07:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.every = days(3);
                    s.at = texts(&["06:00"]);
                }),
                "Every 3 days at 06:00",
                "Cada 3 d\u{ed}as a las 06:00", // i18n-allow
            ),
            (
                spec_of(|s| s.cron = Some("0 9 * * 1".to_string())),
                "Cron 0 9 * * 1",
                "Cron 0 9 * * 1",
            ),
            (
                spec_of(|s| {
                    s.months = texts(&["dec"]);
                    s.at = texts(&["09:00"]);
                }),
                "In December \u{b7} At 09:00",
                "En diciembre \u{b7} A las 09:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.every = days(2);
                    s.at = texts(&["09:00", "18:00"]);
                }),
                "Every 2 days \u{b7} At 09:00 and 18:00",
                "Cada 2 d\u{ed}as \u{b7} A las 09:00 y 18:00", // i18n-allow
            ),
            (
                Spec::default(),
                "No schedule yet",
                "A\u{fa}n sin programaci\u{f3}n", // i18n-allow
            ),
        ] {
            assert_eq!(schedule_words(&spec, &en), english, "{spec:?}");
            assert_eq!(schedule_words(&spec, &es), spanish, "{spec:?}");
        }
    }

    #[test]
    fn one_of_a_thing_is_said_in_the_singular_and_several_in_the_plural() {
        let (en, es) = (speaking("en"), speaking("es"));
        let every = |secs: u64| spec_of(|s| s.every = Some(std::time::Duration::from_secs(secs)));
        for (spec, english, spanish) in [
            (every(3_600), "Every hour", "Cada hora"),
            (every(6 * 3_600), "Every 6 hours", "Cada 6 horas"),
            (every(30 * 60), "Every 30 minutes", "Cada 30 minutos"),
            (every(86_400), "Once a day", "Una vez al d\u{ed}a"), // i18n-allow
            (every(2 * 86_400), "Every 2 days", "Cada 2 d\u{ed}as"), // i18n-allow
            (every(7 * 86_400), "Once a week", "Una vez a la semana"), // i18n-allow
            (
                spec_of(|s| {
                    s.days = texts(&["15"]);
                    s.at = texts(&["09:00"]);
                }),
                "On the 15th of every month at 09:00",
                "El d\u{ed}a 15 de cada mes a las 09:00", // i18n-allow
            ),
            (
                spec_of(|s| {
                    s.days = texts(&["1", "2", "3", "11", "12", "13", "21", "22", "23", "31"]);
                    s.at = texts(&["09:00"]);
                }),
                "On the 1st, 2nd, 3rd, 11th, 12th, 13th, 21st, 22nd, 23rd and 31st of every month at 09:00",
                "Los d\u{ed}as 1, 2, 3, 11, 12, 13, 21, 22, 23 y 31 de cada mes a las 09:00", // i18n-allow
            ),
            (
                spec_of(|s| s.at = texts(&["01:30"])),
                "Every day at 01:30",
                "Todos los d\u{ed}as a la 01:30", // i18n-allow
            ),
        ] {
            assert_eq!(schedule_words(&spec, &en), english, "{spec:?}");
            assert_eq!(schedule_words(&spec, &es), spanish, "{spec:?}");
        }
    }

    #[test]
    fn the_engines_reasons_are_said_in_the_apps_words_never_its_own() {
        for look in [speaking("en"), speaking("es")] {
            for why in [
                ScheduleError::Empty,
                ScheduleError::TooOften {
                    interval: "5m".to_string(),
                    minutes: 15,
                },
                ScheduleError::TooRare {
                    interval: "400d".to_string(),
                    days: 366,
                },
                ScheduleError::Unreadable("\"x\" is not a day (try mon, thu)".to_string()),
            ] {
                let said = schedule_wrong_words(&why, &look);
                assert!(!said.starts_with("schedule-wrong"), "{said}");
                assert!(!said.contains(&why.to_string()), "{said}");
                assert!(!said.contains("try"), "the engine's English: {said}");
            }
        }
        let problem = Problem::Schedule {
            node: 1,
            why: ScheduleError::TooOften {
                interval: "5m".to_string(),
                minutes: 15,
            },
        };
        assert_eq!(
            problem_words(&problem, &speaking("es")),
            "La programaci\u{f3}n no sirve: Dos ejecuciones no pueden estar a menos de 15 minutos: la segunda no ver\u{ed}a nada que no viera la primera." // i18n-allow
        );
    }

    #[test]
    fn everything_a_schedules_dialog_refuses_has_its_words_in_both_languages() {
        let text = || "x".to_string();
        for look in [speaking("en"), speaking("es")] {
            for wrong in [
                Wrong::NoTime,
                Wrong::Time(text()),
                Wrong::OneTime,
                Wrong::NoWeekday,
                Wrong::OneWeekday,
                Wrong::NoDate,
                Wrong::NoMonth,
                Wrong::Count(text()),
                Wrong::Duration(text()),
                Wrong::Weekday(text()),
                Wrong::Month(text()),
                Wrong::MonthDay(text()),
                Wrong::CronAndCalendar,
                Wrong::DaysAndWeekdays,
                Wrong::Cron(text()),
                Wrong::Never,
                Wrong::Engine(ScheduleError::Empty),
            ] {
                let said = wrong_words(&wrong, &look);
                assert!(!said.starts_with("schedule-wrong"), "{wrong:?}: {said}");
            }
            for weekday in Weekday::ALL {
                assert!(!weekday_plural_words(weekday, &look).starts_with("weekday-"));
            }
            assert!(!next_run_words(&Next::Now, Epoch::new(0), &look).starts_with("next-"));
            assert!(!next_run_words(&Next::Never, Epoch::new(0), &look).starts_with("next-"));
        }
    }

    #[test]
    fn tomorrow_is_said_as_tomorrow() {
        let now = at("2026-09-17", "10:00");
        assert_eq!(
            moment_between(at("2026-09-18", "09:00"), now, &speaking("en")),
            "Tomorrow at 09:00"
        );
        assert_eq!(
            moment_between(at("2026-09-18", "01:15"), now, &speaking("es")),
            "Ma\u{f1}ana a la 01:15" // i18n-allow
        );
    }

    #[cfg(feature = "testing")]
    #[test]
    fn the_next_run_is_today_tomorrow_a_date_or_by_hand() {
        // 2026-09-17 at 10:00, UTC.
        let now = Epoch::new(1_789_639_200);
        let hours = |n: i64| Epoch::new(now.get() + n * 3_600);
        let (en, es) = (speaking("en"), speaking("es"));
        assert_eq!(
            next_run_words(&Next::At(hours(2)), now, &en),
            "Today at 12:00"
        );
        assert_eq!(
            next_run_words(&Next::At(hours(23)), now, &en),
            "Tomorrow at 09:00"
        );
        assert_eq!(
            next_run_words(&Next::At(hours(23)), now, &es),
            "Ma\u{f1}ana a las 09:00" // i18n-allow
        );
        assert_eq!(
            next_run_words(&Next::At(hours(24 * 30)), now, &en),
            "October 17 at 10:00"
        );
        assert_eq!(next_run_words(&Next::ByHand, now, &en), "By hand");
        assert_eq!(next_run_words(&Next::ByHand, now, &es), "A mano");
    }

    #[test]
    fn every_month_weekday_and_ordinal_has_its_words() {
        for look in [speaking("en"), speaking("es")] {
            for month in 1..=12 {
                assert!(!month_words(month, &look).starts_with("month-"));
                assert!(!month_short_words(month, &look).starts_with("month-"));
            }
            for weekday in Weekday::ALL {
                assert!(!weekday_words(weekday, &look).starts_with("weekday-"));
                assert!(!weekday_short_words(weekday, &look).starts_with("weekday-"));
            }
            for nth in Nth::ALL {
                assert!(!nth_words(nth, &look).starts_with("nth-"));
            }
            assert_eq!(month_words(13, &look), "13");
            for date in 1..=31 {
                assert!(!date_words(date, &look).starts_with("day-nth"), "{date}");
            }
            for unit in Unit::ALL {
                for count in [1, 2] {
                    assert!(!interval_words(count, unit, &look).starts_with("when-"));
                }
            }
            use Weekday::{Fri, Mon, Sat, Sun, Thu, Tue, Wed};
            for days in [
                BTreeSet::from([Mon, Tue, Wed, Thu, Fri]),
                BTreeSet::from([Sat, Sun]),
                BTreeSet::from(Weekday::ALL),
                BTreeSet::from([Mon]),
            ] {
                assert!(!weekly_words(&days, "09:00", &look).starts_with("when-"));
            }
        }
    }
}
