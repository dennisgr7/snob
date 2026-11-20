//! A run of an automation kept as a file (`snob watch once --automation`,
//! and the due ones a scheduled run or a timer finds): its lists and posts
//! through the monitor's run ([`super::run::run_viewers`]), its other actions
//! through `snob_engine::automations::read`, its report to its own webhook,
//! and a row in its history, as `watch.toml`'s runs leave one ([`Recorded`]),
//! with a sign of life while it goes and what it found as data once it ends.

use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use snob_core::automation::{Automation, Planned, Read, Started, Step, WATCH_TOML};
use snob_core::model::printable;
use snob_core::{Epoch, Pk};
use snob_engine::app::App;
use snob_engine::automations;
use snob_engine::automations::said::{self, Said};
use snob_engine::exit::{ExitCode, ExitError};
use snob_engine::watch::RunTiming;
use snob_store::paths::AppPaths;
use snob_store::secrets::SecretStore;
use snob_store::store::shared::{RUN_BEAT, Shared};

use crate::ui;

use super::delivery::{Delivery, deliver_found, delivery_of};
use super::run::{Printing, RunSummary, drain_all, run_viewers};
use super::say::found_lines;
use super::wire::{found_event, found_payload, json_line};

/// A run being written into an automation's history: started before its
/// work, so it counts from its start, beating while it goes, so a front tells
/// it from one that was killed, and finished with what it came to.
pub(super) struct Recorded {
    shared: Shared,
    row: i64,
    /// What stops the beat: the sender dropped, the thread joined.
    beat: Option<(mpsc::Sender<()>, JoinHandle<()>)>,
}

impl Recorded {
    /// Writes down that a run of `id` starts at `at`; `None`, said in the
    /// log, when `shared.db` cannot be written, which is no reason not to run.
    pub(super) fn start(paths: &AppPaths, id: &str, at: Epoch) -> Option<Self> {
        Self::beating_every(paths, id, at, RUN_BEAT)
    }

    fn beating_every(paths: &AppPaths, id: &str, at: Epoch, every: Duration) -> Option<Self> {
        let started = Shared::open(paths).and_then(|shared| {
            let row = shared.record_automation_run(id, at)?;
            Ok(Self {
                shared,
                row,
                beat: None,
            })
        });
        match started {
            Ok(mut recorded) => {
                recorded.beat = beat(paths, recorded.row, every);
                Some(recorded)
            }
            Err(e) => {
                tracing::debug!(error = %e, "could not write down that the run started");
                None
            }
        }
    }

    /// What the run came to. The beat stops first, so none lands after.
    pub(super) fn finish(mut self, summary: &RunSummary) {
        if let Some((stop, beating)) = self.beat.take() {
            drop(stop);
            if beating.join().is_err() {
                tracing::debug!("the run's beat stopped on a panic");
            }
        }
        let outcome = match &summary.failed {
            Some(e) if summary.code == ExitCode::Ok => {
                snob_engine::exit::from_chain(e).unwrap_or(ExitCode::Error)
            }
            _ => summary.code,
        };
        let said = summary.said.join("\n");
        let found = match serde_json::to_string(&summary.found) {
            Ok(found) => Some(found),
            Err(e) => {
                tracing::debug!(error = %e, "could not write down what the run found");
                None
            }
        };
        if let Err(e) = self.shared.finish_automation_run(
            self.row,
            snob_core::clock::now(),
            outcome.as_str(),
            summary.spent,
            &said,
            found.as_deref(),
        ) {
            tracing::debug!(error = %e, "could not write down what the run came to");
        }
    }
}

/// A thread writing down every `every` that the run `row` still goes, on a
/// `shared.db` connection of its own, until its sender is dropped.
fn beat(paths: &AppPaths, row: i64, every: Duration) -> Option<(mpsc::Sender<()>, JoinHandle<()>)> {
    let paths = paths.clone();
    let (stop, stopped) = mpsc::channel::<()>();
    let beating = std::thread::Builder::new()
        .name("run-beat".into())
        .spawn(move || {
            let shared = match Shared::open(&paths) {
                Ok(shared) => shared,
                Err(e) => {
                    tracing::debug!(error = %e, "the run's beat could not open shared.db");
                    return;
                }
            };
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
                if let Err(e) = shared.beat_automation_run(row, snob_core::clock::now()) {
                    tracing::debug!(error = %e, "could not write down that the run still goes");
                }
            }
        });
    match beating {
        Ok(beating) => Some((stop, beating)),
        Err(e) => {
            tracing::debug!(error = %e, "the run's beat could not start");
            None
        }
    }
}

/// One run of `automation`, kept as a file.
#[expect(
    clippy::too_many_arguments,
    reason = "what a run is handed, each from its own place"
)]
pub(super) async fn run_automation(
    paths: &AppPaths,
    secrets: &SecretStore,
    automation: &Automation,
    started: Started,
    printing: Printing,
    with_progress: bool,
    timing: RunTiming,
    in_use: Option<Pk>,
) -> RunSummary {
    let recorded = Recorded::start(paths, &automation.id, timing.began_at);
    let summary = run_program(
        paths,
        secrets,
        automation,
        started,
        printing,
        with_progress,
        timing,
        in_use,
    )
    .await;
    if let Some(recorded) = recorded {
        recorded.finish(&summary);
    }
    summary
}

/// One run, reading what the triggers it follows lead to: a schedule's
/// alone when one came round, every trigger's when somebody ran it.
#[expect(
    clippy::too_many_arguments,
    reason = "what a run is handed, each from its own place"
)]
async fn run_program(
    paths: &AppPaths,
    secrets: &SecretStore,
    automation: &Automation,
    started: Started,
    printing: Printing,
    with_progress: bool,
    timing: RunTiming,
    in_use: Option<Pk>,
) -> RunSummary {
    let problems = automations::problems(paths, automation);
    if !problems.is_empty() {
        let said: Vec<String> = problems.iter().map(ToString::to_string).collect();
        return cannot_run(
            ExitError::new(
                ExitCode::Error,
                format!(
                    "\"{}\" cannot run: {}",
                    printable(&automation.name),
                    said.join("; ")
                ),
            )
            .into(),
        );
    }
    let delivery = match delivery_of(automation.webhook()) {
        Ok(delivery) => delivery,
        Err(e) => return failed(e),
    };
    let reads = automation.reads_when(started);
    let monitor = |delivered: bool| -> Vec<Planned> {
        reads
            .iter()
            .filter(|read| matches!(read.what, Read::Lists | Read::Posts { .. }))
            .filter(|read| read.delivered == delivered)
            .cloned()
            .collect()
    };
    let mut summary = RunSummary {
        spent: 0,
        code: ExitCode::Ok,
        failed: None,
        said: Vec::new(),
        found: Vec::new(),
    };
    for (delivered, to) in [(true, delivery.as_ref()), (false, None)] {
        let watched = automations::watched(&monitor(delivered), in_use);
        if watched.is_empty() {
            continue;
        }
        if let Err(e) = super::posts::refuse_what_cannot_be_saved(&watched) {
            fold(&mut summary, failed(e));
            continue;
        }
        let ran = run_viewers(
            paths,
            secrets,
            &watched,
            to.filter(|_| delivered),
            printing,
            with_progress,
            timing,
        )
        .await;
        fold(&mut summary, ran);
    }

    let others: Vec<&Planned> = reads
        .iter()
        .filter(|read| !matches!(read.what, Read::Lists | Read::Posts { .. }))
        .collect();
    if !others.is_empty() {
        let found = read_others(
            paths,
            secrets,
            automation,
            &others,
            delivery.as_ref(),
            printing,
            with_progress,
            in_use,
        )
        .await;
        fold(&mut summary, found);
        if let Some(delivery) = delivery.as_ref() {
            let cancel = snob_engine::system::interrupt::install();
            drain_all(paths, &cancel, delivery, false).await;
        }
    }
    summary
}

/// Adds what a part of the run came to, its first reason and its last
/// failure kept, as a run over several accounts keeps them.
fn fold(into: &mut RunSummary, part: RunSummary) {
    into.spent += part.spent;
    if into.code == ExitCode::Ok {
        into.code = part.code;
    }
    if let Some(earlier) = into.failed.take() {
        crate::report::print_error(&earlier, crate::report::Wording::Prose);
    }
    into.failed = part.failed;
    into.said.extend(part.said);
    into.found.extend(part.found);
}

/// The actions the monitor does not read, each as the viewer it names.
#[allow(clippy::too_many_arguments)]
async fn read_others(
    paths: &AppPaths,
    secrets: &SecretStore,
    automation: &Automation,
    reads: &[&Planned],
    delivery: Option<&Delivery>,
    printing: Printing,
    with_progress: bool,
    in_use: Option<Pk>,
) -> RunSummary {
    let mut summary = RunSummary {
        spent: 0,
        code: ExitCode::Ok,
        failed: None,
        said: Vec::new(),
        found: Vec::new(),
    };
    let mut viewers: Vec<Option<Pk>> = Vec::new();
    for read in reads {
        let viewer = read.account.viewer.or(in_use);
        if !viewers.contains(&viewer) {
            viewers.push(viewer);
        }
    }
    let cancel = snob_engine::system::interrupt::install();
    for viewer in viewers {
        if cancel.is_canceled() {
            break;
        }
        let Some(pk) = viewer else {
            let e = snob_engine::sentences::no_session();
            fold(&mut summary, failed(e));
            continue;
        };
        let account = paths.account(pk);
        let opened = App::open(&secrets.session_of(&account), &account, || {
            crate::front::front(with_progress)
        });
        let mut app = match opened {
            Ok(Some(app)) => app,
            Ok(None) => {
                fold(
                    &mut summary,
                    failed(
                        ExitError::new(
                            ExitCode::NoSession,
                            format!("no session is stored for account {pk}"),
                        )
                        .with_hint(format!("run \"snob login --account {pk}\""))
                        .into(),
                    ),
                );
                continue;
            }
            Err(e) => {
                fold(&mut summary, failed(e));
                continue;
            }
        };
        app.consent_comes_from_the_config();
        app.stops_on_a_critical_battery(snob_engine::system::power::battery::critical);
        for (index, read) in reads
            .iter()
            .filter(|read| read.account.viewer.or(in_use) == viewer)
            .enumerate()
        {
            if app.cancel().is_canceled() {
                break;
            }
            if index > 0
                && app
                    .client()
                    .dwell_before_a_list(&snob_ig::pace::Pace::default())
                    .await
            {
                break;
            }
            let before = app.client().pacer().spent();
            let part = read_one(&mut app, automation, read, delivery, printing).await;
            let spent = app.client().pacer().spent().saturating_sub(before);
            fold(&mut summary, RunSummary { spent, ..part });
        }
    }
    summary
}

fn failed(e: anyhow::Error) -> RunSummary {
    let found = said::failed(&e);
    refusal(e, found)
}

/// A run its steps keep from running.
fn cannot_run(e: anyhow::Error) -> RunSummary {
    refusal(e, Said::CannotRun)
}

fn refusal(e: anyhow::Error, found: Said) -> RunSummary {
    RunSummary {
        spent: 0,
        code: snob_engine::exit::exit_code_for(&e),
        said: vec![format!("{e:#}")],
        found: vec![found],
        failed: Some(e),
    }
}

/// One action: read, told, delivered.
async fn read_one(
    app: &mut App,
    automation: &Automation,
    read: &Planned,
    delivery: Option<&Delivery>,
    printing: Printing,
) -> RunSummary {
    let found = automations::read::read(app, &automation.id, read).await;
    app.progress().finish();
    let found = match found {
        Ok(found) => found,
        Err(e) => return failed(e),
    };
    let lines = found_lines(&found);
    if printing.json() {
        let at = snob_core::clock::now();
        let line = found_payload(
            automation,
            &found,
            app.viewer(),
            "",
            found_event(found.has_news()),
            at,
        );
        ui::say!("{}", json_line(&line));
    } else {
        for line in &lines {
            ui::say!("{line}");
        }
    }
    let to = delivery.filter(|_| read.delivered);
    if let Err(e) = deliver_found(app, automation, &found, to).await {
        return failed(e);
    }
    RunSummary {
        spent: 0,
        code: ExitCode::Ok,
        failed: None,
        said: lines,
        found: vec![said::of_found(&found)],
    }
}

/// The stored automations a scheduled run or a timer finds due at `now`,
/// each run in its turn: `watch.toml`'s is not among them, its schedule
/// being the run's own.
pub(super) async fn run_due(
    paths: &AppPaths,
    secrets: &SecretStore,
    printing: Printing,
    with_progress: bool,
    in_use: Option<Pk>,
) -> Vec<RunSummary> {
    let kept = automations::all(paths);
    for unreadable in &kept.unreadable {
        ui::warn(unreadable);
    }
    let mut ran = Vec::new();
    for automation in kept
        .automations
        .iter()
        .filter(|automation| automation.id != WATCH_TOML)
    {
        let now = snob_core::clock::now();
        let due_at = match automations::next_run(paths, automation, now) {
            Ok((automations::Next::Now, due_at)) => due_at,
            Ok(_) => continue,
            Err(e) => {
                tracing::debug!(error = %e, id = %automation.id, "could not tell whether it is due");
                continue;
            }
        };
        ui::info(&format!(
            "Running \"{}\", which is due.",
            printable(&automation.name)
        ));
        let timing = RunTiming::due(due_at, now, std::time::Duration::ZERO);
        let summary = run_automation(
            paths,
            secrets,
            automation,
            Started::BySchedule,
            printing,
            with_progress,
            timing,
            in_use,
        )
        .await;
        ran.push(summary);
    }
    ran
}

/// Whether any automation kept as a file has a schedule: one only ever run
/// by hand gives a scheduled run or a timer nothing to wait for.
pub(super) fn any_on_a_schedule(paths: &AppPaths) -> bool {
    automations::all(paths)
        .automations
        .iter()
        .filter(|automation| automation.id != WATCH_TOML)
        .flat_map(|automation| &automation.nodes)
        .any(|node| node.enabled && matches!(node.step, Step::Schedule { .. }))
}

/// The automation `id`, or why there is none to run.
pub(super) fn named(paths: &AppPaths, id: &str) -> Result<Automation> {
    automations::find(paths, id)?.ok_or_else(|| {
        ExitError::new(
            ExitCode::Error,
            format!("there is no automation \"{}\"", printable(id)),
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive_at(paths: &AppPaths, row: i64) -> i64 {
        Shared::open(paths)
            .unwrap()
            .conn()
            .query_row(
                "SELECT alive_at FROM automation_runs WHERE id = ?1",
                [row],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn the_heartbeat_stops_when_the_run_is_written_down() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let recorded = Recorded::beating_every(
            &paths,
            "abcd1234",
            Epoch::new(1_000),
            Duration::from_millis(10),
        )
        .unwrap();
        let row = recorded.row;
        let started = std::time::Instant::now();
        while alive_at(&paths, row) == 1_000 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the run never beat"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        recorded.finish(&RunSummary {
            spent: 2,
            code: ExitCode::Ok,
            failed: None,
            said: vec!["Your fans: 1 account.".into()],
            found: vec![Said::CannotRun],
        });
        let now = snob_core::clock::now();
        let runs = automations::runs(&paths, "abcd1234", 1, now).unwrap();
        assert!(!runs[0].running && runs[0].ended_at.is_some());
        assert_eq!(
            runs[0].found.as_deref().and_then(said::read),
            Some(vec![Said::CannotRun])
        );
        let last = alive_at(&paths, row);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            alive_at(&paths, row),
            last,
            "no beat once it is written down"
        );
    }
}
