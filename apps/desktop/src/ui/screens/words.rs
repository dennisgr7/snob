//! What a step, a problem and a refusal are called, in the words of the
//! system's language: the names the panel, the canvas, an automation's page
//! and the dialogs share, so a thing is the same thing everywhere. And the
//! days, months and moments every screen says alike: **"now" is always
//! handed in**, never read from a clock here, so a picture of a screen says
//! the same whatever the day it is taken.

use chrono::{Datelike as _, NaiveDate, NaiveDateTime, Timelike as _};
use snob_core::Epoch;
use snob_core::automation::{Automation, Problem, Stage, Step, Unjoined, WATCH_TOML};
use snob_core::duration;
use snob_core::watch::schedule::{Nth, Spec, Weekday};

use crate::ui::automations::Item;
use crate::ui::look::Look;

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

/// A schedule in a line: `every 6h · mon, thu · at 09:00`.
pub fn schedule_words(spec: &Spec, look: &Look) -> String {
    let mut parts = Vec::new();
    if let Some(every) = spec.every {
        parts.push(look.tr_with("node-every", &[("every", &duration::format(every))]));
    }
    if !spec.on.is_empty() {
        parts.push(look.tr_with("node-on", &[("days", &spec.on.join(", "))]));
    }
    if !spec.at.is_empty() {
        parts.push(look.tr_with("node-at", &[("times", &spec.at.join(", "))]));
    }
    if let Some(cron) = &spec.cron {
        parts.push(look.tr_with("node-cron", &[("expression", cron)]));
    }
    if parts.is_empty() {
        return look.tr("node-no-schedule");
    }
    parts.join(" \u{b7} ")
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

/// A moment as a person reads it on this computer. **A test build reads
/// it in UTC**, so a picture of one is the same in every time zone.
pub fn moment(at: Epoch) -> String {
    let Some(at) = chrono::DateTime::from_timestamp(at.get(), 0) else {
        return String::new();
    };
    #[cfg(feature = "testing")]
    let at = at.with_timezone(&chrono::Utc);
    #[cfg(not(feature = "testing"))]
    let at = at.with_timezone(&chrono::Local);
    at.format("%Y-%m-%d %H:%M").to_string()
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
        Problem::Schedule { why, .. } => {
            look.tr_with("problem-schedule", &[("why", &why.to_string())])
        }
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

/// `at` on the clock a person reads: this computer's, **UTC in a test
/// build**, as [`moment`].
fn on_the_clock(at: Epoch) -> NaiveDateTime {
    let Some(at) = chrono::DateTime::from_timestamp(at.get(), 0) else {
        return NaiveDateTime::default();
    };
    #[cfg(feature = "testing")]
    let at = at.with_timezone(&chrono::Utc);
    #[cfg(not(feature = "testing"))]
    let at = at.with_timezone(&chrono::Local);
    at.naive_local()
}

/// Which day `at` is, said against `now`.
pub fn day_of(at: Epoch, now: Epoch) -> Day {
    day_between(on_the_clock(at).date(), on_the_clock(now).date())
}

fn day_between(at: NaiveDate, today: NaiveDate) -> Day {
    match (today - at).num_days() {
        0 => Day::Today,
        1 => Day::Yesterday,
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

/// A moment as a person says it against `now`: "Today at 3:05 PM",
/// "Monday at 9:30 AM", "March 5, 2025 at 11:59 PM".
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Settings;

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

    #[test]
    fn a_moment_is_today_yesterday_a_weekday_this_week_or_a_date_in_both_languages() {
        // A Thursday.
        let now = at("2026-09-17", "10:00");
        let (en, es) = (speaking("en"), speaking("es"));
        for (moment, english, spanish) in [
            (
                at("2026-09-17", "15:05"),
                "Today at 3:05 PM",
                "Hoy a las 15:05", // i18n-allow
            ),
            (
                at("2026-09-16", "01:05"),
                "Yesterday at 1:05 AM",
                "Ayer a la 01:05", // i18n-allow
            ),
            (
                at("2026-09-14", "09:30"),
                "Monday at 9:30 AM",
                "El lunes a las 09:30", // i18n-allow
            ),
            (
                at("2026-09-11", "00:00"),
                "Friday at 12:00 AM",
                "El viernes a las 00:00", // i18n-allow
            ),
            (
                at("2026-09-10", "12:00"),
                "September 10 at 12:00 PM",
                "El 10 de septiembre a las 12:00", // i18n-allow
            ),
            (
                at("2025-12-31", "23:59"),
                "December 31, 2025 at 11:59 PM",
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
        assert_eq!(clock_words(early, &speaking("en")), "12:30 AM");
        assert_eq!(
            moment_words(late, early, &speaking("en")),
            "Yesterday at 11:30 PM"
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
        }
    }
}
