//! `snob __job …` and `snob jobs …`, as the account this run acts as and on
//! that account's jobs alone, and what `purge` and `logout` say of the jobs
//! they stop.

use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use snob_store::paths::{self, AccountPaths};

use super::show::{ended, find, finished, line, row, settle, whole};
use super::{Seen, Stopping, argv, start, supervise, sweep_if_free};
use snob_engine::exit::ExitCode;

fn now() -> i64 {
    snob_core::clock::now_ms().get()
}

/// [`super::stop_every_job`], saying how many were stopped when there were
/// any, and warning when one did not stop in time.
pub async fn stop_before_leaving(accounts: Vec<AccountPaths>) -> Result<Vec<Stopping>> {
    let (stopping, settled) = super::stop_every_job(accounts).await?;
    let count: usize = stopping.iter().map(|jobs| jobs.ids.len()).sum();
    if count > 0 {
        crate::ui::say!(
            "Stopped {count} {} first.",
            if count == 1 { "job" } else { "jobs" }
        );
    }
    if !settled {
        crate::ui::warn("a job did not stop in time; what it holds may not all go");
    }
    Ok(stopping)
}

/// Runs `snob __job …` and `snob jobs …`, as the account this run acts as and
/// on that account's jobs alone.
pub async fn dispatch(
    cli: &crate::cli::Cli,
    store: &snob_store::secrets::SecretStore,
    app: &paths::AppPaths,
) -> anyhow::Result<snob_engine::exit::ExitCode> {
    use crate::cli::{Command, JobCommand, JobsArgs, JobsCommand};

    let env = std::env::var("SNOB_ACCOUNT").ok();
    let registry = snob_store::registry::Registry::load(app)?;
    let viewer =
        snob_engine::account::resolve_if_any(cli.account.as_deref(), env.as_deref(), &registry)?
            .ok_or_else(crate::commands::common::no_session)?;
    snob_engine::sentences::act_as(viewer.clone());
    let account = app.account(viewer.pk);
    // What reads the jobs sweeps what is past its keeping first, when the
    // lock is free: a start sweeps under it, and a supervisor touches only
    // its own job.
    if !matches!(
        cli.command,
        Command::Job(JobCommand::Start(_) | JobCommand::Run(_))
    ) {
        sweep_if_free(&account, snob_core::clock::now_ms().get());
    }

    match &cli.command {
        Command::Job(JobCommand::Start(args)) => start::run(cli, args, store, &account).await,
        Command::Job(JobCommand::Run(args)) => {
            let walk_args = |command: &[String]| argv::walk(cli, account.pk(), command);
            supervise::run(&account, &args.id, argv::walk_of, &walk_args).await
        }
        Command::Job(JobCommand::Show(args))
        | Command::Jobs(JobsArgs {
            command: Some(JobsCommand::Show(args)),
            ..
        }) => show(&account, &args.id, args.wait, args.output.json).await,
        Command::Job(JobCommand::Cancel(args))
        | Command::Jobs(JobsArgs {
            command: Some(JobsCommand::Cancel(args)),
            ..
        }) => cancel(&account, &args.id, args.wait, args.output.json).await,
        Command::Job(JobCommand::List(args))
        | Command::Jobs(JobsArgs {
            command: Some(JobsCommand::List(args)),
            ..
        })
        | Command::Jobs(JobsArgs {
            command: None,
            list: args,
        }) => list(&account, args.all, args.output.json),
        _ => unreachable!("only the jobs' commands are dispatched here"),
    }
}

/// `show <id>`: the job, and its result once it has one.
pub async fn show(
    account: &AccountPaths,
    id: &str,
    wait: Duration,
    json: bool,
) -> Result<ExitCode> {
    let seen = settle(account, find(account, id)?, wait).await?;
    let now = now();
    if json {
        crate::ui::say!(
            "{}",
            serde_json::to_string(&whole(&seen, now, argv::walk_of))?
        );
    } else {
        crate::ui::say!("{}", line(&seen, now, argv::walk_of));
        if let Some(finished) = finished(&seen) {
            crate::ui::say!("  {}", ended(&finished, seen.id.as_str()));
        }
    }
    Ok(ExitCode::Ok)
}

/// `list`: the jobs not yet finished, or every one kept with `all`.
pub fn list(account: &AccountPaths, all: bool, json: bool) -> Result<ExitCode> {
    let now = now();
    let jobs: Vec<Seen> = super::all(account, now)?
        .into_iter()
        .filter(|seen| all || !seen.status.finished())
        .collect();
    if json {
        let records: Vec<Value> = jobs
            .iter()
            .map(|seen| row(seen, now, argv::walk_of))
            .collect();
        crate::ui::say!("{}", serde_json::to_string(&json!({ "jobs": records }))?);
    } else if jobs.is_empty() {
        crate::ui::say!(
            "{}",
            if all {
                "No jobs are kept for this account."
            } else {
                "No job is running for this account."
            }
        );
    } else {
        for seen in &jobs {
            crate::ui::say!("{}", line(seen, now, argv::walk_of));
        }
    }
    Ok(ExitCode::Ok)
}

/// `cancel <id>`: asks the job to stop, as Ctrl+C stops a walk, keeping what
/// it read, and waits at most `wait` for it to have.
pub async fn cancel(
    account: &AccountPaths,
    id: &str,
    wait: Duration,
    json: bool,
) -> Result<ExitCode> {
    let seen = find(account, id)?;
    let asked = !seen.status.is_over();
    if asked {
        super::ask_to_stop(&seen)?;
    }
    let seen = settle(account, seen, wait).await?;
    let now = now();
    if json {
        crate::ui::say!(
            "{}",
            serde_json::to_string(&whole(&seen, now, argv::walk_of))?
        );
    } else if asked {
        crate::ui::say!("{} was asked to stop; it keeps what it read.", seen.id);
        crate::ui::say!("{}", line(&seen, now, argv::walk_of));
    } else {
        crate::ui::say!("{}", line(&seen, now, argv::walk_of));
    }
    Ok(ExitCode::Ok)
}
