//! `snob watch`: the monitor.

use anyhow::Result;
use snob_engine::exit::ExitCode;
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::secrets::SecretStore;

use crate::cli::{WatchArgs, WatchCommand};

pub(super) mod delivery;

pub(super) mod setup;
pub(super) mod status;

pub(super) mod say;
pub(super) mod wire;

mod check;
mod deliver;
mod diff;
mod once;
mod run;
mod scheduled;
mod unit;

pub mod posts;

mod preflight;
mod schedule;
mod watched;

#[cfg(test)]
mod fixtures;

use check::check;
use deliver::deliver;
use diff::diff;
use once::once;
use scheduled::scheduled;
use unit::unit;

/// Runs the monitor. `account` is the account resolved for this run, when any
/// is signed in; a watched entry that names no viewer is read as it.
pub async fn run(
    args: WatchArgs,
    secrets: SecretStore,
    paths: &AppPaths,
    account: Option<AccountPaths>,
) -> Result<ExitCode> {
    let in_use = account.as_ref().map(|account| account.pk());
    let account = account.as_ref();
    match args.command {
        Some(WatchCommand::Diff(args)) => diff(args, secrets, account),
        Some(WatchCommand::Once(args)) => once(args, secrets, paths, in_use).await,
        Some(WatchCommand::Check(args)) => check(args, secrets, paths, account).await,
        Some(WatchCommand::Setup(args)) => setup::setup(args, secrets, paths, account).await,
        Some(WatchCommand::Status(args)) => status::status(args, paths, account),
        Some(WatchCommand::Deliver(args)) => deliver(args, secrets, paths).await,
        Some(WatchCommand::Unit(args)) => unit(args, paths),
        // With nobody signed in there is nobody to read any entry as, and a
        // service started then would read the entries that name no viewer as
        // nobody for as long as it ran.
        None => {
            let in_use = in_use.ok_or_else(snob_engine::sentences::no_session)?;
            scheduled(args.run, secrets, paths, in_use).await
        }
    }
}
