//! `snob watch status`: what is configured, and whether it is working.
//!
//! `health` is where that answer is decided, in `status`'s output **and** in its
//! exit code, so a machine and a person are told the same thing.

use std::collections::BTreeMap;

use anyhow::Result;
use snob_core::model::{ListKind, printable};
use snob_core::watch::{RunOutcome, schedule};
use snob_core::{Epoch, Pk};
use snob_engine::app::Viewer;
use snob_engine::exit::ExitCode;
use snob_engine::watch::Watched;
use snob_engine::watch::check::Verdict;
use snob_store::config::{self, WatchConfig};
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::registry::Registry;
use snob_store::store::shared::PowerWait;
use snob_store::store::{Store, deliveries, watch as watch_store};

use crate::cli::WatchStatusArgs;
use crate::report;

use super::schedule::schedule_from;
use super::watched::watched_from;

/// Reports what is configured and what has happened.
pub fn status(
    args: WatchStatusArgs,
    paths: &AppPaths,
    account: Option<&AccountPaths>,
) -> Result<ExitCode> {
    let config = config::load(paths)?;
    let recorded = recorded(paths, config.as_ref(), account.map(|a| a.pk()))?;
    let owed = recorded.owed;

    let now = snob_core::clock::now();
    let health = health(
        config.as_ref(),
        &recorded.runs,
        &recorded.reported,
        owed,
        now,
        waiting_for_power(paths),
    );
    let next = config
        .as_ref()
        .and_then(|config| next_run(paths, config, now));

    if args.output.json {
        let document = super::wire::status_json(
            config.is_some(),
            &config::path(paths),
            &owed,
            &health,
            &recorded.runs,
            &recorded.marks,
            &recorded.posts,
            next,
        );
        crate::ui::say!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(health.verdict.exit_code());
    }

    match &config {
        Some(config) => {
            crate::ui::say!("Configured in {}", config::path(paths).display());
            for line in describe_config(config) {
                crate::ui::say!("  {line}");
            }
            if let Some(next) = next {
                crate::ui::say!("  {}", next_line(next));
            }
        }
        None => crate::ui::say!(
            "Nothing is configured. Run \"snob watch setup\", or pass the schedule on the \
             command line."
        ),
    }

    for line in recorded.lines() {
        crate::ui::say!("{line}");
    }

    // Two lines, about two different sets of rows: `pending` is every row in the
    // state, while only the ones addressed here are ever handed back.
    if owed.waiting > 0 || owed.elsewhere > 0 || owed.given_up > 0 {
        crate::ui::say!();
    }
    if owed.given_up > 0 {
        // `say::given_up_sentence`, so the run's warning and this line cannot
        // drift apart over the one event they both describe.
        crate::ui::say!("{}", super::say::given_up_sentence(owed.given_up));
    }
    if owed.waiting > 0 {
        let (subject, it) = if owed.waiting == 1 {
            ("report is", "it")
        } else {
            ("reports are", "them")
        };
        crate::ui::say!(
            "{} {subject} waiting to be delivered; the next run tries {it}.",
            owed.waiting
        );
    }
    if owed.elsewhere > 0 {
        // The verb is in the tuple with everything else it has to agree with:
        // exactly one report left over after the webhook address moved is the
        // ordinary case.
        let (subject, they, expire, their, it) = if owed.elsewhere == 1 {
            ("report is", "It", "expires", "its", "it")
        } else {
            ("reports are", "They", "expire", "their", "them")
        };
        crate::ui::say!(
            "{} {subject} addressed to a webhook this configuration does not send to, so \
             nothing here will try {it}. {they} {expire} on {their} own.",
            owed.elsewhere
        );
    }

    if !health.notes.is_empty() {
        crate::ui::say!();
        crate::ui::say!("Health: {}", health.verdict.as_str());
        for note in &health.notes {
            crate::ui::say!("  {note}");
        }
    }

    // Non-zero when the monitor is not doing what it was configured to do, so
    // this is usable as a probe rather than only as something to read.
    Ok(health.verdict.exit_code())
}

/// Whether the monitor is doing what it was configured to do.
pub(super) struct Health {
    pub(super) verdict: Verdict,
    pub(super) notes: Vec<String>,
}

/// The accounts the configuration has `viewer` watch right now, as ids, with
/// `db` being `viewer`'s database.
fn watched_pks(db: &Store, watched: Option<&[Watched]>, viewer: Pk) -> Result<Option<Vec<Pk>>> {
    let Some(watched) = watched else {
        return Ok(None);
    };
    let mut pks = Vec::new();
    for entry in watched {
        match (entry.viewer(), entry.name()) {
            // Read as the account in use, and nobody is signed in.
            (None, _) => return Ok(None),
            (Some(other), _) if other != viewer => {}
            (Some(_), None) => pks.push(viewer),
            (Some(_), Some(named)) => {
                if let Some(pk) =
                    snob_store::store::accounts::find_pk_by_username(db.conn(), named)?
                {
                    pks.push(pk);
                }
            }
        }
    }
    Ok(Some(pks))
}

/// One account's newest run, with the two things [`health`] cannot work out for
/// itself: what to call the account, and whether the file still names it.
pub(super) struct RunOf {
    pub(super) run: watch_store::Run,
    /// `@name`, or the id when the name was never learned.
    who: String,
    /// Whether `watch.toml` still lists this account.
    watched: bool,
    /// The account whose database recorded it: the one the run read as.
    pub(super) viewer: Viewer,
}

/// One receipt, with what to call its account and whose database holds it.
pub(super) struct MarkOf {
    pub(super) mark: watch_store::AccountMark,
    who: String,
    pub(super) viewer: Viewer,
}

/// The receipt for one account's posts, with what to call it, whose database
/// holds it, and how many of its new posts still owe a file.
pub(super) struct PostsOf {
    pub(super) mark: snob_store::store::watch_posts::Mark,
    who: String,
    pub(super) viewer: Viewer,
    pub(super) owed: usize,
}

/// What every account's database says the monitor did.
///
/// **Every database, not the account in use's.**
#[derive(Default)]
struct Recorded {
    /// The accounts read, in the order their databases were.
    viewers: Vec<Viewer>,
    /// What every account owes, added up.
    owed: deliveries::Owed,
    runs: Vec<RunOf>,
    marks: Vec<MarkOf>,
    posts: Vec<PostsOf>,
    reported: Vec<HalfRead>,
}

/// Reads what every account with a database recorded.
fn recorded(
    paths: &AppPaths,
    config: Option<&WatchConfig>,
    in_use: Option<Pk>,
) -> Result<Recorded> {
    // The address this configuration could post to, spelled the way the outbox
    // records it — `status` builds no delivery of its own, so it goes through
    // the same function `delivery_from` does rather than comparing the file's
    // raw string against a normalized URL.
    let destination = config
        .and_then(|c| c.webhook.as_ref())
        .and_then(|w| url::Url::parse(&w.url).ok())
        .map(|url| super::delivery::destination_of(&url));
    let watched = config.map(|config| watched_from(None, Some(config), in_use));
    let registry = Registry::load(paths).unwrap_or_default();

    let mut recorded = Recorded::default();
    for account in super::run::account_databases(paths) {
        let viewer = Viewer {
            pk: account.pk(),
            username: snob_engine::account::username(&registry, account.pk()).map(str::to_string),
        };
        // Gone since it was listed: `purge` removed it meanwhile.
        let Some(db) = Store::read_existing(&account)? else {
            continue;
        };
        recorded.read(&db, viewer, watched.as_deref(), destination.as_deref())?;
    }
    Ok(recorded)
}

impl Recorded {
    /// Adds what `db`, `viewer`'s database, recorded.
    fn read(
        &mut self,
        db: &Store,
        viewer: Viewer,
        watched: Option<&[Watched]>,
        destination: Option<&str>,
    ) -> Result<()> {
        let conn = db.conn();
        let owed = deliveries::owed(conn, destination)?;
        self.owed.waiting += owed.waiting;
        self.owed.elsewhere += owed.elsewhere;
        self.owed.given_up += owed.given_up;

        // Resolved once per account, here where the store is open, so every
        // line names an account the same way.
        let mut labels: BTreeMap<Pk, String> = BTreeMap::new();
        let mut who = |pk: Pk| -> Result<String> {
            if let Some(label) = labels.get(&pk) {
                return Ok(label.clone());
            }
            let name = snob_store::store::users::name(conn, pk)?;
            let label = snob_engine::sentences::label(pk, name.as_deref());
            labels.insert(pk, label.clone());
            Ok(label)
        };

        // Per account, and reported per account: a run covers every configured
        // one, so one unqualified "last ran" line is whichever account happened
        // to be last in the file.
        let named = watched_pks(db, watched, viewer.pk)?;
        for run in watch_store::last_runs(conn)? {
            self.runs.push(RunOf {
                who: who(run.account_pk)?,
                watched: named
                    .as_ref()
                    .is_none_or(|pks| pks.contains(&run.account_pk)),
                viewer: viewer.clone(),
                run,
            });
        }

        // Which accounts have had exactly one of their two lists reported on.
        let mut seen: BTreeMap<Pk, Vec<ListKind>> = BTreeMap::new();
        for mark in watch_store::all_marks(conn)? {
            seen.entry(mark.account_pk).or_default().push(mark.kind);
            self.marks.push(MarkOf {
                who: who(mark.account_pk)?,
                viewer: viewer.clone(),
                mark,
            });
        }
        for mark in snob_store::store::watch_posts::all_marks(conn)? {
            self.posts.push(PostsOf {
                who: who(mark.account_pk)?,
                viewer: viewer.clone(),
                owed: snob_store::store::watch_posts::pending_saves(conn, mark.account_pk)?.len(),
                mark,
            });
        }
        for (pk, kinds) in seen {
            self.reported.push(HalfRead {
                who: who(pk)?,
                reported: kinds[0],
                missing: (kinds.len() == 1).then(|| match kinds[0] {
                    ListKind::Followers => ListKind::Following,
                    ListKind::Following => ListKind::Followers,
                }),
            });
        }

        self.viewers.push(viewer);
        Ok(())
    }

    /// The runs and receipts, under the account each was read as.
    fn lines(&self) -> Vec<String> {
        if self.viewers.is_empty() {
            return vec![String::new(), "It has not run yet.".to_string()];
        }
        let mut lines = Vec::new();
        for viewer in &self.viewers {
            lines.push(String::new());
            lines.push(format!("As {}:", viewer.label()));
            // Said before the marks, because it answers the question somebody
            // opening `status` actually has.
            let mut ran = false;
            for RunOf { run, who, .. } in self.runs.iter().filter(|of| of.viewer.pk == viewer.pk) {
                ran = true;
                let mut line = format!(
                    "  {who} last ran on {}",
                    snob_engine::sentences::stored_on(run.started_at)
                );
                if let Some(outcome) = &run.outcome
                    && *outcome != RunOutcome::Ok
                {
                    line.push_str(&format!(" and could not look ({outcome})"));
                } else if run.changes == 0 {
                    line.push_str(" and found nothing");
                } else {
                    line.push_str(&format!(
                        " and found {} change{}",
                        run.changes,
                        plural(run.changes as usize)
                    ));
                }
                lines.push(format!("{line}."));
                if let Some(timing) = timing_of(run) {
                    let late = timing
                        .late_by()
                        .filter(|_| timing.late())
                        .map(|late| {
                            format!(
                                ", {} late",
                                snob_core::duration::format(std::time::Duration::from_secs(
                                    late.unsigned_abs()
                                ))
                            )
                        })
                        .unwrap_or_default();
                    lines.push(format!(
                        "    It was due on {} and began on {}{late}.",
                        snob_engine::sentences::stored_on(run.due_at.unwrap_or_default()),
                        snob_engine::sentences::stored_on(timing.began_at),
                    ));
                }
            }
            if !ran {
                lines.push("  It has not run yet.".to_string());
            }

            let mut reported = false;
            for MarkOf { mark, who, .. } in self.marks.iter().filter(|of| of.viewer.pk == viewer.pk)
            {
                reported = true;
                lines.push(format!(
                    "  {who}: {} last reported on {}",
                    mark.kind,
                    snob_engine::sentences::stored_on(mark.compared_at)
                ));
            }
            for PostsOf {
                mark, who, owed, ..
            } in self.posts.iter().filter(|of| of.viewer.pk == viewer.pk)
            {
                reported = true;
                let last = match mark.reported_at {
                    Some(at) => format!(
                        "a new one last reported on {}",
                        snob_engine::sentences::stored_on(at)
                    ),
                    None => "none new reported yet".to_string(),
                };
                let mut line = format!(
                    "  {who}: posts seen since {}, last read on {}, {last}",
                    snob_engine::sentences::stored_on(mark.baseline_at),
                    snob_engine::sentences::stored_on(mark.read_at),
                );
                if *owed > 0 {
                    line.push_str(&format!(
                        "; {owed} still owe{} a file, saved by a run that finds {} on its pages",
                        if *owed == 1 { "s" } else { "" },
                        if *owed == 1 { "it" } else { "them" }
                    ));
                }
                lines.push(format!("{line}."));
            }
            if !reported {
                lines.push("  It has not reported on anything yet.".to_string());
            }
        }
        lines
    }
}

/// How many scheduled runs may be missed before silence is a failure rather
/// than a warning.
const MISSED_BEFORE_FAILED: i64 = 3;

/// When a recorded run was due and began, when it was recorded with both: what
/// `status` and its `--json` tell a late run by.
pub(super) fn timing_of(run: &watch_store::Run) -> Option<snob_engine::watch::RunTiming> {
    Some(snob_engine::watch::RunTiming {
        ended_at: run.finished_at,
        ..snob_engine::watch::RunTiming::due(
            Some(run.due_at?),
            run.began_at?,
            std::time::Duration::ZERO,
        )
    })
}

/// How often the timer `watch unit` prints asks whether a run is due: a
/// quarter of an hour (`unit::ASK_EVERY_MINUTES`), so a run under it starts
/// up to that late, on time.
const TIMER_ASKS_EVERY_SECS: i64 = 15 * 60;

/// How long this configuration means the monitor may be silent for.
///
/// **The widest gap in a cycle, not the distance between the next two
/// moments.**
fn expected_gap(config: &WatchConfig, now: Epoch) -> Option<i64> {
    /// Far enough to see a whole week's pattern, and one day over so a weekly
    /// schedule is measured rather than truncated.
    const HORIZON_SECS: i64 = 8 * 24 * 3600;
    /// A backstop for a schedule that fires often enough to make the horizon
    /// expensive. Two hundred steps of `--every 5m` is under a day, and a
    /// uniform grid has already given its answer by step one.
    const MOST_STEPS: usize = 200;

    let schedule = schedule_from(&Default::default(), Some(config)).ok()?;
    let first = schedule::next_after(&schedule, Some(now), now, &chrono::Local)?;

    let mut at = first;
    let mut widest = 0;
    for _ in 0..MOST_STEPS {
        let Some(next) = schedule::next_after(&schedule, Some(at), at, &chrono::Local) else {
            break;
        };
        widest = widest.max(next - at);
        at = next;
        if at - first >= HORIZON_SECS {
            break;
        }
    }
    (widest > 0).then_some(widest)
}

/// An account with exactly one of its two lists ever reported on.
struct HalfRead {
    who: String,
    reported: ListKind,
    /// The one that never has been, or `None` when both have.
    missing: Option<ListKind>,
}

/// The due run the monitor holds back for a critical battery, as the run
/// loop wrote it down; `None` when it is not, or nothing can tell.
fn waiting_for_power(paths: &AppPaths) -> Option<PowerWait> {
    let shared = snob_store::store::shared::Shared::read_existing(paths).ok()??;
    shared.waiting_for_power().ok()?
}

fn health(
    config: Option<&WatchConfig>,
    runs: &[RunOf],
    reported: &[HalfRead],
    owed: deliveries::Owed,
    now: Epoch,
    waiting_for_power: Option<PowerWait>,
) -> Health {
    let mut notes = Vec::new();
    let mut verdict = Verdict::Ok;
    let mut at_least = |level: Verdict| verdict = verdict.max(level);

    if config.is_none() {
        at_least(Verdict::Warned);
        notes.push(crate::report::NOTHING_CONFIGURED.to_string());
    }

    if runs.is_empty() {
        at_least(Verdict::Warned);
        notes.push("it has not run yet".to_string());
    }

    // **One list being read while the other never is.**
    //
    // Every run, forever, on the account AGENTS.md files under "Known walls":
    // `following` meets the truncation wall on every walk while `followers`
    // completes.
    for half in reported.iter().filter(|r| r.missing.is_some()) {
        let (read, never) = (half.reported, half.missing.expect("filtered"));
        at_least(Verdict::Warned);
        notes.push(format!(
            "{}: {read} has been reported on and {never} never has, so one of the two \
             lists is not being read",
            half.who
        ));
    }

    // A schedule the scheduler refuses is a monitor that cannot start.
    //
    // **A file that names no schedule is not one of those**: the schedule can
    // come from the command line, as `snob watch --every 6h` run as a service
    // takes it, and calling that broken made a working monitor exit 1.
    if let Some(config) = config
        && super::schedule::names_no_schedule(config)
    {
        at_least(Verdict::Warned);
        notes.push(
            "the file names no schedule, so it runs only where one is given on the command \
             line, as \"snob watch --every 6h\""
                .to_string(),
        );
    } else if let Some(config) = config
        && let Err(e) = schedule_from(&Default::default(), Some(config))
    {
        at_least(Verdict::Failed);
        notes.push(format!("the configured schedule cannot be built: {e}"));
    }

    // And an address the delivery refuses is a monitor that cannot finish:
    // `delivery_from` runs before anything is opened or spent, so a bad address
    // kills every run before `record_failed_run` can file one, and the table
    // stays empty.
    if let Some(webhook) = config.and_then(|c| c.webhook.as_ref())
        && let Some(problem) =
            snob_engine::watch::webhook::problem_with_config(&webhook.url, &webhook.headers)
    {
        at_least(Verdict::Failed);
        notes.push(problem);
    }

    // **Whether it is still running at all.**
    let gap = config.and_then(|c| expected_gap(c, now));
    let waiting_for_power =
        waiting_for_power.filter(|wait| gap.is_none_or(|gap| now - wait.seen <= gap));
    if let Some(wait) = waiting_for_power {
        at_least(Verdict::Warned);
        notes.push(format!(
            "the run that is due is waiting for power: the battery has been critical since {}",
            snob_engine::sentences::stored_on(wait.since)
        ));
    } else if let Some(gap) = gap
        && let Some(newest) = runs.iter().map(|r| r.run.started_at).max()
    {
        // A run on time may still start this much after its moment: the jitter
        // only ever delays it, and the timer `watch unit` prints asks once a
        // quarter of an hour.
        let slack = config
            .and_then(|c| schedule_from(&Default::default(), Some(c)).ok())
            .map_or(0, |schedule| schedule.jitter().as_secs() as i64)
            + TIMER_ASKS_EVERY_SECS;
        let silent = (now - newest - slack).max(0);
        let missed = silent / gap.max(1);
        if missed >= MISSED_BEFORE_FAILED {
            at_least(Verdict::Failed);
            notes.push(format!(
                "it has not run since {}, which is {missed} scheduled runs ago",
                snob_engine::sentences::stored_on(newest)
            ));
        } else if missed >= 1 {
            at_least(Verdict::Warned);
            notes.push(format!(
                "it has not run since {}, and one was due by now",
                snob_engine::sentences::stored_on(newest)
            ));
        }
    }

    // What stopped the last run of each account.
    //
    // **Only for accounts the file still names.**
    for RunOf {
        run, who, watched, ..
    } in runs
    {
        // Posts that a run could not look at for a reason that does not lift
        // by itself, while its lists were read and the outcome says `ok`.
        if *watched
            && let Some(why) = run.posts_skipped.as_deref()
            && matches!(why, "hidden" | "no_browser")
        {
            at_least(Verdict::Warned);
            notes.push(format!(
                "{who}'s posts could not be read on its last run ({})",
                if why == "hidden" {
                    "the account is private and not followed"
                } else {
                    "SNOB_NO_BROWSER is set, and a grid is read from the browser"
                }
            ));
        }
        // A last run that began well after the moment it was due for: the
        // machine slept through it, the timer missed it, or it waited for the
        // budget or a cooldown.
        if *watched
            && let Some(timing) = timing_of(run).filter(|t| t.late())
            && let Some(late) = timing.late_by()
        {
            at_least(Verdict::Warned);
            notes.push(format!(
                "{who}'s last run began {} after the moment it was due for",
                snob_core::duration::format(std::time::Duration::from_secs(late.unsigned_abs()))
            ));
        }
        let Some(code) = run.outcome.as_ref().filter(|c| **c != RunOutcome::Ok) else {
            continue;
        };
        if !watched {
            notes.push(format!(
                "{who} last ended in {code}, and the configuration no longer names it"
            ));
            continue;
        }
        // **Matched as an outcome, not against literals spelled again here**:
        // `as_str`'s own doc says the vocabulary exists "so nothing has to
        // invent tokens inline, which is how two spellings of one condition get
        // shipped".
        match code.known() {
            // A cooldown lifts by itself and an interrupt was the user.
            Some(RunOutcome::RateLimited | RunOutcome::Interrupted) => {
                at_least(Verdict::Warned);
                notes.push(format!("{who}'s last run ended in {code}"));
            }
            _ => {
                at_least(Verdict::Failed);
                notes.push(format!("{who}'s last run ended in {code}"));
            }
        }
    }

    // **Which queue it is, asked of the queue**, not of one `pending` count or
    // of the file: a monitor whose `[webhook] url` moved must not be scored on
    // rows `due` can never return, and a run given the address on the command
    // line from a unit with no `watch.toml` (the README's own example) has rows
    // with a real address the next run drains.
    if owed.elsewhere > 0 {
        if config.and_then(|c| c.webhook.as_ref()).is_some() {
            // The file names an address and these are for a different one, so
            // nothing will ever post them: they expire where they are, and the
            // changes in them are already marked as reported, which makes them
            // the only copy.
            at_least(Verdict::Failed);
            notes.push(format!(
                "{} queued report(s) are addressed somewhere this configuration does not send \
                 to, so nothing here will try them",
                owed.elsewhere
            ));
        } else {
            // With no address in the file, a run given `--webhook` and a
            // `[webhook]` somebody deleted look identical from here, and
            // guessing in the alarming direction would fail the supported one.
            at_least(Verdict::Warned);
            notes.push(format!(
                "{} queued report(s) are addressed to a webhook this file does not name, so \
                 only a run given --webhook can send them",
                owed.elsewhere
            ));
        }
    }

    if owed.waiting > 0 {
        at_least(Verdict::Warned);
        notes.push(format!(
            "{} report(s) still waiting to be delivered",
            owed.waiting
        ));
    }

    // A report given up on is not a failure of the monitor -- it is usually a
    // receiver that was down for a day -- but it must not be silent, and it
    // must not be the thing that lets the verdict go green: counting only
    // `pending` would turn `warning` into `ok` the moment the change was thrown
    // away.
    if owed.given_up > 0 {
        at_least(Verdict::Warned);
        notes.push(format!(
            "{} report(s) were given up on, too old, out of attempts or with no request that \
             could be sent",
            owed.given_up
        ));
    }

    Health { verdict, notes }
}

/// When the schedule in the file next comes round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NextRun {
    /// On the grid or past the floor at this moment, before any jitter.
    At(Epoch),
    /// Already owed: the next start of the monitor, or its timer, runs it.
    Now,
    /// An interval whose clock no run or start has begun: one interval
    /// after the monitor first starts.
    AfterFirstStart(std::time::Duration),
}

/// The next run, asked of the evaluator the scheduled mode sleeps on
/// (`schedule::due`) from the last run it would start from
/// (`scheduled::seed_as_stored`, read and never written).
fn next_run(paths: &AppPaths, config: &WatchConfig, now: Epoch) -> Option<NextRun> {
    let schedule = schedule_from(&Default::default(), Some(config)).ok()?;
    let last_run = super::scheduled::seed_as_stored(paths, &schedule).ok()?;
    if last_run.is_none() && !schedule.is_on_a_calendar() {
        return config.every.map(NextRun::AfterFirstStart);
    }
    let asked_since = snob_store::store::shared::Shared::read_existing(paths)
        .ok()
        .flatten()
        .and_then(|shared| shared.asked_since().ok().flatten());
    let due = match asked_since {
        Some(asked) => schedule::due_asked_since(&schedule, last_run, asked, now, &chrono::Local),
        None => schedule::due(&schedule, last_run, now, &chrono::Local),
    };
    match due {
        schedule::Due::At(at) => Some(NextRun::At(at)),
        schedule::Due::Now { .. } => Some(NextRun::Now),
        schedule::Due::Never => None,
    }
}

fn next_line(next: NextRun) -> String {
    match next {
        NextRun::At(at) => format!("Next run: {}.", snob_engine::sentences::stored_on(at)),
        NextRun::Now => {
            "Next run: due now, at the next start of the monitor or of its timer.".to_string()
        }
        NextRun::AfterFirstStart(every) => format!(
            "Next run: {} after the monitor first starts.",
            snob_core::duration::format(every)
        ),
    }
}

/// The configuration in a few lines, for `status` and for the confirmation
/// `setup` shows before replacing a file.
pub(super) fn describe_config(config: &WatchConfig) -> Vec<String> {
    let mut lines = Vec::new();

    let when = report::schedule_clauses(&config.spec());
    lines.push(if when.is_empty() {
        "no schedule: it will not run until one is set".to_string()
    } else {
        format!("Runs {}", when.join(", "))
    });

    // What the file says, not what a `Schedule` would clamp it to: this is the
    // place a person reads their configuration back, a hand-edited
    // `jitter = "1h"` included.
    if let Some(jitter) = config.jitter
        && let Some(sentence) = report::jitter_sentence(jitter)
    {
        lines.push(sentence);
    }

    // Redacted, not just filtered: `webhook::check` refuses an address carrying
    // a password because it would be echoed by `status`.
    let address = config
        .webhook
        .as_ref()
        .map(|webhook| printable(&snob_engine::watch::webhook::shown(&webhook.url)));

    match (&config.webhook, &address) {
        (Some(webhook), Some(address)) => {
            lines.push(format!("Reports to {address}"));
            if webhook.heartbeat {
                lines.push("Sends a report even when nothing changed".to_string());
            }
        }
        // What the **file** says, which is not the whole of where a report can
        // go.
        _ => lines.push(
            "No address here: the report goes to standard output unless a run is given \
             --webhook"
                .to_string(),
        ),
    }

    lines.push(watching_line(config));

    // The two facts above are one fact, not two unrelated lines: somebody
    // else's names leaving this machine on every run is the part of this
    // configuration a person would most want to be reminded of.
    if let Some(address) = &address
        && config.accounts.iter().any(|a| !a.is_own())
    {
        lines.push(format!(
            "Their usernames and names go to {address} with every report"
        ));
    }

    lines
}

/// Who a run over this file would actually walk.
///
/// **Asked of `watched_from`, which is the function that decides it**, so the
/// sentence cannot describe a run that does something else.
fn watching_line(config: &WatchConfig) -> String {
    let watched = watched_from(None, Some(config), None);
    // A `self` is its viewer's own account, so each viewer's counts once.
    let own = watched
        .iter()
        .filter(|w| w.name().is_none())
        .map(Watched::viewer)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let others = watched.iter().filter(|w| w.name().is_some()).count();

    let yours = match own {
        0 => {
            return format!(
                "Watches {others} account{}, and not your own",
                plural(others)
            );
        }
        1 => "your account".to_string(),
        n => format!("{n} of your accounts"),
    };
    match others {
        0 => format!("Watches {yours}"),
        n => format!("Watches {yours} and {n} other{}", plural(n)),
    }
}

pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::watch::fixtures::{known_account, watch_toml};
    use snob_core::Pk;
    use snob_core::watch::RecordedOutcome;
    use std::time::Duration;

    /// The one address in the file is shown twice when somebody else is
    /// watched, and both showings are the cleaned one. The second was the
    /// raw string, so a hand-written `user:pass@` -- which `check` refuses
    /// and `status` does not run -- reached the terminal after the first
    /// line had taken care to hide it.
    #[test]
    fn a_password_in_the_address_is_hidden_from_every_line() {
        let lines = describe_config(&watch_toml(
            "schema = 1
every = \"6h\"

[webhook]
url = \"https://me:hunter2@n8n.local/hook\"

[[account]]
target = \"someone\"
",
        ));
        let text = lines.join(
            "
",
        );
        assert!(text.contains("go to"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("n8n.local/hook"), "{text}");
    }

    #[test]
    fn it_describes_an_interval_and_a_webhook() {
        let lines = describe_config(&watch_toml(
            "schema = 1\nevery = \"6h\"\n\n[webhook]\nurl = \"https://n8n.local/hook\"\nheartbeat = true\n",
        ));
        let text = lines.join("\n");
        assert!(text.contains("every 6h"), "{text}");
        assert!(text.contains("https://n8n.local/hook"), "{text}");
        assert!(text.contains("even when nothing changed"), "{text}");
    }

    /// A file with no webhook is a whole configuration, and saying so is what
    /// stops somebody thinking the delivery is broken.
    #[test]
    fn it_says_when_nothing_is_sent_anywhere() {
        let lines = describe_config(&watch_toml("schema = 1\nevery = \"6h\"\n"));
        assert!(lines.join("\n").contains("standard output"));
    }

    /// A hand-edited file can have neither half of a schedule, and then the
    /// monitor never runs. Saying "Runs" with nothing after it would read as
    /// though it were fine.
    #[test]
    fn a_file_with_no_schedule_says_it_will_not_run() {
        let lines = describe_config(&watch_toml("schema = 1\n"));
        assert!(lines[0].contains("will not run"), "{lines:?}");
    }

    #[test]
    fn it_counts_the_other_accounts() {
        let lines = describe_config(&watch_toml(
            "schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"self\"\n\n\
             [[account]]\ntarget = \"someone\"\n[account.consent]\nagreed_at = 1\n",
        ));
        assert!(
            lines.join("\n").contains("your account and 1 other"),
            "{lines:?}"
        );
    }

    /// A file that names one stranger is walked as that stranger alone:
    /// `watched_from` falls back to the viewer only when the list is empty. Both
    /// of the two places a person reads the configuration back said otherwise,
    /// because this counted the strangers and then claimed the viewer anyway.
    #[test]
    fn it_does_not_claim_your_account_when_the_file_does_not_list_it() {
        let lines = describe_config(&watch_toml(
            "schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"friend\"\n\
             [account.consent]\nagreed_at = 1\n",
        ));
        let text = lines.join("\n");

        assert!(!text.contains("your account"), "{text}");
        assert!(text.contains("1 account, and not your own"), "{text}");
    }

    /// The sentence names the accounts a run would actually walk, over every
    /// shape the file can take.
    ///
    /// The rule that makes it hard: the viewer is added only when the account
    /// list is empty, so a file naming one stranger is walked as that stranger
    /// alone.
    ///
    /// The expected sentences are written out rather than computed from
    /// `watched_from`, which would make this agree with itself whatever either
    /// side did. Written out, a change to `watched_from` moves the sentence and
    /// fails here -- which is the whole property.
    #[test]
    fn the_line_names_the_accounts_a_run_would_actually_walk() {
        let friend = "[account.consent]\nagreed_at = 1\n";
        for (body, expected) in [
            (
                "schema = 1\nevery = \"6h\"\n".to_string(),
                "Watches your account",
            ),
            (
                "schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"self\"\n".to_string(),
                "Watches your account",
            ),
            (
                format!("schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"friend\"\n{friend}"),
                "Watches 1 account, and not your own",
            ),
            (
                format!(
                    "schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"self\"\n\n\
                     [[account]]\ntarget = \"friend\"\n{friend}"
                ),
                "Watches your account and 1 other",
            ),
            (
                format!(
                    "schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"@friend\"\n{friend}\n\
                     [[account]]\ntarget = \"other\"\n{friend}"
                ),
                "Watches 2 accounts, and not your own",
            ),
            (
                format!(
                    "schema = 2
every = \"6h\"

[[account]]
target = \"self\"
viewer = 1

                     [[account]]
target = \"self\"
viewer = 2

                     [[account]]
target = \"friend\"
viewer = 2
{friend}"
                ),
                "Watches 2 of your accounts and 1 other",
            ),
        ] {
            let file = watch_toml(&body);
            assert_eq!(watching_line(&file), expected, "for {body:?}");
        }
    }

    /// A file with no `[[account]]` at all means the obvious thing, and this is
    /// the one shape where the old wording happened to be right by accident --
    /// it printed nothing.
    #[test]
    fn a_file_naming_nobody_watches_the_viewer() {
        let lines = describe_config(&watch_toml("schema = 1\nevery = \"6h\"\n"));
        assert!(
            lines.join("\n").contains("Watches your account"),
            "{lines:?}"
        );
    }

    /// The one setting somebody could write into the file and never see again.
    ///
    /// `status` and the replace-this-file confirmation are where a person reads
    /// their configuration back, and `WatchConfig.jitter` reached neither.
    #[test]
    fn a_configured_jitter_is_read_back() {
        let lines = describe_config(&watch_toml("schema = 1\nevery = \"6h\"\njitter = \"1h\"\n"));
        assert!(
            lines.join("\n").contains("pushed up to 1h later"),
            "{lines:?}"
        );

        let none = describe_config(&watch_toml("schema = 1\nevery = \"6h\"\njitter = \"0\"\n"));
        assert!(
            !none.join("\n").contains("pushed up to"),
            "a jitter that was turned off has nothing to say: {none:?}"
        );
    }

    /// A name written with an at sign still names the account it watches.
    ///
    /// `status` settles which accounts the file names by looking each one up by
    /// username. Were `target = "@friend"` to find nobody, every run of that
    /// account would be scored as belonging to an account the configuration no
    /// longer names -- deliberately only a note, never a verdict -- and a
    /// monitor whose one watched account fails every run would exit 0 forever.
    #[test]
    fn a_name_written_with_an_at_sign_is_still_an_account_the_file_watches() {
        let db = Store::in_memory().unwrap();
        known_account(&db, Pk::new(7), "friend", false);

        let edited =
            watch_toml("schema = 1\nevery = \"6h\"\n\n[[account]]\ntarget = \"@friend\"\n");
        let watched = watched_from(None, Some(&edited), Some(Pk::new(42)));
        assert_eq!(
            watched_pks(&db, Some(&watched), Pk::new(42)).unwrap(),
            Some(vec![Pk::new(7)]),
            "the file names @friend and the store knows friend; they are one account"
        );
    }

    /// `self` is the account its entry is read as, which is not necessarily the
    /// one this database belongs to.
    #[test]
    fn self_is_the_account_its_entry_is_read_as() {
        let db = Store::in_memory().unwrap();
        let file = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "self"
viewer = 1

[[account]]
target = "self"
"#,
        );

        let signed_in = watched_from(None, Some(&file), Some(Pk::new(3)));
        assert_eq!(
            watched_pks(&db, Some(&signed_in), Pk::new(1)).unwrap(),
            Some(vec![Pk::new(1)])
        );
        assert_eq!(
            watched_pks(&db, Some(&signed_in), Pk::new(3)).unwrap(),
            Some(vec![Pk::new(3)]),
            "the second names no viewer, so it is the account in use"
        );
        let nobody = watched_from(None, Some(&file), None);
        assert_eq!(
            watched_pks(&db, Some(&nobody), Pk::new(1)).unwrap(),
            None,
            "with nobody signed in, whose the second is cannot be settled"
        );
    }

    /// A fixed present, so how old a run is is something these tests state
    /// rather than something they inherit from the wall clock.
    const NOW: Epoch = Epoch::new(1_700_000_000);

    /// A run that happened a minute ago, not at the epoch: a run that old under
    /// a six-hourly schedule is a monitor that stopped.
    fn ran(outcome: ExitCode) -> watch_store::Run {
        watch_store::Run {
            account_pk: Pk::new(42),
            started_at: NOW - Duration::from_secs(60),
            finished_at: Some(NOW - Duration::from_secs(60)),
            requests: 1,
            // The conversion `commit` writes through, not a token spelled here:
            // a fixture spelling the literals `health` matched on would move
            // with any misspelling.
            outcome: Some(outcome.into()),
            changes: 0,
            posts_skipped: None,
            due_at: None,
            began_at: None,
        }
    }

    /// A run belonging to an account the file still names.
    fn of(run: &watch_store::Run) -> RunOf {
        RunOf {
            run: run.clone(),
            who: "@me".to_string(),
            watched: true,
            viewer: me(),
        }
    }

    fn me() -> Viewer {
        Viewer {
            pk: Pk::new(42),
            username: Some("me".into()),
        }
    }

    /// Which states count as a monitor that has stopped doing its job.
    ///
    /// What is pinned here is the line between "working" and "not", because
    /// that is what an exit code is.
    #[test]
    fn a_healthy_monitor_is_told_apart_from_one_that_has_stopped_working() {
        let configured = watch_toml(
            "schema = 1
every = \"6h\"

[webhook]
url = \"https://n8n.internal/hook\"
",
        );

        let ok = ran(ExitCode::Ok);
        assert_eq!(
            health(
                Some(&configured),
                &[of(&ok)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Ok,
            "configured, ran just now, nothing owed"
        );

        // A cooldown lifts on its own, and an interrupt was the user. Neither
        // is a monitor that needs attention.
        for lifts in [ExitCode::RateLimited, ExitCode::Interrupted] {
            let run = ran(lifts);
            assert_eq!(
                health(
                    Some(&configured),
                    &[of(&run)],
                    &[],
                    deliveries::Owed::default(),
                    NOW,
                    None
                )
                .verdict,
                Verdict::Warned,
                "{lifts:?} passes on its own"
            );
        }

        // A session that has gone will not come back without somebody logging
        // in, and every run until then does nothing at all.
        let dead = ran(ExitCode::NoSession);
        assert_eq!(
            health(
                Some(&configured),
                &[of(&dead)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Failed
        );

        // Owed reports this configuration could post are a wait.
        assert_eq!(
            health(
                Some(&configured),
                &[of(&ok)],
                &[],
                deliveries::Owed {
                    waiting: 2,
                    elsewhere: 0,
                    given_up: 0,
                },
                NOW,
                None
            )
            .verdict,
            Verdict::Warned
        );
        // Addressed to somewhere this file does not name is a different
        // question: a file with no address cannot tell a run given --webhook
        // from a [webhook] somebody deleted.
        // `a_report_addressed_elsewhere_is_not_owed_to_this_run` holds both
        // directions down.
        let no_webhook = watch_toml(
            "schema = 1
every = \"6h\"
",
        );
        assert_eq!(
            health(
                Some(&no_webhook),
                &[of(&ok)],
                &[],
                deliveries::Owed {
                    waiting: 0,
                    elsewhere: 2,
                    given_up: 0,
                },
                NOW,
                None
            )
            .verdict,
            Verdict::Warned,
            "a file with no address cannot tell --webhook from a deleted section"
        );

        // And a machine with nothing configured is not broken, but a bare
        // `snob watch` there has no schedule to run on.
        let nothing = health(None, &[], &[], deliveries::Owed::default(), NOW, None);
        assert_eq!(nothing.verdict, Verdict::Warned);
        assert_eq!(nothing.notes.len(), 2, "{:?}", nothing.notes);
    }

    /// **An address that kills every run before it starts is a failure, not a
    /// quiet month.**
    ///
    /// `delivery_from` runs before anything is opened or spent, so a bad
    /// address means `record_failed_run` never files a row and `watch_runs`
    /// stays empty, which alone reads as "it has not run yet".
    #[test]
    fn an_address_that_can_never_work_is_a_failure_and_not_a_quiet_monitor() {
        // No scheme. Exactly what a hand-edited file looks like, and the file
        // invites hand-editing on its first line.
        let no_scheme = watch_toml(
            "schema = 1
every = \"6h\"

[webhook]
url = \"n8n.local/hook\"
",
        );
        let found = health(
            Some(&no_scheme),
            &[],
            &[],
            deliveries::Owed::default(),
            NOW,
            None,
        );
        assert_eq!(
            found.verdict,
            Verdict::Failed,
            "an unusable address with an empty run log: {:?}",
            found.notes
        );
        assert_eq!(found.verdict.exit_code(), ExitCode::Error);

        // A credential in the address is the other refusal `webhook::check`
        // makes, and it reaches this probe by the same route.
        let in_the_url = watch_toml(
            "schema = 1
every = \"6h\"

[webhook]
url = \"https://user:pw@example.com/hook\"
",
        );
        assert_eq!(
            health(
                Some(&in_the_url),
                &[],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Failed
        );

        // And an address that is merely unreachable is not this probe's
        // question -- it cannot be answered without posting.
        let fine = watch_toml(
            "schema = 1
every = \"6h\"

[webhook]
url = \"https://example.com/hook\"
",
        );
        let ok = health(
            Some(&fine),
            &[],
            &[],
            deliveries::Owed::default(),
            NOW,
            None,
        );
        assert_eq!(ok.verdict, Verdict::Warned, "{:?}", ok.notes);
    }

    /// What stopped the last run is read in the vocabulary exit codes are
    /// written in, not in three literals spelled again inside `health`.
    ///
    /// Respelling one would score a monitor in an ordinary cooldown `Failed`,
    /// so `status` would exit 1 about something that resumes by itself.
    ///
    /// Walked over `ExitCode::ALL`, so a code added later has to be placed
    /// deliberately rather than defaulted into `Failed` by nobody mentioning it.
    #[test]
    fn what_stopped_the_last_run_is_read_in_the_tokens_exit_codes_are_written_in() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");
        let verdict_of = |run: &watch_store::Run| {
            health(
                Some(&configured),
                &[of(run)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None,
            )
            .verdict
        };

        for code in ExitCode::ALL {
            let run = ran(code);
            let expected = match code {
                ExitCode::Ok => Verdict::Ok,
                ExitCode::RateLimited | ExitCode::Interrupted => Verdict::Warned,
                _ => Verdict::Failed,
            };
            assert_eq!(
                verdict_of(&run),
                expected,
                "a run recorded as {code:?} ({})",
                code.as_str()
            );
        }

        // Named on its own as well as inside the walk, because this is the one
        // the mapping can lose without the walk noticing: drop a code from `ALL`
        // and the loop simply stops testing it.
        let cooled = ran(ExitCode::RateLimited);
        assert_eq!(
            verdict_of(&cooled),
            Verdict::Warned,
            "a cooldown lifts by itself; a probe that pages for one is a probe people switch off"
        );

        // And a row spelled by something that is not this build is a failure it
        // cannot explain, rather than a quiet `Ok`. It is an `Unknown` and not a
        // near miss for `rate_limited`: nothing here guesses at a token a newer
        // build wrote.
        let unknown = watch_store::Run {
            outcome: Some(RecordedOutcome::Unknown("rate-limited".into())),
            ..ran(ExitCode::Ok)
        };
        assert_eq!(
            verdict_of(&unknown),
            Verdict::Failed,
            "a token this build does not know is not a healthy run"
        );
    }

    /// The two probes say the same thing about a machine with no `watch.toml`.
    ///
    /// It is the advice a newly installed tool gives, so it is the one somebody
    /// edits, and two probes run one after the other must not disagree about
    /// the same machine.
    ///
    /// One run is given so `status` has nothing else to say: the only note left
    /// is the one under test, which is what makes this an equality rather than a
    /// search for a substring.
    #[test]
    fn both_probes_say_the_same_thing_about_an_unconfigured_machine() {
        let ok = ran(ExitCode::Ok);
        let status = health(
            None,
            &[of(&ok)],
            &[],
            deliveries::Owed::default(),
            NOW,
            None,
        );
        assert_eq!(status.notes.len(), 1, "{:?}", status.notes);

        let check = snob_engine::watch::check::without_a_session(None, None, NOW);
        assert_eq!(check.checked.len(), 1, "{:?}", check.checked);

        assert_eq!(
            status.notes[0],
            super::super::say::problem_line(
                check.checked[0]
                    .problem
                    .as_ref()
                    .expect("the check has to say why nothing is configured")
            ),
            "two probes somebody runs one after the other, disagreeing about one machine"
        );
    }

    /// A queue for an address the file does not name is not a monitor with
    /// nowhere to send, and one for an address it *has* moved away from is.
    ///
    /// A run given the address on the command line from a unit with no
    /// `watch.toml` (the README's own systemd example) has rows with a real
    /// address the next run drains, while a `[webhook] url` moved to a new host
    /// with the old queue still standing really does lose changes.
    #[test]
    fn a_report_addressed_elsewhere_is_not_owed_to_this_run() {
        let ok = ran(ExitCode::Ok);
        let two_elsewhere = deliveries::Owed {
            waiting: 0,
            elsewhere: 2,
            given_up: 0,
        };

        // The address moved. Those rows can never go, and the changes in them
        // are already marked as reported, so they are the only copy.
        let moved = watch_toml(
            "schema = 1\nevery = \"6h\"\n\n[webhook]\nurl = \"https://n8n.new.local/hook\"\n",
        );
        let stranded = health(Some(&moved), &[of(&ok)], &[], two_elsewhere, NOW, None);
        assert_eq!(stranded.verdict, Verdict::Failed, "{:?}", stranded.notes);

        // The same rows with no `[webhook]` in the file are the shape the README
        // leads with: the address is on the command line and the next run drains
        // them. A guess in the alarming direction is what costs a probe its
        // credibility.
        let from_the_flag = watch_toml("schema = 1\nevery = \"6h\"\n");
        let fine = health(
            Some(&from_the_flag),
            &[of(&ok)],
            &[],
            two_elsewhere,
            NOW,
            None,
        );
        assert_eq!(fine.verdict, Verdict::Warned, "{:?}", fine.notes);
        assert_eq!(fine.verdict.exit_code(), ExitCode::Ok);
        assert!(
            fine.notes.iter().any(|n| n.contains("--webhook")),
            "and it has to say what would send them: {:?}",
            fine.notes
        );

        // And the file that says where reports go says so out loud, because
        // "Sends nothing" was printed about a monitor that delivers on every
        // run.
        assert!(
            !describe_config(&from_the_flag)
                .join("\n")
                .contains("Sends nothing"),
            "the file names no address; that is not the same as sending nothing"
        );
    }

    /// A file whose schedule the scheduler refuses is a monitor that cannot
    /// start, and both read-only probes say so.
    ///
    /// Every one of these gets into the file through a hand-edit, which the
    /// first line of `watch.toml` says is fine, and every one passes
    /// `config::parse` — which reads TOML, the schema number and one key clash.
    #[test]
    fn a_schedule_the_scheduler_refuses_is_not_healthy() {
        for refused in [
            "schema = 1\nevery = \"5m\"\n",
            "schema = 1\ncron = \"0 9 * *\"\n",
            "schema = 1\nat = [\"25:00\"]\n",
            // Not `every = "2w"` beside `on = ["mon"]`: that is the shape the
            // README leads with, and it builds
            // (`one_monday_in_every_two_is_what_the_readme_says_it_is`).
        ] {
            let configured = watch_toml(refused);
            let ok = ran(ExitCode::Ok);
            let health = health(
                Some(&configured),
                &[of(&ok)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None,
            );
            assert_eq!(
                health.verdict,
                Verdict::Failed,
                "{refused:?} kills every run: {:?}",
                health.notes
            );
        }

        // And one that builds is still fine.
        let good = watch_toml("schema = 1\nevery = \"6h\"\n");
        let ok = ran(ExitCode::Ok);
        assert_eq!(
            health(
                Some(&good),
                &[of(&ok)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Ok
        );
    }

    /// One list being read while the other never is.
    ///
    /// `watch_runs.outcome` is one column for a tick covering two lists, and
    /// `TickReport::looked()` asks `any` rather than `all` — so a run where
    /// followers completed and following was refused records `ok`. Every run,
    /// forever, on the account AGENTS.md files under "Known walls".
    ///
    /// Asked of the marks, because that is where the durable answer already
    /// is: a mark moves only when a list was actually compared.
    #[test]
    fn one_list_never_being_read_is_not_a_healthy_monitor() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");
        let ok = ran(ExitCode::Ok);

        let half = HalfRead {
            who: "@me".to_string(),
            reported: ListKind::Followers,
            missing: Some(ListKind::Following),
        };
        let blind = health(
            Some(&configured),
            &[of(&ok)],
            &[half],
            deliveries::Owed::default(),
            NOW,
            None,
        );
        assert_eq!(blind.verdict, Verdict::Warned, "{:?}", blind.notes);
        assert!(
            blind
                .notes
                .iter()
                .any(|n| n.contains("following") && n.contains("@me")),
            "the line has to name the list and the account: {:?}",
            blind.notes
        );

        // Both read is the ordinary case and says nothing.
        let whole = HalfRead {
            who: "@me".to_string(),
            reported: ListKind::Followers,
            missing: None,
        };
        assert_eq!(
            health(
                Some(&configured),
                &[of(&ok)],
                &[whole],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Ok
        );
    }

    /// A monitor that stopped is not a healthy monitor.
    ///
    /// A successful run three weeks old is not `Ok`, and `prune` keeps the
    /// newest row per account whatever its age, so it never ages into "it has
    /// not run yet" either. That is the likeliest real failure of an unattended
    /// service: a unit disabled, a container nobody restarted, a process the
    /// kernel killed.
    #[test]
    fn a_monitor_that_stopped_is_not_healthy() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");

        let recent = ran(ExitCode::Ok);
        assert_eq!(
            health(
                Some(&configured),
                &[of(&recent)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Ok
        );

        // One six-hour gap missed is a laptop that was shut.
        let late = watch_store::Run {
            started_at: NOW - Duration::from_secs(7 * 3_600),
            ..ran(ExitCode::Ok)
        };
        let one = health(
            Some(&configured),
            &[of(&late)],
            &[],
            deliveries::Owed::default(),
            NOW,
            None,
        );
        assert_eq!(one.verdict, Verdict::Warned, "{:?}", one.notes);

        // Three weeks is nobody coming back.
        let gone = watch_store::Run {
            started_at: NOW - Duration::from_secs(21 * 86_400),
            ..ran(ExitCode::Ok)
        };
        let stopped = health(
            Some(&configured),
            &[of(&gone)],
            &[],
            deliveries::Owed::default(),
            NOW,
            None,
        );
        assert_eq!(stopped.verdict, Verdict::Failed, "{:?}", stopped.notes);
        assert!(
            stopped
                .notes
                .iter()
                .any(|n| n.contains("has not run since")),
            "and it has to say so: {:?}",
            stopped.notes
        );
    }

    /// A monitor holding its run for a critical battery is doing what it
    /// should: said as such, and not counted late however long it waits.
    #[test]
    fn a_monitor_waiting_for_power_is_not_late() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");
        let gone = watch_store::Run {
            started_at: NOW - Duration::from_secs(2 * 86_400),
            ..ran(ExitCode::Ok)
        };
        let waiting = health(
            Some(&configured),
            &[of(&gone)],
            &[],
            deliveries::Owed::default(),
            NOW,
            Some(PowerWait {
                since: NOW - Duration::from_secs(3_600),
                seen: NOW - Duration::from_secs(60),
            }),
        );
        assert_eq!(waiting.verdict, Verdict::Warned, "{:?}", waiting.notes);
        assert!(
            waiting
                .notes
                .iter()
                .any(|n| n.contains("waiting for power")),
            "{:?}",
            waiting.notes
        );
        assert!(
            !waiting
                .notes
                .iter()
                .any(|n| n.contains("has not run since")),
            "{:?}",
            waiting.notes
        );
    }

    /// A marker nobody has looked at for a whole period was left by a monitor
    /// that stopped while it waited: it is late like any other, not waiting.
    #[test]
    fn a_marker_left_behind_does_not_hide_a_dead_monitor() {
        let configured = watch_toml(
            "schema = 1
every = \"6h\"
",
        );
        let gone = watch_store::Run {
            started_at: NOW - Duration::from_secs(2 * 86_400),
            ..ran(ExitCode::Ok)
        };
        let left_behind = health(
            Some(&configured),
            &[of(&gone)],
            &[],
            deliveries::Owed::default(),
            NOW,
            Some(PowerWait {
                since: NOW - Duration::from_secs(43_200),
                seen: NOW - Duration::from_secs(7 * 3_600),
            }),
        );
        assert_eq!(
            left_behind.verdict,
            Verdict::Failed,
            "{:?}",
            left_behind.notes
        );
        assert!(
            !left_behind
                .notes
                .iter()
                .any(|n| n.contains("waiting for power")),
            "{:?}",
            left_behind.notes
        );
        assert!(
            left_behind
                .notes
                .iter()
                .any(|n| n.contains("has not run since")),
            "{:?}",
            left_behind.notes
        );
    }

    /// How late is late comes from the schedule, so a weekly monitor is not
    /// called late after two days.
    #[test]
    fn how_late_is_late_depends_on_the_schedule() {
        let two_days_ago = watch_store::Run {
            started_at: NOW - Duration::from_secs(2 * 86_400),
            ..ran(ExitCode::Ok)
        };

        let weekly = watch_toml("schema = 1\non = [\"mon\"]\nat = [\"09:00\"]\n");
        assert_eq!(
            health(
                Some(&weekly),
                &[of(&two_days_ago)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Ok,
            "two days is not late for a weekly schedule"
        );

        let six_hourly = watch_toml("schema = 1\nevery = \"6h\"\n");
        assert_ne!(
            health(
                Some(&six_hourly),
                &[of(&two_days_ago)],
                &[],
                deliveries::Owed::default(),
                NOW,
                None
            )
            .verdict,
            Verdict::Ok,
            "and it very much is for a six-hourly one"
        );
    }

    /// **A schedule that is not a uniform grid, which is what the wizard
    /// suggests.**
    ///
    /// The test above uses one weekly moment and `every = "6h"` — both
    /// uniform, so both have one gap. Answer the two prompts with the examples
    /// they print and the gaps are 12h, 60h, 12h and 84h; reading the first one
    /// would call a working monitor three runs late for two days out of every
    /// seven.
    #[test]
    fn an_uneven_schedule_is_measured_by_its_widest_gap_and_not_its_narrowest() {
        let uneven = watch_toml(
            "schema = 1
on = [\"mon\", \"thu\"]
at = [\"09:00\", \"21:00\"]
",
        );
        let gap = expected_gap(&uneven, NOW).expect("the schedule builds and fires");
        assert!(
            gap >= 80 * 3600,
            "the widest gap in this week is 84h; got {}h",
            gap / 3600
        );

        // Twice a day is still twice a day: the widest gap is the overnight
        // one, not the twelve hours between the two the wizard prints.
        let daily_pair = watch_toml(
            "schema = 1
at = [\"09:00\", \"10:00\"]
",
        );
        let gap = expected_gap(&daily_pair, NOW).expect("fires");
        assert!(
            gap >= 22 * 3600,
            "09:00 and 10:00 leaves 23 hours overnight; got {}h",
            gap / 3600
        );

        // And a uniform grid is unchanged, which is the half that already
        // worked and must keep working.
        let uniform = watch_toml(
            "schema = 1
every = \"6h\"
",
        );
        assert_eq!(expected_gap(&uniform, NOW), Some(6 * 3600));
    }

    /// A run belonging to an account the file no longer names is worth a line,
    /// not a verdict.
    ///
    /// `last_runs` answers about every account that has ever run, and one old
    /// failure for a stranger since removed must not pin the verdict at
    /// `Failed` for good. The note names the account, so several do not print
    /// one sentence N times.
    #[test]
    fn a_run_for_an_account_nobody_watches_any_more_does_not_fail_the_verdict() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");
        let failed = ran(ExitCode::NoSession);

        let orphan = RunOf {
            run: failed,
            who: "@stranger".to_string(),
            watched: false,
            viewer: me(),
        };
        let health = health(
            Some(&configured),
            &[orphan],
            &[],
            deliveries::Owed::default(),
            NOW,
            None,
        );

        assert_eq!(health.verdict, Verdict::Ok, "{:?}", health.notes);
        assert!(
            health.notes.iter().any(|n| n.contains("@stranger")),
            "the line has to say which account: {:?}",
            health.notes
        );
    }

    /// What each account recorded is read from its own database and shown
    /// under it, and the JSON names the account on every row.
    #[test]
    fn status_groups_what_each_account_recorded_by_viewer() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        for (pk, name) in [(1, "one"), (2, "two")] {
            let db = Store::open(&paths.account(Pk::new(pk))).unwrap();
            known_account(&db, Pk::new(pk), name, true);
            watch_store::record_run(
                db.conn(),
                &watch_store::Run {
                    account_pk: Pk::new(pk),
                    ..ran(ExitCode::Ok)
                },
            )
            .unwrap();
        }
        let file = watch_toml(
            "schema = 2\nevery = \"6h\"\n\n[[account]]\ntarget = \"self\"\nviewer = 1\n\n\
             [[account]]\ntarget = \"self\"\nviewer = 2\n",
        );

        let recorded = recorded(&paths, Some(&file), Some(Pk::new(1))).unwrap();

        assert_eq!(recorded.runs.len(), 2);
        assert!(recorded.runs.iter().all(|of| of.watched), "both are named");
        let lines = recorded.lines();
        let at = |line: &str| {
            lines
                .iter()
                .position(|l| l == line)
                .unwrap_or_else(|| panic!("no {line:?} in {lines:?}"))
        };
        assert!(at("As account 1:") < at("As account 2:"), "{lines:?}");
        assert!(lines[at("As account 1:") + 1].starts_with("  @one last ran"));
        assert!(lines[at("As account 2:") + 1].starts_with("  @two last ran"));

        let health = health(
            Some(&file),
            &recorded.runs,
            &recorded.reported,
            recorded.owed,
            NOW,
            None,
        );
        let document = super::super::wire::status_json(
            true,
            std::path::Path::new("watch.toml"),
            &recorded.owed,
            &health,
            &recorded.runs,
            &recorded.marks,
            &recorded.posts,
            None,
        );
        let viewers: Vec<_> = document["last_runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| (run["pk"].clone(), run["viewer"]["pk"].clone()))
            .collect();
        assert_eq!(
            viewers,
            [(1.into(), 1.into()), (2.into(), 2.into())],
            "{document}"
        );
    }

    /// A run a little past its gap, by no more than the jitter and the
    /// timer's quarter of an hour, is on time.
    #[test]
    fn a_run_late_by_the_jitter_and_the_timer_is_not_called_late() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\njitter = \"15m\"\n");
        let just_late = watch_store::Run {
            started_at: NOW - Duration::from_secs(6 * 3_600 + 25 * 60),
            ..ran(ExitCode::Ok)
        };
        let found = health(
            Some(&configured),
            &[of(&just_late)],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Ok, "{:?}", found.notes);

        let late = watch_store::Run {
            started_at: NOW - Duration::from_secs(6 * 3_600 + 31 * 60),
            ..ran(ExitCode::Ok)
        };
        let found = health(
            Some(&configured),
            &[of(&late)],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Warned, "{:?}", found.notes);
    }

    /// A last run that began well after its moment is a note, and said
    /// beside the run; one on time, or by hand, is not.
    #[test]
    fn a_run_that_began_late_is_noted() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");
        let late = watch_store::Run {
            due_at: Some(NOW - Duration::from_secs(3 * 3_600)),
            began_at: Some(NOW - Duration::from_secs(60)),
            ..ran(ExitCode::Ok)
        };
        let found = health(
            Some(&configured),
            &[of(&late)],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Warned, "{:?}", found.notes);
        assert!(
            found
                .notes
                .iter()
                .any(|n| n.contains("after the moment it was due for")),
            "{:?}",
            found.notes
        );

        let on_time = watch_store::Run {
            due_at: Some(NOW - Duration::from_secs(120)),
            began_at: Some(NOW - Duration::from_secs(60)),
            ..ran(ExitCode::Ok)
        };
        let found = health(
            Some(&configured),
            &[of(&on_time)],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Ok, "{:?}", found.notes);
    }

    /// A file with no schedule is a warning, not a monitor that cannot start:
    /// the schedule can come from the command line.
    #[test]
    fn a_file_with_no_schedule_is_a_warning() {
        let bare = watch_toml("schema = 2\n");
        let found = health(
            Some(&bare),
            &[of(&ran(ExitCode::Ok))],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Warned, "{:?}", found.notes);
        assert!(
            found.notes.iter().any(|n| n.contains("names no schedule")),
            "{:?}",
            found.notes
        );
    }

    /// A run that read the lists and could not look at the posts, for a
    /// reason that does not lift by itself, is worth a note though its
    /// outcome is `ok`; one that skipped them within the hour is not.
    #[test]
    fn posts_a_run_could_not_read_are_noted() {
        let configured = watch_toml("schema = 1\nevery = \"6h\"\n");
        let hidden = watch_store::Run {
            posts_skipped: Some("hidden".into()),
            ..ran(ExitCode::Ok)
        };
        let found = health(
            Some(&configured),
            &[of(&hidden)],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Warned);
        assert!(
            found
                .notes
                .iter()
                .any(|n| n.contains("posts could not be read")),
            "{:?}",
            found.notes
        );

        let recent = watch_store::Run {
            posts_skipped: Some("not_verified".into()),
            ..ran(ExitCode::Ok)
        };
        let found = health(
            Some(&configured),
            &[of(&recent)],
            &[],
            Default::default(),
            NOW,
            None,
        );
        assert_eq!(found.verdict, Verdict::Ok, "{:?}", found.notes);
    }
}
