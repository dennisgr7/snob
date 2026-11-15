//! What a configured monitor would find if it ran now.
//!
//! **This writes nothing and can be repeated.**

use snob_core::{Epoch, Pk};
use snob_store::secrets::SecretStore;
use snob_store::store::snapshots;
use snob_store::store::watch as watch_store;

use crate::exit::ExitCode;
use snob_core::watch::schedule::{self, Schedule};
use snob_store::config::WatchConfig;

use crate::app::{App, Held};

/// How a single check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verdict {
    /// It would work.
    Ok,
    /// It would work, but something about it will surprise somebody.
    Warned,
    /// A scheduled run would not do what the configuration says.
    Failed,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warned => "warning",
            Self::Failed => "failed",
        }
    }

    /// What a command exits with after reaching this verdict.
    pub fn exit_code(self) -> ExitCode {
        match self {
            Self::Failed => ExitCode::Error,
            Self::Ok | Self::Warned => ExitCode::Ok,
        }
    }
}

/// One thing that was checked.
#[derive(Debug, Clone)]
pub struct Checked {
    pub what: What,
    pub verdict: Verdict,
    pub problem: Option<Problem>,
}

/// What is wrong with something that was checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// There is no `watch.toml`, so a bare `snob watch` has nothing to run on.
    NothingConfigured,
    /// The schedule is well formed and names no moment that exists.
    NeverFires,
    /// The scheduler refused the file, and said why.
    Unbuildable(String),
    /// The file names no schedule: a run takes one from the command line, or
    /// does not start.
    NoSchedule,
    /// Nothing was asked about this, because the account is in cooldown, its
    /// own or the brake on every account.
    InCooldown { held: Held },
    /// An unattended run may not read this account, which is a fact about the
    /// file and not about the network.
    NoRecordedConsent { in_cooldown: bool },
    /// The session did not answer, so nothing could be asked about anybody.
    SessionSilent,
    /// There is no session at all.
    NoSession,
    /// The session stored was signed out on this computer since
    /// (`owner::signed_out`), so nothing is sent as it.
    SignedOutHere,
    /// The session carries no username and Instagram did not name the account
    /// either, so there is nothing to ask the profile endpoint with.
    NoUsername,
    /// The id came from search, which carries no counters — so this run cannot
    /// tell a truncated list from a complete one.
    CountersUnknowable,
    /// Nothing has been reported on yet, so the first scheduled run has nothing
    /// to compare against and will say nothing.
    FirstRunLaysTheBaseline,
    /// `--no-webhook`, so the address was checked and not used.
    NotPosted,
    /// The account's posts are watched, and a grid is read from the browser
    /// only: `SNOB_NO_BROWSER` is set.
    PostsNeedABrowser,
    /// The account is private and the viewer does not follow it, so its
    /// posts are not shown.
    PostsHidden,
    /// `quality = "best"` and no ffmpeg on this machine: every run refuses.
    NoFfmpeg,
    /// The directory the posts are saved in cannot be made or written.
    CannotSaveTo(String),
    /// Nothing of the account's posts has been seen yet, so the first run
    /// lays their baseline and reports none.
    FirstRunLaysThePostsBaseline,
    /// Whatever the schedule parser, the store, Instagram or the user's own
    /// server said, carried through unchanged.
    Foreign(String),
}

/// What was checked, and what was learned about it.
#[derive(Debug, Clone)]
pub enum What {
    /// No `watch.toml` at all, so there is nothing else to check.
    NotConfigured,
    /// The schedule, and the next few moments it fires at.
    Schedule { next: Vec<Epoch> },
    /// The session, and which backend the secret store landed on.
    Session {
        viewer: Option<String>,
        backend: &'static str,
    },
    /// One configured account.
    Account {
        /// As the file spells it. `None` is the session's own account.
        target: Option<String>,
        pk: Option<Pk>,
        followers: Option<u64>,
        following: Option<u64>,
        /// Whether an unattended run may read it, which for somebody else's
        /// account means a recorded answer.
        may_run_unattended: bool,
        /// Whether its followers and following are watched, or only its
        /// posts.
        lists: bool,
    },
    /// The address a report would be posted to.
    Webhook {
        destination: String,
        status: Option<u16>,
        signed: bool,
    },
    /// Whether there is anything to compare the first scheduled run against.
    Baseline {
        /// When the newest capture of each list was taken. Empty means none.
        taken_at: Vec<(snob_core::model::ListKind, Epoch)>,
    },
    /// The posts of one configured account, when they are watched.
    Posts {
        /// As the file spells it. `None` is the session's own account.
        target: Option<String>,
        /// Where they are saved, as the file says.
        save_to: Option<String>,
        /// When their baseline was laid, if it was.
        baseline_at: Option<Epoch>,
    },
}

/// Everything that was checked, in the order it was checked.
#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    pub checked: Vec<Checked>,
}

impl CheckReport {
    /// The worst verdict in it, which is what the exit code is made of.
    pub fn verdict(&self) -> Verdict {
        self.checked
            .iter()
            .map(|c| c.verdict)
            .max()
            .unwrap_or(Verdict::Ok)
    }

    /// Whether anything in here says the first scheduled run would have nothing
    /// to compare against.
    pub fn wants_a_baseline(&self) -> bool {
        self.checked.iter().any(|c| wants_a_baseline(&c.what))
    }

    fn push(&mut self, what: What, verdict: Verdict, problem: Option<Problem>) {
        self.checked.push(Checked {
            what,
            verdict,
            problem,
        });
    }
}

/// How many upcoming moments to work out, so somebody can recognize their own
/// schedule in them. Three is enough to tell "every Monday" from "every day".
const MOMENTS_SHOWN: usize = 3;

/// Checks the schedule alone, which needs no session and no network.
pub fn schedule_of(schedule: &Schedule, now: Epoch) -> Checked {
    let mut next = Vec::new();
    let mut at = now;
    for _ in 0..MOMENTS_SHOWN {
        match schedule::next_after(schedule, Some(at), at, &chrono::Local) {
            Some(moment) if moment > at => {
                next.push(moment);
                at = moment;
            }
            _ => break,
        }
    }

    let verdict = if next.is_empty() {
        Verdict::Failed
    } else {
        Verdict::Ok
    };
    let problem = next.is_empty().then_some(Problem::NeverFires);

    Checked {
        what: What::Schedule { next },
        verdict,
        problem,
    }
}

/// Everything that can be checked without a session.
pub fn without_a_session(
    configured: Option<&WatchConfig>,
    schedule: Option<&Result<Schedule, String>>,
    now: Epoch,
) -> CheckReport {
    let mut report = CheckReport::default();

    if configured.is_none() {
        report.push(
            What::NotConfigured,
            Verdict::Warned,
            Some(Problem::NothingConfigured),
        );
    }

    match schedule {
        Some(Ok(schedule)) => report.checked.push(schedule_of(schedule, now)),
        Some(Err(why)) => report.push(
            What::Schedule { next: Vec::new() },
            Verdict::Failed,
            Some(Problem::Unbuildable(why.clone())),
        ),
        None => {}
    }

    report
}

/// The session, the accounts and the baselines.
pub async fn with_a_session(
    app: &App,
    secrets: &SecretStore,
    watched: &[super::watch::Watched],
    report: &mut CheckReport,
) {
    // **Nothing is spent during a cooldown.**
    match app.held() {
        Ok(Some(held)) => {
            report.checked.push(waiting_out(
                What::Session {
                    viewer: app.viewer().username.clone(),
                    backend: secrets.backend().as_str(),
                },
                held.clone(),
            ));
            for account in watched {
                report.checked.push(not_asked_about(account, held.clone()));
            }
            return;
        }
        Ok(None) => {}
        // The budget itself is unreadable.
        Err(e) => {
            report.checked.push(Checked {
                what: What::Session {
                    viewer: app.viewer().username.clone(),
                    backend: secrets.backend().as_str(),
                },
                verdict: Verdict::Failed,
                problem: Some(Problem::Foreign(e.to_string())),
            });
            return;
        }
    }

    let session = match app.client().validate().await {
        Ok(()) => Checked {
            what: What::Session {
                viewer: app.viewer().username.clone(),
                backend: secrets.backend().as_str(),
            },
            verdict: Verdict::Ok,
            problem: None,
        },
        Err(e) => Checked {
            what: What::Session {
                viewer: app.viewer().username.clone(),
                backend: secrets.backend().as_str(),
            },
            verdict: Verdict::Failed,
            problem: Some(Problem::Foreign(crate::sentences::what_instagram_said(&e))),
        },
    };
    let session_works = session.verdict == Verdict::Ok;
    report.checked.push(session);

    for (asked, account) in watched.iter().enumerate() {
        // Asked again before every account, because one of them can earn a
        // cooldown while this loop is running.
        if asked > 0
            && let Ok(Some(held)) = app.held()
        {
            for remaining in &watched[asked..] {
                report
                    .checked
                    .push(not_asked_about(remaining, held.clone()));
            }
            return;
        }

        // The baseline is asked about with the id this check just resolved, so
        // it is the same account the scheduled run would compare.
        let (checked, pk, shown) = account_of(app, account, session_works).await;
        report.checked.push(checked);
        if let Some(pk) = pk
            && account.lists()
        {
            report.checked.push(baseline_of(app, pk));
        }
        if account.posts().is_some() {
            report.checked.push(posts_of(app, account, pk, shown));
        }
    }
}

/// What a run would make of `watched`'s posts, asked of what is known here and
/// of whether the profile just read shows them (`shown`); no request.
///
/// **The directory is not made**: `check` writes nothing.
pub fn posts_of(
    app: &App,
    watched: &super::watch::Watched,
    pk: Option<Pk>,
    shown: Option<bool>,
) -> Checked {
    let entry = watched.posts().cloned().unwrap_or_default();
    let baseline_at = pk.and_then(|pk| {
        snob_store::store::watch_posts::mark(app.db().conn(), pk)
            .ok()
            .flatten()
            .map(|mark| mark.baseline_at)
    });
    let what = What::Posts {
        target: watched.name().map(str::to_string),
        save_to: entry
            .save_to
            .as_ref()
            .map(|dir| dir.to_string_lossy().into_owned()),
        baseline_at,
    };
    let (verdict, problem) = if !app.client().has_page() {
        (Verdict::Warned, Some(Problem::PostsNeedABrowser))
    } else if shown == Some(false) {
        (Verdict::Warned, Some(Problem::PostsHidden))
    } else if entry.save_to.is_some()
        && entry.quality == Some(snob_store::config::VideoQuality::Best)
        && crate::ffmpeg::Ffmpeg::find(None).is_none()
    {
        (Verdict::Failed, Some(Problem::NoFfmpeg))
    } else if let Some(dir) = entry.save_to.as_deref()
        && !could_be_made(dir)
    {
        (
            Verdict::Warned,
            Some(Problem::CannotSaveTo(dir.to_string_lossy().into_owned())),
        )
    } else if baseline_at.is_none() {
        (Verdict::Warned, Some(Problem::FirstRunLaysThePostsBaseline))
    } else {
        (Verdict::Ok, None)
    };
    Checked {
        what,
        verdict,
        problem,
    }
}

/// Whether `dir` is a directory that can be written, or could be made under
/// the nearest one that is, without making anything.
fn could_be_made(dir: &std::path::Path) -> bool {
    let mut at = Some(dir);
    while let Some(here) = at {
        if here.exists() {
            return here.is_dir()
                && std::fs::metadata(here).is_ok_and(|meta| !meta.permissions().readonly());
        }
        at = here.parent();
    }
    false
}

/// The line something gets when a cooldown means it was not asked about.
fn waiting_out(what: What, held: Held) -> Checked {
    Checked {
        what,
        verdict: Verdict::Warned,
        problem: Some(Problem::InCooldown { held }),
    }
}

/// The same, for an account — except for the one thing a cooldown has nothing
/// to do with.
///
/// It is decided before any request, no cooldown affects it, and
/// `commands::watch` refuses to **start** without it.
fn not_asked_about(account: &super::watch::Watched, held: Held) -> Checked {
    let may_run_unattended = account.may_run_unattended();
    let what = What::Account {
        target: account.name().map(str::to_string),
        pk: None,
        followers: None,
        following: None,
        may_run_unattended,
        lists: account.lists(),
    };

    if may_run_unattended {
        return waiting_out(what, held);
    }
    Checked {
        what,
        verdict: Verdict::Failed,
        problem: Some(Problem::NoRecordedConsent { in_cooldown: true }),
    }
}

/// One configured account: does it resolve, may an unattended run read it, and
/// what do its counters say.
async fn account_of(
    app: &App,
    watched: &super::watch::Watched,
    ask: bool,
) -> (Checked, Option<Pk>, Option<bool>) {
    let target = watched.name().map(str::to_string);
    let may_run_unattended = watched.may_run_unattended();

    let mut what = What::Account {
        target: target.clone(),
        pk: None,
        followers: None,
        following: None,
        may_run_unattended,
        lists: watched.lists(),
    };

    // A session that does not work cannot answer about anybody, and asking
    // would spend a request to learn what the line above already said.
    if !ask {
        return (
            Checked {
                what,
                verdict: Verdict::Warned,
                problem: Some(Problem::SessionSilent),
            },
            None,
            None,
        );
    }

    let name = match &target {
        Some(name) => name.clone(),
        None => match &app.viewer().username {
            Some(name) => name.clone(),
            // The session carries an id and no name, so the account has to be resolved
            // before anything can be asked about it.
            None => match app.client().resolve_username(app.viewer().pk).await {
                Ok(Some(name)) => name,
                // Instagram answered and carried no username.
                Ok(None) => {
                    return (
                        Checked {
                            what,
                            verdict: Verdict::Warned,
                            problem: Some(Problem::NoUsername),
                        },
                        None,
                        None,
                    );
                }
                Err(e) => {
                    return (
                        Checked {
                            what,
                            verdict: Verdict::Failed,
                            problem: Some(Problem::Foreign(crate::sentences::what_instagram_said(
                                &e,
                            ))),
                        },
                        None,
                        None,
                    );
                }
            },
        },
    };

    // The own account's pk is the session's, and a third party's is known
    // once a list walked here named it; a check writes nothing, so without
    // one every poll pays the lookup again.
    let known = match &target {
        None => Some(app.viewer().pk),
        Some(_) => crate::engine::target::known_pk(app, &name).ok().flatten(),
    };
    // The client's own read: a check writes nothing, the cache included.
    match app.client().profile_named(&name, known).await {
        Ok(profile) => {
            let shown = profile.id == app.viewer().pk
                || profile.is_private != Some(true)
                || profile.followed_by_viewer == Some(true);
            what = What::Account {
                target,
                pk: Some(profile.id),
                followers: profile.follower_count(),
                following: profile.following_count(),
                may_run_unattended,
                lists: watched.lists(),
            };
            // Counters that cannot be known are the preflight's own subject.
            let counters_unknown = !profile.counters_are_knowable();
            let verdict = if !may_run_unattended {
                Verdict::Failed
            } else if counters_unknown {
                Verdict::Warned
            } else {
                Verdict::Ok
            };
            let problem = if !may_run_unattended {
                Some(Problem::NoRecordedConsent { in_cooldown: false })
            } else if counters_unknown {
                Some(Problem::CountersUnknowable)
            } else {
                None
            };
            (
                Checked {
                    what,
                    verdict,
                    problem,
                },
                Some(profile.id),
                Some(shown),
            )
        }
        Err(e) => (
            Checked {
                what,
                verdict: Verdict::Failed,
                problem: Some(Problem::Foreign(crate::sentences::what_instagram_said(&e))),
            },
            None,
            None,
        ),
    }
}

/// What the preflight message calls itself, in the header and in the body.
pub const PREFLIGHT_EVENT: &str = "watch.preflight";

/// Posts one message to the address a report would go to.
///
/// `event` is `watch.preflight`, so a receiver can branch on it exactly as it
/// branches on the rest, and **nothing is queued**: this is not a report, so
/// there is nothing to retry and nothing to deduplicate.
pub async fn webhook_of(
    client: &crate::watch::webhook::WebhookClient,
    destination: String,
    signed: bool,
    run_id: &str,
    body: &str,
    content_type: &str,
) -> Checked {
    use crate::watch::webhook::Attempt;

    // Built inside each arm, because the status differs; `describe_check`
    // builds its "{destination} answered {code}" sentence out of both fields.
    match client
        .post(body, content_type, PREFLIGHT_EVENT, run_id, 1)
        .await
    {
        Attempt::Delivered { status } => Checked {
            what: What::Webhook {
                destination,
                status: Some(status),
                signed,
            },
            verdict: Verdict::Ok,
            problem: None,
        },
        // **The code the far end answered with is carried through**, so a probe
        // can tell "404, the workflow is not registered" from "the host does
        // not resolve".
        other => Checked {
            what: What::Webhook {
                destination,
                status: other.status(),
                signed,
            },
            verdict: Verdict::Failed,
            problem: Some(Problem::Foreign(other.error().to_string())),
        },
    }
}

/// The one test of "there is nothing to compare against yet".
fn wants_a_baseline(what: &What) -> bool {
    matches!(what, What::Baseline { taken_at } if taken_at.len() < 2)
        || matches!(
            what,
            What::Posts {
                baseline_at: None,
                ..
            }
        )
}

/// When the capture a run would compare against was taken, if there is one.
///
/// **Asked of `watch_marks`, not of the newest capture.** A run compares against
/// what was last *reported*, as AGENTS.md states.
fn reported_baseline(
    app: &App,
    pk: Pk,
    kind: snob_core::model::ListKind,
) -> Result<Option<Epoch>, snob_store::store::StoreError> {
    let Some(id) = watch_store::mark(app.db().conn(), pk, kind)?.and_then(|m| m.snapshot_id) else {
        return Ok(None);
    };
    Ok(snapshots::find_usable(app.db().conn(), id)?.map(|s| s.taken_at.unwrap_or_default()))
}

/// Whether there is anything for the first scheduled run to compare against.
pub fn baseline_of(app: &App, pk: Pk) -> Checked {
    let mut taken_at = Vec::new();
    for kind in [
        snob_core::model::ListKind::Followers,
        snob_core::model::ListKind::Following,
    ] {
        match reported_baseline(app, pk, kind) {
            Ok(Some(at)) => taken_at.push((kind, at)),
            Ok(None) => {}
            Err(e) => {
                return Checked {
                    what: What::Baseline { taken_at },
                    verdict: Verdict::Warned,
                    problem: Some(Problem::Foreign(e.to_string())),
                };
            }
        }
    }

    let what = What::Baseline { taken_at };
    let wants = wants_a_baseline(&what);
    Checked {
        what,
        verdict: if wants { Verdict::Warned } else { Verdict::Ok },
        problem: wants.then_some(Problem::FirstRunLaysTheBaseline),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A schedule the scheduler refuses is reported, not omitted.
    ///
    /// Without a schedule line `verdict()`, `max().unwrap_or(Ok)`, would exit
    /// 0 about a file that kills `snob watch` at `schedule_from` on every
    /// invocation. The `NotConfigured` warning does not cover it, because a
    /// file exists.
    #[test]
    fn a_schedule_the_scheduler_refuses_is_reported_rather_than_omitted() {
        let configured = snob_store::config::parse(
            "schema = 1\nevery = \"5m\"\n",
            std::path::Path::new("watch.toml"),
        )
        .expect("config::parse reads TOML and a schema number, not what the values mean");
        let refused: Result<Schedule, String> = Err("5m is too often".to_string());

        let report =
            without_a_session(Some(&configured), Some(&refused), Epoch::new(1_700_000_000));

        assert_eq!(report.verdict(), Verdict::Failed);
        assert_eq!(report.checked.len(), 1, "{:?}", report.checked);
        assert!(
            matches!(
                &report.checked[0].problem,
                Some(Problem::Unbuildable(why)) if why.contains("5m is too often")
            ),
            "the line has to carry what the scheduler said: {:?}",
            report.checked
        );
    }

    /// One verdict, one code, whichever command asked and in whichever format.
    #[test]
    fn a_verdict_decides_one_exit_code() {
        assert_eq!(Verdict::Failed.exit_code(), ExitCode::Error);

        // A monitor with no baseline yet will work; it just has nothing to say
        // on its first run, and one sitting out a cooldown is working too.
        assert_eq!(Verdict::Warned.exit_code(), ExitCode::Ok);
        assert_eq!(Verdict::Ok.exit_code(), ExitCode::Ok);
    }

    /// The moments a schedule names, worked out through the evaluator that
    /// actually decides them.
    ///
    /// Not recomputed here in any other way, and that is the point: a preflight
    /// with arithmetic of its own would be checking a schedule nobody runs.
    #[test]
    fn a_schedule_names_the_moments_it_will_fire_at() {
        let schedule = Schedule::every(Duration::from_secs(6 * 3_600)).unwrap();
        let checked = schedule_of(&schedule, Epoch::new(1_700_000_000));

        assert_eq!(checked.verdict, Verdict::Ok);
        let What::Schedule { next } = checked.what else {
            panic!("a schedule check is about a schedule");
        };
        assert_eq!(next.len(), MOMENTS_SHOWN);
        assert!(
            next.windows(2).all(|pair| pair[1] > pair[0]),
            "they have to be in the future and in order: {next:?}"
        );
    }

    /// A calendar that never fires is the one answer worth a red line before
    /// anything is scheduled at all. `0 0 31 2 *` is the standing example:
    /// there is no thirty-first of February.
    #[test]
    fn a_schedule_that_never_fires_is_a_failure_rather_than_a_wait() {
        let schedule = Schedule::cron("0 0 31 2 *").unwrap();
        let checked = schedule_of(&schedule, Epoch::new(1_700_000_000));

        assert_eq!(checked.verdict, Verdict::Failed);
        assert!(checked.problem.is_some());
    }

    /// A machine with no `watch.toml` is not broken, but a bare `snob watch`
    /// there has no schedule to run on, and saying so is the whole job.
    #[test]
    fn nothing_configured_is_reported_rather_than_passed_over() {
        let report = without_a_session(None, None, Epoch::new(1_700_000_000));

        assert_eq!(report.verdict(), Verdict::Warned);
        assert!(matches!(
            report.checked.first().map(|c| &c.what),
            Some(What::NotConfigured)
        ));
    }

    /// The exit code is made of the worst line, so one failure among healthy
    /// ones is still a failure.
    #[test]
    fn the_verdict_is_the_worst_of_them() {
        let mut report = CheckReport::default();
        assert_eq!(
            report.verdict(),
            Verdict::Ok,
            "nothing checked is not a fail"
        );

        report.push(What::NotConfigured, Verdict::Ok, None);
        report.push(What::NotConfigured, Verdict::Failed, None);
        report.push(What::NotConfigured, Verdict::Warned, None);
        assert_eq!(report.verdict(), Verdict::Failed);
    }
}
