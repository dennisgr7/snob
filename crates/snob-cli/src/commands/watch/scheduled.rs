//! The scheduled mode: `snob watch` with no subcommand.

use anyhow::Result;
use snob_core::model::printable;
use snob_core::watch::schedule::{self, Due, Schedule};
use snob_core::{Epoch, Pk};
use snob_engine::exit::{ExitCode, ExitError};
use snob_store::config;
use snob_store::paths::AppPaths;
use snob_store::secrets::SecretStore;
use snob_store::store::shared::Shared;

use crate::cli::WatchRunArgs;
use crate::report;
use crate::ui;

use super::automation::{Recorded, any_on_a_schedule, run_due};
use super::delivery::delivery_from;
use super::run::{Printing, account_databases, run_viewers, settle_all};
use super::schedule::{describe_schedule, schedule_from, when_from};
use super::watched::{watched_from, watching_label};

/// Stays up and runs on a schedule until it is stopped.
pub(super) async fn scheduled(
    args: WatchRunArgs,
    secrets: SecretStore,
    paths: &AppPaths,
    in_use: Pk,
) -> Result<ExitCode> {
    // Before anything that can refuse to start.
    settle_all(paths);

    // Read first, so the flags can override it.
    let configured = config::load(paths)?;

    // Nothing in watch.toml to run, and automations made in the app: this
    // runs those alone, each on its own schedule.
    if !super::schedule::typed_a_schedule(&args)
        && args.target.is_none()
        && configured
            .as_ref()
            .is_none_or(super::schedule::names_no_schedule)
        && any_on_a_schedule(paths)
    {
        return only_the_app(&args, &secrets, paths, in_use).await;
    }

    let schedule = schedule_from(&args, configured.as_ref())?;
    let watched = watched_from(args.target.clone(), configured.as_ref(), Some(in_use));
    // Built once and reused, so a service that runs for months holds one
    // connection pool rather than building a TLS stack every few hours.
    let delivery = delivery_from(&args.delivery, configured.as_ref(), &secrets)?;
    // An entry that keeps its videos at their best on a machine with no
    // ffmpeg, refused for the same reason and in the same place.
    super::posts::refuse_what_cannot_be_saved(&watched)?;

    // Refused here rather than at the first tick.
    if let Some(unallowed) = watched.iter().find(|w| !w.may_run_unattended()) {
        return Err(refuse_unattended(unallowed.name().unwrap_or_default()));
    }

    ui::info(&format!(
        "Watching {}. {} Stop with Ctrl+C.",
        watching_label(&watched),
        describe_schedule(&when_from(&args, configured.as_ref()), &schedule, args.now),
    ));

    // Installed once for the process, which is what lets this open an `App` per
    // run without leaving a signal listener behind on each one.
    let cancel = snob_engine::system::interrupt::install();
    let printing = Printing::unattended(args.output.json);
    let (held, entries, to) = (&secrets, &watched, delivery.as_ref());
    let with_progress = !args.progress.no_progress;
    let run = |timing: snob_engine::watch::RunTiming| async move {
        let recorded = Recorded::start(paths, snob_core::automation::WATCH_TOML, timing.began_at);
        let summary = run_viewers(paths, held, entries, to, printing, with_progress, timing).await;
        if let Some(recorded) = recorded {
            recorded.finish(&summary);
        }
        summary
    };

    let mut last_run = seed_for(paths, &schedule, snob_core::clock::now())?;

    if args.now {
        // Run one, here, rather than by pretending nothing has ever run:
        // `last_run = None` means "due immediately" only for an interval, and a
        // calendar would answer with its next moment on the grid.
        last_run = Some(snob_core::clock::now());
        // Asked for at start, by hand: due for nothing.
        let timing = snob_engine::watch::RunTiming::by_hand(snob_core::clock::now());
        if let Err(e) = run(timing).await.into_result() {
            report::print_error(&e, printing.wording());
        }
    }

    // The moment being waited for, the number of runs it stands for, and the
    // jitter roll that shifted it.
    let mut waiting_for: Option<(Epoch, u32, Option<Epoch>)> = None;
    // The previous time round's clock reading, which is how a clock that jumped
    // is told from one that ticked.
    let mut clock_was: Option<Epoch> = None;
    // Whether the run that is due is being held for a critical battery, so it
    // is said once rather than at every look.
    let mut waiting_for_power = false;

    loop {
        let now = snob_core::clock::now();

        // A clock that moved by more than a nap did not tick: it was corrected,
        // or the machine was suspended.
        if let Some(before) = clock_was
            && !(0..=CLOCK_JUMP_SECS).contains(&(now - before))
        {
            waiting_for = None;
        }
        clock_was = Some(now);

        let (wake_at, missed, due_at) = match waiting_for {
            Some(pending) => pending,
            None => {
                let pending = match schedule::due(&schedule, last_run, now, &chrono::Local) {
                    // Already owed, so it goes now.
                    Due::Now { missed } => (
                        now,
                        missed,
                        schedule::due_moment(&schedule, last_run, None, now, &chrono::Local),
                    ),
                    // A variant rather than a sentinel moment, so the compiler
                    // asks for this case instead of an arm order keeping
                    // `wake_at` from saturating.
                    Due::Never => {
                        return Err(anyhow::anyhow!(
                            "this schedule can never come round: nothing matches it"
                        ));
                    }
                    // Rolled once per due moment.
                    //
                    // `wake_at` and not `jittered`: the jitter the banner
                    // printed is measured against the grid, in seconds-of-day,
                    // and a day a zone springs forward through is an hour
                    // shorter than that.
                    Due::At(at) => (
                        schedule::wake_at(&schedule, at, fastrand::f64(), &chrono::Local),
                        0,
                        Some(at),
                    ),
                };
                waiting_for = Some(pending);
                pending
            }
        };

        if now >= wake_at {
            // **A critical battery holds the run back**, and only that: a run
            // started now would be cut off by the system hibernating under it.
            if super::run::waits_for_power(paths, now) {
                if !waiting_for_power {
                    ui::warn(report::BATTERY_CRITICAL_HOLD);
                    waiting_for_power = true;
                }
                if cancel
                    .sleep_or_cancel(std::time::Duration::from_secs(NAP_NEAR_SECS.unsigned_abs()))
                    .await
                {
                    // Nobody is waiting any more, so `snob watch status` must
                    // not go on saying somebody is.
                    super::run::power_is_back(paths);
                    ui::info("Stopped.");
                    return Ok(ExitCode::Interrupted);
                }
                continue;
            }
            if std::mem::take(&mut waiting_for_power) {
                super::run::power_is_back(paths);
            }
            if missed > 0 {
                ui::warn(&missed_warning(missed));
            }
            // Recorded before the work, so a run that fails cannot turn into a
            // tight loop retrying it.
            last_run = Some(now);
            waiting_for = None;
            // The jitter rolled is the schedule's own lateness, not the run's.
            let timing = snob_engine::watch::RunTiming::due(due_at, now, schedule.jitter());

            // A scheduled service does not exit because one run failed.
            if let Err(e) = run(timing).await.into_result() {
                report::print_error(&e, printing.wording());
            }
            // The browser does not sleep through the hours to the next run:
            // what it rotated is written back now, and the browser is left to
            // its owner, which closes it once idle (or closed here, when this
            // process runs its own).
            snob_engine::browser::owner::finish(&secrets, paths).await;
            continue;
        }

        // The automations made in the app, each on its own schedule, looked
        // at every time round: a nap is five minutes at most.
        if run_the_due(
            &secrets,
            paths,
            printing,
            !args.progress.no_progress,
            in_use,
        )
        .await
        {
            continue;
        }

        // Slept in bounded stretches and re-checked against the wall clock each
        // time round, rather than in one long sleep.
        let nap = nap_for(wake_at - now);
        if cancel
            .sleep_or_cancel(std::time::Duration::from_secs(nap))
            .await
        {
            ui::info("Stopped.");
            return Ok(ExitCode::Interrupted);
        }
    }
}

/// Runs the automations made in the app that are due, and says whether any
/// ran, which hands the browsers back to their owner.
async fn run_the_due(
    secrets: &SecretStore,
    paths: &AppPaths,
    printing: Printing,
    with_progress: bool,
    in_use: Pk,
) -> bool {
    let ran = run_due(paths, secrets, printing, with_progress, Some(in_use)).await;
    for summary in &ran {
        if let Some(e) = &summary.failed {
            report::print_error(e, printing.wording());
        }
    }
    if ran.is_empty() {
        return false;
    }
    snob_engine::browser::owner::finish(secrets, paths).await;
    true
}

/// The scheduled mode with nothing of `watch.toml`'s to run: the
/// automations made in the app, looked at every [`NAP_NEAR_SECS`].
async fn only_the_app(
    args: &WatchRunArgs,
    secrets: &SecretStore,
    paths: &AppPaths,
    in_use: Pk,
) -> Result<ExitCode> {
    ui::info(
        "Running the automations made in the app, each on its own schedule. Stop with Ctrl+C.",
    );
    let cancel = snob_engine::system::interrupt::install();
    let printing = Printing::unattended(args.output.json);
    loop {
        run_the_due(secrets, paths, printing, !args.progress.no_progress, in_use).await;
        if cancel
            .sleep_or_cancel(std::time::Duration::from_secs(NAP_NEAR_SECS.unsigned_abs()))
            .await
        {
            ui::info("Stopped.");
            return Ok(ExitCode::Interrupted);
        }
    }
}

/// The refusal a scheduled run raises for an account nobody confirmed.
fn refuse_unattended(name: &str) -> anyhow::Error {
    let shown = printable(name);
    ExitError::new(
        ExitCode::Interrupted,
        format!(
            "reading @{shown}'s lists needs confirmation, and a scheduled run has nobody to \
             ask.\nRun \"snob watch setup\" to answer it once, or \"snob watch once {shown}\" \
             while you are here."
        ),
    )
    .into()
}

/// What the monitor says about the runs it was not running for.
///
/// **One is the ordinary case**, so the sentence has a singular form: a machine
/// off for a day on a daily schedule misses exactly one, and `missed_since`
/// takes one off the end because the moment being served is itself in the past,
/// so one is what a laptop that was shut overnight produces.
fn missed_warning(missed: u32) -> String {
    if missed == 1 {
        "1 scheduled run was missed while this was not running. It is not replayed: there is \
         only one present state, so there is nothing to catch up on"
            .to_string()
    } else {
        format!(
            "{missed} scheduled runs were missed while this was not running. They are reported \
             as one: there is only one present state, so there is nothing to catch up on"
        )
    }
}

/// How long to sleep before looking at the clock again, given how far the next
/// run is.
fn nap_for(remaining: i64) -> u64 {
    const FAR: i64 = 10 * 60;
    let ceiling = if remaining > FAR {
        NAP_FAR_SECS
    } else {
        NAP_NEAR_SECS
    };
    remaining.clamp(1, ceiling) as u64
}

/// The two nap ceilings [`nap_for`] chooses between.
const NAP_NEAR_SECS: i64 = 60;
const NAP_FAR_SECS: i64 = 5 * 60;

/// How far the clock may move between two turns of the loop and still be
/// ticking.
const CLOCK_JUMP_SECS: i64 = 2 * NAP_FAR_SECS;

// The two orderings the loop leans on, held where they cannot be argued
// with: the jump threshold sits clear above the longest nap (or every
// ordinary turn reads as a suspend), and no nap may cross the floor between
// runs (or the loop could sleep through one).
const _: () = assert!(CLOCK_JUMP_SECS > NAP_FAR_SECS);
const _: () = assert!(NAP_FAR_SECS < snob_core::watch::schedule::MIN_GAP_SECS);

/// When the monitor last started a run, for any account: the latest across every
/// account's database, and the start every run writes in `shared.db` before its
/// work (`run::started`), which is there even when no account got as far as its
/// row.
pub(super) fn last_started(paths: &AppPaths) -> Result<Option<Epoch>> {
    let mut latest = Shared::read_existing(paths)?
        .map(|shared| shared.monitor_last_started())
        .transpose()?
        .flatten();
    // Read, never written (`Store::read_existing`): `watch status` asks this
    // too, and says it writes nothing.
    for account in account_databases(paths) {
        let Some(store) = snob_store::store::Store::read_existing(&account)? else {
            continue;
        };
        latest = latest.max(snob_store::store::watch::last_started(store.conn())?);
    }
    Ok(latest)
}

/// The seed when the run log is empty: none on a calendar, `now` otherwise,
/// because the answer depends on what kind of schedule it is:
///
/// - **An interval** has no moments of its own, so `None` means "due now" and a
///   fresh install would walk the second it was set up.
/// - **A calendar** has its own moments, and inventing a run for it is harmful.
fn seed_last_run(schedule: &Schedule, now: Epoch) -> Option<Epoch> {
    (!schedule.is_on_a_calendar()).then_some(now)
}

/// What the loop starts its clock from.
///
/// `last_started` reads `watch_runs`, whose only writer is a tick that
/// finished, so an invented `now` held only in a variable would, while that log
/// stays empty, **measure every start's interval from that start**.
pub(super) fn seed_as_stored(paths: &AppPaths, schedule: &Schedule) -> Result<Option<Epoch>> {
    if let Some(recorded) = last_started(paths)? {
        return Ok(Some(recorded));
    }
    if schedule.is_on_a_calendar() {
        return Ok(None);
    }
    let shared = snob_store::store::shared::Shared::read_existing(paths)?;
    if let Some(seeded) = shared
        .as_ref()
        .map(|shared| shared.interval_seeded_at())
        .transpose()?
    {
        return Ok(seeded);
    }
    let mut kept_before = None;
    for account in account_databases(paths) {
        let Some(store) = snob_store::store::Store::read_existing(&account)? else {
            continue;
        };
        if let Some(at) = snob_store::store::watch::interval_seeded_at(store.conn())? {
            kept_before = Some(kept_before.map_or(at, |earliest: Epoch| earliest.min(at)));
        }
    }
    Ok(kept_before)
}

pub(super) fn seed_for(paths: &AppPaths, schedule: &Schedule, now: Epoch) -> Result<Option<Epoch>> {
    if let Some(recorded) = last_started(paths)? {
        return Ok(Some(recorded));
    }
    let Some(invented) = seed_last_run(schedule, now) else {
        return Ok(None);
    };

    let shared = Shared::open(paths)?;
    if let Some(seeded) = shared.interval_seeded_at()? {
        return Ok(Some(seeded));
    }
    let mut kept_before = None;
    for account in account_databases(paths) {
        let Some(store) = snob_store::store::Store::open_existing(&account)? else {
            continue;
        };
        if let Some(at) = snob_store::store::watch::interval_seeded_at(store.conn())? {
            kept_before = Some(kept_before.map_or(at, |earliest: Epoch| earliest.min(at)));
        }
    }
    Ok(Some(shared.seed_interval(kept_before.unwrap_or(invented))?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Near the moment the loop steps by the minute; far from it, by five.
    /// Neither step can carry the loop past the moment itself.
    #[test]
    fn the_nap_is_short_near_the_moment_and_long_far_from_it() {
        assert_eq!(nap_for(0), 1, "never a zero-second sleep");
        assert_eq!(nap_for(-5), 1, "a moment already past is looked at now");
        assert_eq!(nap_for(30), 30, "bounded by the distance when close");
        assert_eq!(nap_for(90), 60, "a minute at most inside the last ten");
        assert_eq!(nap_for(10 * 60), 60, "the last ten minutes are near");
        assert_eq!(nap_for(10 * 60 + 1), 5 * 60, "past them, five minutes");
        assert_eq!(nap_for(6 * 3600), 5 * 60);
    }

    /// An interval measures from the first start, not from this one, while
    /// the run log is still empty.
    #[test]
    fn an_interval_measures_from_the_first_start_and_not_from_this_one() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let every = Schedule::every(std::time::Duration::from_secs(24 * 3_600)).unwrap();

        let first = seed_for(&paths, &every, Epoch::new(1_700_000_000)).unwrap();
        assert_eq!(first, Some(Epoch::new(1_700_000_000)));

        // A second start two hours later, with the run log still empty.
        let second = seed_for(&paths, &every, Epoch::new(1_700_007_200)).unwrap();
        assert_eq!(
            second, first,
            "a restart must not put the interval back to zero"
        );
    }

    /// A seed an account's database kept from before is the one kept, and it
    /// is copied once: the monitor's clock is not put back to zero by moving
    /// to one seed for every account.
    #[test]
    fn a_seed_an_account_kept_before_is_carried_over() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let every = Schedule::every(std::time::Duration::from_secs(24 * 3_600)).unwrap();
        for (pk, seeded) in [(42, 1_700_000_500), (43, 1_700_000_000)] {
            let store = snob_store::store::Store::open(&paths.account(Pk::new(pk))).unwrap();
            store
                .conn()
                .execute(
                    "INSERT INTO watch_state (key, value) VALUES ('interval_seeded_at', ?1)",
                    [seeded],
                )
                .unwrap();
        }

        assert_eq!(
            seed_for(&paths, &every, Epoch::new(1_700_007_200)).unwrap(),
            Some(Epoch::new(1_700_000_000)),
            "the earliest seed an account kept"
        );
        assert_eq!(
            Shared::open(&paths).unwrap().interval_seeded_at().unwrap(),
            Some(Epoch::new(1_700_000_000)),
            "and it is kept where every account reads it"
        );
    }

    /// The monitor last ran when any account last ran.
    #[test]
    fn the_last_run_is_the_latest_across_every_account() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        for (pk, started) in [(42, 1_700_000_000), (43, 1_700_003_600)] {
            let pk = Pk::new(pk);
            let store = snob_store::store::Store::open(&paths.account(pk)).unwrap();
            crate::commands::watch::fixtures::known_account(&store, pk, "me", true);
            snob_store::store::watch::record_run(
                store.conn(),
                &snob_store::store::watch::Run {
                    account_pk: pk,
                    started_at: Epoch::new(started),
                    finished_at: Some(Epoch::new(started)),
                    requests: 0,
                    outcome: None,
                    changes: 0,
                    posts_skipped: None,
                    due_at: None,
                    began_at: None,
                },
            )
            .unwrap();
        }

        assert_eq!(
            last_started(&paths).unwrap(),
            Some(Epoch::new(1_700_003_600))
        );
    }

    /// A run that wrote no row in any account's database still counts: its
    /// start, written before the work, moves the schedule on.
    #[test]
    fn a_run_that_recorded_nothing_still_moves_the_schedule_on() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let every = Schedule::every(std::time::Duration::from_secs(6 * 3_600)).unwrap();
        let started = Epoch::new(1_700_000_000);
        Shared::open(&paths)
            .unwrap()
            .monitor_started(started)
            .unwrap();

        assert_eq!(last_started(&paths).unwrap(), Some(started));
        let a_look_later = started + std::time::Duration::from_secs(15 * 60);
        let seed = seed_for(&paths, &every, a_look_later).unwrap();
        assert_eq!(seed, Some(started));
        assert!(
            matches!(
                schedule::due(&every, seed, a_look_later, &chrono::Utc),
                schedule::Due::At(_)
            ),
            "the next look a quarter of an hour on is not due"
        );
    }

    /// A recorded run wins over the seed, for an interval and for a calendar.
    #[test]
    fn a_recorded_run_wins_over_the_seed() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let pk = Pk::new(42);
        let store = snob_store::store::Store::open(&paths.account(pk)).unwrap();
        crate::commands::watch::fixtures::known_account(&store, pk, "me", true);
        snob_store::store::watch::record_run(
            store.conn(),
            &snob_store::store::watch::Run {
                account_pk: pk,
                started_at: Epoch::new(1_700_000_000),
                finished_at: Some(Epoch::new(1_700_000_000)),
                requests: 0,
                outcome: None,
                changes: 0,
                posts_skipped: None,
                due_at: None,
                began_at: None,
            },
        )
        .unwrap();

        let every = Schedule::every(std::time::Duration::from_secs(24 * 3_600)).unwrap();
        let at_nine = Schedule::calendar(&[], &[schedule::parse_time("09:00").unwrap()]).unwrap();
        for schedule in [every, at_nine] {
            assert_eq!(
                seed_for(&paths, &schedule, Epoch::new(1_700_007_200)).unwrap(),
                Some(Epoch::new(1_700_000_000)),
                "{schedule:?}"
            );
        }
    }

    /// And a calendar still seeds nothing, because inventing a run for one
    /// steps over every moment in the next quarter of an hour.
    #[test]
    fn a_calendar_is_not_seeded() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let at_nine = Schedule::calendar(&[], &[schedule::parse_time("09:00").unwrap()]).unwrap();

        assert_eq!(
            seed_for(&paths, &at_nine, Epoch::new(1_700_000_000)).unwrap(),
            None
        );
    }

    /// Both slots of the refusal take the same name, so both must be filtered.
    ///
    /// The account comes from `watch.toml`, which nothing validates, and the
    /// refusal is the one place the monitor prints it before any client
    /// exists — the earliest a hostile name can reach a terminal.
    #[test]
    fn the_unattended_refusal_filters_the_name_in_both_slots() {
        let message = refuse_unattended("gh\u{1b}[2K\u{1b}[A").to_string();

        assert!(
            !message.chars().any(|c| c.is_control() && c != '\n'),
            "{message:?} is printed to a terminal and written to the journal"
        );
        assert_eq!(
            message.matches("gh[2K[A").count(),
            2,
            "the account and the command that answers for it name the same person: {message}"
        );
    }

    /// A fresh install does not step over the first moment its calendar names:
    /// a seed of `Some(now)` would put the floor at `now + MIN_GAP_SECS`.
    #[test]
    fn a_fresh_install_keeps_the_first_moment_its_calendar_names() {
        // Midnight UTC on a Monday, so the wall clock is checkable by hand.
        const MONDAY_0000: i64 = 1_786_924_800;
        let at = |secs: i64| Epoch::new(MONDAY_0000 + secs);

        let calendar = Schedule::cron("0 9 * * *").unwrap();
        let ten_to_nine = at(8 * 3600 + 50 * 60);

        assert_eq!(
            seed_last_run(&calendar, ten_to_nine),
            None,
            "a calendar has its own moments and needs no invented run"
        );
        assert_eq!(
            schedule::due(&calendar, None, ten_to_nine, &chrono::Utc),
            schedule::Due::At(at(9 * 3600)),
            "nine o'clock is ten minutes away, which is inside the minimum gap"
        );

        // An interval is the other way round: with no past to measure from it
        // would be due immediately, and a fresh install must not walk the second
        // it is set up.
        let interval = Schedule::every(std::time::Duration::from_secs(6 * 3600)).unwrap();
        assert_eq!(seed_last_run(&interval, ten_to_nine), Some(ten_to_nine));
    }

    /// The monitor's own sentence about its gaps reads as a sentence for one,
    /// the case it prints most often.
    #[test]
    fn the_missed_warning_reads_as_a_sentence_for_one() {
        let one = missed_warning(1);
        assert!(one.contains("1 scheduled run was missed"), "{one}");
        assert!(!one.contains("runs were"), "{one}");
        assert!(!one.contains("They are"), "{one}");

        let several = missed_warning(4);
        assert!(
            several.contains("4 scheduled runs were missed"),
            "{several}"
        );

        // Both have to say the part that matters, which is that they are not
        // replayed: firing twelve to catch up is the burst the pacing exists to
        // prevent, and they would all report the same present state anyway.
        for text in [one, several] {
            assert!(text.contains("nothing to catch up on"), "{text}");
        }
    }
}
