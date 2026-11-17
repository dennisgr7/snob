//! What a step, a problem and a refusal are called, in the words of the
//! system's language: the names the panel, the canvas, an automation's page
//! and the dialogs share, so a thing is the same thing everywhere.

use snob_core::Epoch;
use snob_core::automation::{Automation, Problem, Stage, Step, Unjoined, WATCH_TOML};
use snob_core::duration;
use snob_core::watch::schedule::Spec;

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
        Problem::Schedule { why, .. } => look.tr_with("problem-schedule", &[("why", why)]),
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
