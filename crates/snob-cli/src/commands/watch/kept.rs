//! `snob watch automations` and `snob watch start`: every automation, what
//! keeps it from running, when it runs next and how its newest runs went; and
//! a run of one started now, apart from the command
//! (`snob_engine::automations::run_now`). **Neither sends anything to
//! Instagram**: the run started does, under every rule a run by hand keeps.

use anyhow::Result;
use serde_json::{Value, json};
use snob_core::Epoch;
use snob_core::model::printable;
use snob_engine::automations::{self, Next, automation::Automation};
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
            problems,
            next,
            runs: automations::runs(paths, &automation.id, NEWEST_RUNS)?,
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
            "problems": self.problems,
            "next": { "when": when, "at": at },
            "runs": self.runs.iter().map(|run| json!({
                "started_at": run.started_at,
                "ended_at": run.ended_at,
                "outcome": run.outcome.as_deref().map(printable),
                "requests": run.requests,
                "summary": printable(&run.summary),
            })).collect::<Vec<_>>(),
        })
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = vec![if self.name.is_empty() {
            self.id.clone()
        } else {
            format!("{}  {}", self.id, self.name)
        }];
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
            lines.push(format!(
                "  Last run: {}, {}.",
                snob_engine::sentences::stored_on(run.started_at),
                run.outcome
                    .as_deref()
                    .map_or("still running", |outcome| outcome)
            ));
        }
        lines
    }
}
