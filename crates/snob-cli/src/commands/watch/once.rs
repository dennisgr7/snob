//! `snob watch once`: one run of the monitor, by hand or on a timer.

use anyhow::Result;
use snob_core::Pk;
use snob_engine::exit::ExitCode;
use snob_store::config;
use snob_store::paths::AppPaths;
use snob_store::secrets::SecretStore;

use crate::cli::WatchOnceArgs;
use crate::report;
use crate::ui;

use super::delivery::delivery_from;
use super::run::{Printing, run_viewers, settle_all};
use super::watched::watched_from;

/// One run of the monitor: look, report, and remember having reported.
///
/// **The scheduled run without the loop**, which is what the README's "one run,
/// for cron or a systemd timer" describes: the webhook and the accounts both
/// come from `watch.toml`, through the same `watched_from` the scheduled mode
/// asks.
pub(super) async fn once(
    args: WatchOnceArgs,
    secrets: SecretStore,
    paths: &AppPaths,
    in_use: Option<Pk>,
) -> Result<ExitCode> {
    // Settled before anything can refuse: a session that has gone, or a webhook
    // address `webhook::check` refuses, would otherwise leave owed reports aging
    // past `MAX_AGE_SECS`, where `due` no longer returns them and `failed` (the
    // only other thing that expires one) is never reached, while `status` goes
    // on promising the next run will try them.
    settle_all(paths);

    // Before the session is opened and long before a request is spent, so a
    // webhook address that could never work costs nothing to find out about.
    let configured = config::load(paths)?;
    let began = snob_core::clock::now();
    // A run by hand was due for nothing; one a timer asked for was due for
    // the moment the schedule names, which is what tells it late.
    let timing = if args.when_due {
        let Some(due_at) = due_for(paths, configured.as_ref(), began)? else {
            return Ok(ExitCode::Ok);
        };
        snob_engine::engine::watch::RunTiming::due(due_at, began, std::time::Duration::ZERO)
    } else {
        snob_engine::engine::watch::RunTiming::by_hand(began)
    };
    let delivery = delivery_from(&args.delivery, configured.as_ref(), &secrets)?;

    let watched = watched_from(args.target.clone(), configured.as_ref(), in_use);
    // A `quality = "best"` with no ffmpeg here, refused as an impossible
    // webhook address is: before anything is opened or spent.
    super::posts::refuse_what_cannot_be_saved(&watched)?;

    // A run on a timer at a critical battery would be cut off by the system
    // hibernating under it.
    if super::run::waits_for_power(paths, snob_core::clock::now()) {
        ui::warn(report::BATTERY_CRITICAL_HOLD);
        return Ok(ExitCode::Interrupted);
    }
    super::run::power_is_back(paths);

    // Said before the question about somebody's lists, which `engine::list`
    // asks and which knows nothing about where a report goes: the answer is
    // read as leave to send what changes in them to this address too, as the
    // wizard's question says. Only to somebody who can be asked.
    if let Some(delivery) = delivery.as_ref()
        && ui::can_be_asked()
    {
        for entry in watched
            .iter()
            .filter(|entry| entry.lists() && !entry.may_run_unattended())
        {
            if let Some(name) = entry.name() {
                ui::info(&report::sent_to(name, &delivery.destination));
            }
        }
    }

    let outcome = run_viewers(
        paths,
        &secrets,
        &watched,
        delivery.as_ref(),
        Printing::watched(args.output.json),
        !args.progress.no_progress,
        timing,
    )
    .await;

    // Said whether or not an account failed: a run that stopped halfway still
    // spent requests, and this is the mode somebody is watching.
    ui::info(&format!(
        "{} - {}",
        snob_engine::sentences::stored_on(snob_core::clock::now()),
        report::requests(outcome.spent)
    ));

    if let Some(e) = outcome.failed {
        return Err(e);
    }
    // A run whose own verdict is good but whose report did not arrive is not a
    // 0: on a timer nothing tries the report again before the next run, and a
    // status that said all was well left nobody to look.
    if outcome.code == ExitCode::Ok && delivery.as_ref().is_some_and(|d| d.left_undelivered()) {
        return Err(snob_engine::exit::ExitError::new(
            ExitCode::Error,
            "the run went through, but a report did not reach the webhook; it stays queued \
             for the next run, unless it was given up on as said above",
        )
        .with_reason("not_delivered")
        .into());
    }

    // The run's own verdict, as written into `watch_runs`.
    Ok(outcome.code)
}

/// `--when-due`: whether the schedule in the file says a run is owed now, asked
/// of the evaluator the scheduled mode sleeps on, from the same last run
/// (`scheduled::seed_for`), so a timer that asks every quarter of an hour runs
/// on the schedule `snob watch` would keep, a quarter of an hour late at most.
/// Not due costs nothing: no session is read and nothing is sent, and it is
/// said on standard error **only to a terminal**: a timer asks 96 times a day,
/// and each line would be one more in its journal, or one more mail from cron.
///
/// **A timer keeps nothing between two looks**, so the first look is written
/// down (`Shared::ask_since`) and a calendar that has never run counts the
/// moments it owes from it (`schedule::due_asked_since`).
fn due_for(
    paths: &AppPaths,
    configured: Option<&config::WatchConfig>,
    now: snob_core::Epoch,
) -> Result<Option<Option<snob_core::Epoch>>> {
    use snob_core::watch::schedule::{self, Due};

    // Not the flags' sentence: `once` has none of them, and `--when-due` reads
    // the file alone.
    if configured.is_none_or(super::schedule::names_no_schedule) {
        return Err(anyhow::anyhow!(
            "how often should this run? --when-due takes the schedule from watch.toml, and it \
             names none.\nRun \"snob watch setup\", or write one in it, such as every = \"6h\"."
        ));
    }
    let schedule =
        super::schedule::schedule_from(&crate::cli::WatchRunArgs::default(), configured)?;
    let last_run = super::scheduled::seed_for(paths, &schedule, now)?;
    let asked_since = snob_store::store::shared::Shared::open(paths)?.ask_since(now)?;
    match schedule::due_asked_since(&schedule, last_run, asked_since, now, &chrono::Local) {
        Due::Now { .. } => Ok(Some(schedule::due_moment(
            &schedule,
            last_run,
            Some(asked_since),
            now,
            &chrono::Local,
        ))),
        Due::At(at) => {
            use std::io::IsTerminal as _;
            let said = format!(
                "Not due yet: the next run is on {}.",
                snob_engine::sentences::stored_on(at)
            );
            if std::io::stderr().is_terminal() {
                ui::info(&said);
            } else {
                tracing::debug!("{said}");
            }
            Ok(None)
        }
        Due::Never => Err(anyhow::anyhow!(
            "this schedule can never come round: nothing matches it"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::watch::fixtures::owed_long_ago;
    use snob_store::config::WatchConfig;
    use snob_store::store::deliveries;

    /// And neither does a webhook the run refuses.
    ///
    /// `delivery_from` ends in `webhook::check`, and both modes call it before
    /// anything is opened, so an address that could never work costs nothing to
    /// find out about; retention must still happen. A header this tool already
    /// sends is a natural thing to write into `[webhook.headers]`, and it is
    /// refused.
    #[tokio::test]
    async fn a_webhook_the_run_refuses_still_lets_the_queue_settle() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = snob_store::paths::AppPaths::rooted_at(tmp.path());
        let account = paths.account(snob_core::Pk::new(42));
        let now = snob_core::clock::now();
        let id = owed_long_ago(&account, now);

        let secrets = snob_store::secrets::SecretStore::new(paths.clone(), true)
            .with_service(&format!("snob-ig-test-refused-{}", std::process::id()));
        let args = WatchOnceArgs {
            delivery: crate::cli::WebhookArgs {
                webhook: Some("https://receiver.example/hook".to_string()),
                header: vec!["X-Snob-Source: homelab".to_string()],
                sign_with: None,
                heartbeat: false,
                webhook_format: None,
            },
            target: None,
            when_due: false,
            output: crate::cli::StatusOutputArgs { json: false },
            progress: crate::cli::ProgressArgs { no_progress: true },
        };

        let refused = once(args, secrets, &paths, Some(account.pk())).await;
        assert!(
            refused.is_err(),
            "that header is part of what snob sends, so the address is refused"
        );

        let db = snob_store::store::Store::open(&account).unwrap();
        assert_eq!(
            deliveries::state(db.conn(), id).unwrap().as_deref(),
            Some("expired"),
            "the address was refused, and the database still has to be tidied"
        );
    }

    /// `snob watch once` watches what the file says to watch.
    ///
    /// Both modes go through `watched_from`, so there is one answer to "which
    /// accounts" rather than two that disagree. Typing the name instead is no
    /// answer: `Watched::asking` carries no consent, and an unattended run
    /// accepts only a recorded one.
    ///
    /// What this pins is that answer. That `once` asks for it rather than
    /// building its own is not reachable from here — it would take an
    /// integration test that opens a session and a keyring to drive the command
    /// — so it is said in the doc-comment on `once` instead, where somebody
    /// changing it will read it.
    #[test]
    fn once_watches_the_accounts_the_file_lists() {
        let file = WatchConfig {
            schema: 1,
            every: Some(std::time::Duration::from_secs(6 * 3600)),
            accounts: vec![
                snob_store::config::AccountConfig::lists_of("self".to_string(), None, None),
                snob_store::config::AccountConfig::lists_of(
                    "friend".to_string(),
                    None,
                    Some(snob_store::config::ConsentConfig {
                        agreed_at: snob_core::Epoch::new(1_700_000_000),
                    }),
                ),
            ],
            ..Default::default()
        };

        let watched = watched_from(None, Some(&file), None);
        let names: Vec<Option<&str>> = watched.iter().map(|w| w.name()).collect();
        assert_eq!(
            names,
            vec![None, Some("friend")],
            "the file's own account and the one it lists"
        );
        assert!(
            watched.iter().all(|w| w.may_run_unattended()),
            "the recorded consent is what makes the second one legal on a timer"
        );

        // And a name typed on the command line still picks up the answer on
        // record, rather than discarding it and failing every unattended run.
        let typed = watched_from(Some("friend".to_string()), Some(&file), None);
        assert_eq!(typed.len(), 1);
        assert!(typed[0].may_run_unattended());
    }
}
