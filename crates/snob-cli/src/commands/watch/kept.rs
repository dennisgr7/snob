//! `snob watch automations` and `snob watch start`: every automation, its
//! schedules, what keeps it from running, when it runs next and how its
//! newest runs went, whether one is still going and what each found as data
//! (`automations::said`); and a run of one started now, apart from the command
//! (`snob_engine::automations::run_now`). **Neither sends anything to
//! Instagram**: the run started does, under every rule a run by hand keeps.

use anyhow::Result;
use serde_json::{Value, json};
use snob_core::Epoch;
use snob_core::model::printable;
use snob_core::watch::schedule::Spec;
use snob_engine::automations::{self, Next, automation::Automation, said};
use snob_engine::exit::{ExitCode, ExitError};
use snob_store::paths::{AccountPaths, AppPaths};

use crate::cli::{Cli, WatchAutomationsArgs, WatchStartArgs};
use crate::ui;

/// How many of each automation's newest runs are listed.
pub const NEWEST_RUNS: usize = 5;

/// Lists every automation.
pub fn automations(args: WatchAutomationsArgs, paths: &AppPaths) -> Result<ExitCode> {
    let kept = automations::all(paths);
    let now = snob_core::clock::now();
    let mut listed = Vec::with_capacity(kept.automations.len());
    for automation in &kept.automations {
        listed.push(Listed::of(paths, automation, now)?);
    }
    let unreadable: Vec<String> = kept.unreadable.iter().map(|e| printable(e)).collect();

    if args.output.json {
        ui::say!(
            "{}",
            super::wire::json_line(&json!({
                "automations": listed.iter().map(Listed::json).collect::<Vec<_>>(),
                "unreadable": unreadable,
            }))
        );
        return Ok(ExitCode::Ok);
    }
    if listed.is_empty() {
        ui::info("There is no automation: \"snob watch setup\" writes watch.toml.");
    }
    for automation in &listed {
        for line in automation.lines() {
            ui::say!("{line}");
        }
    }
    for sentence in &unreadable {
        ui::warn(&format!("Could not read an automation: {sentence}"));
    }
    Ok(ExitCode::Ok)
}

/// Starts a run of the automation `args.id` now, as the account this command
/// resolved, when any.
pub fn start(
    cli: &Cli,
    args: &WatchStartArgs,
    paths: &AppPaths,
    account: Option<&AccountPaths>,
) -> Result<ExitCode> {
    let automation = super::automation::named(paths, &args.id)?;
    // Refused here, where the person reads it, rather than in the run's log.
    if let Some(problem) = automations::problems(paths, &automation).first() {
        return Err(ExitError::new(
            ExitCode::Error,
            format!(
                "\"{}\" cannot run: {}",
                printable(&automation.id),
                printable(&problem.to_string())
            ),
        )
        .into());
    }
    let mut globals = Vec::new();
    if let Some(account) = account {
        globals.extend(["--account".into(), account.pk().to_string().into()]);
    }
    globals.extend(crate::jobs::argv::settings(cli)?);
    automations::run_now(paths, &automation.id, &globals)?;

    let id = printable(&automation.id);
    if args.output.json {
        ui::say!("{}", super::wire::json_line(&json!({ "started": id })));
    }
    ui::info(&format!(
        "Started a run of \"{id}\". \"snob watch automations\" shows how it went once it ends."
    ));
    Ok(ExitCode::Ok)
}

/// One automation as listed.
struct Listed {
    id: String,
    name: String,
    /// Its enabled triggers' schedules, as written.
    schedules: Vec<Spec>,
    problems: Vec<String>,
    next: Next,
    runs: Vec<snob_store::store::shared::AutomationRun>,
}

impl Listed {
    fn of(paths: &AppPaths, automation: &Automation, now: Epoch) -> Result<Self> {
        let problems = automations::problems(paths, automation)
            .iter()
            .map(|problem| printable(&problem.to_string()))
            .collect();
        let (next, _) = automations::next_run(paths, automation, now)?;
        Ok(Self {
            id: printable(&automation.id),
            name: printable(&automation.name),
            schedules: automation.schedules().into_iter().map(shown).collect(),
            problems,
            next,
            runs: automations::runs(paths, &automation.id, NEWEST_RUNS, now)?,
        })
    }

    fn json(&self) -> Value {
        let (when, at) = match self.next {
            Next::Now => ("now", None),
            Next::At(at) => ("at", Some(at)),
            Next::ByHand => ("by_hand", None),
            Next::Never => ("never", None),
        };
        json!({
            "id": self.id,
            "name": self.name,
            "schedules": self.schedules,
            "problems": self.problems,
            "next": { "when": when, "at": at },
            "runs": self.runs.iter().map(|run| json!({
                "started_at": run.started_at,
                "ended_at": run.ended_at,
                "outcome": run.outcome.as_deref().map(printable),
                "requests": run.requests,
                "summary": printable(&run.summary),
                "running": run.running,
                "found": run.found.as_deref().and_then(said::read),
            })).collect::<Vec<_>>(),
        })
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = vec![if self.name.is_empty() {
            self.id.clone()
        } else {
            format!("{}  {}", self.id, self.name)
        }];
        lines.extend(self.schedules.iter().map(|when| {
            let clauses = crate::report::schedule_clauses(when);
            if clauses.is_empty() {
                "  Runs on no schedule yet.".to_string()
            } else {
                format!("  Runs {}.", clauses.join(", "))
            }
        }));
        lines.push(format!(
            "  Next run: {}.",
            match self.next {
                Next::Now => "due now".to_string(),
                Next::At(at) => snob_engine::sentences::stored_on(at),
                Next::ByHand => "only when started by hand".to_string(),
                Next::Never => "never".to_string(),
            }
        ));
        lines.extend(
            self.problems
                .iter()
                .map(|problem| format!("  Cannot run: {problem}.")),
        );
        if let Some(run) = self.runs.first() {
            let how = match (run.running, run.outcome.as_deref()) {
                (true, _) => "still running".to_string(),
                (false, Some(outcome)) => printable(outcome),
                (false, None) => "did not finish".to_string(),
            };
            lines.push(format!(
                "  Last run: {}, {how}.",
                snob_engine::sentences::stored_on(run.started_at),
            ));
        }
        lines
    }
}

/// A schedule as written by a person, each word made printable.
fn shown(when: &Spec) -> Spec {
    let all = |words: &[String]| words.iter().map(|word| printable(word)).collect();
    Spec {
        on: all(&when.on),
        at: all(&when.at),
        months: all(&when.months),
        days: all(&when.days),
        cron: when.cron.as_deref().map(printable),
        ..when.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(schedules: Vec<Spec>) -> Listed {
        Listed {
            id: "watch".into(),
            name: String::new(),
            schedules,
            problems: Vec::new(),
            next: Next::ByHand,
            runs: Vec::new(),
        }
    }

    fn run(running: bool, outcome: Option<&str>) -> snob_store::store::shared::AutomationRun {
        snob_store::store::shared::AutomationRun {
            id: 1,
            automation: "watch".into(),
            started_at: Epoch::new(1_000),
            ended_at: outcome.map(|_| Epoch::new(1_060)),
            outcome: outcome.map(str::to_string),
            requests: 3,
            summary: String::new(),
            running,
            found: None,
        }
    }

    #[test]
    fn a_run_that_went_quiet_is_listed_as_unfinished_not_running() {
        let last_run = |run| {
            let mut listed = listed(Vec::new());
            listed.runs = vec![run];
            listed.lines().pop().unwrap()
        };
        assert!(last_run(run(true, None)).ends_with(", still running."));
        assert!(last_run(run(false, None)).ends_with(", did not finish."));
        assert!(last_run(run(false, Some("rate_limited"))).ends_with(", rate_limited."));

        let mut listed = listed(Vec::new());
        listed.runs = vec![run(false, None)];
        let json = listed.json();
        assert_eq!(json["runs"][0]["running"], false);
        assert!(json["runs"][0]["found"].is_null());
    }

    #[test]
    fn what_a_run_found_is_listed_as_data_and_a_kind_unknown_as_null() {
        let found = |found: &str| {
            let mut listed = listed(Vec::new());
            let mut run = run(false, Some("ok"));
            run.found = Some(found.to_string());
            listed.runs = vec![run];
            listed.json()["runs"][0]["found"].clone()
        };
        let known = found(r#"[{"kind":"cannot_run"},{"kind":"failed","code":"error"}]"#);
        assert_eq!(known[0]["kind"], "cannot_run");
        assert_eq!(known[1]["kind"], "failed");
        assert_eq!(known[1]["code"], "error");
        assert!(found(r#"[{"kind":"from_a_newer_snob"}]"#).is_null());
    }

    #[test]
    fn each_schedule_is_a_line_and_an_empty_one_says_so() {
        let lines = listed(vec![
            Spec {
                days: vec!["1".into(), "first mon".into()],
                months: vec!["jan".into(), "jul".into()],
                at: vec!["09:00".into()],
                ..Spec::default()
            },
            Spec::default(),
        ])
        .lines();
        assert_eq!(
            lines[1],
            "  Runs on the 1, first mon of the month, in jan, jul, at 09:00."
        );
        assert_eq!(lines[2], "  Runs on no schedule yet.");
    }
}
