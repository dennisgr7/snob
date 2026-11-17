//! `snob status`: where the account stands, read from what is stored here.

use anyhow::Result;
use serde_json::{Value, json};
use snob_core::model::{ListKind, printable};
use snob_core::{Epoch, EpochMs};
use snob_engine::exit::ExitCode;
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::registry::Registry;
use snob_store::secrets::SecretStore;
use snob_store::store::answers::{self, AnswersDay};
use snob_store::store::rate_budget::{self, BucketState, BudgetState};
use snob_store::store::shared::Shared;
use snob_store::store::{Store, accounts, mcp_acts, snapshots, users, watch as watch_store};

use crate::cli::{StatusArgs, StatusSections};
use crate::report;

/// Reports on `account`, the account resolved for this run, and with `--json`
/// and no section named on the updater as well (`update::report`, read from its
/// file), `sandboxed` saying whether this run is a sandbox's.
pub fn run(
    args: StatusArgs,
    secrets: &SecretStore,
    paths: &AppPaths,
    account: Option<AccountPaths>,
    sandboxed: bool,
) -> Result<ExitCode> {
    let viewer = snob_engine::sentences::acting_as().map(|viewer| viewer.json());
    // Where the session was found, not where this run would put a new one:
    // on a machine with no keyring, `login` fell back to the file, and every
    // later run reads it from there.
    let located = match &account {
        Some(account) => secrets.session_of(account).load_located()?,
        None => None,
    };
    let (Some((session, storage)), Some(account)) = (located, account) else {
        return no_session(
            &args,
            viewer,
            "no session is stored on this computer",
            paths,
            sandboxed,
        );
    };
    // A session this computer no longer sends as is no session to report on
    // either, and is said as what it is: every command refuses it at its
    // start (`App::open`), with the same code.
    if snob_engine::browser::owner::signed_out(paths, &session) {
        return no_session(
            &args,
            viewer,
            snob_engine::sentences::SIGNED_OUT_HERE,
            paths,
            sandboxed,
        );
    }

    let now = snob_core::clock::now_ms();
    // An account that has never sent anything has no database, and its
    // budget is at rest: read over an empty one rather than making its file.
    let store = match Store::read_existing(&account)? {
        Some(store) => store,
        None => Store::in_memory()?,
    };
    let shared = Shared::read_existing(paths).unwrap_or_else(|e| {
        tracing::debug!(error = %e, "shared.db could not be read; no brake is reported");
        None
    });
    let budget = rate_budget::state(store.conn(), shared.as_ref(), now)?;
    let held = budget.held_until(now);
    let braked = braked_names(&budget, held, paths);

    let pk = account.pk();
    let sections = args.sections();
    let mut lists = Vec::new();
    if sections.lists {
        let counters = accounts::find(store.conn(), pk)?;
        for kind in [ListKind::Followers, ListKind::Following] {
            lists.push(Stored {
                kind,
                latest: snapshots::latest_complete(store.conn(), pk, kind)?,
                resumable: snapshots::is_resumable(store.conn(), pk, kind)?,
                counter: counters.as_ref().and_then(|c| c.counter(kind)),
                polled_at: counters.as_ref().and_then(|c| c.polled_at),
            });
        }
    }
    let mut runs = Vec::new();
    if sections.watch {
        for run in watch_store::last_runs(store.conn())? {
            let name = users::name(store.conn(), run.account_pk)?;
            runs.push((run, name));
        }
    }

    // How long Instagram's answers took, and the floor the pace would put
    // under the next step from the latest of them (`Pacer::slowed`).
    let answered = if sections.answers {
        let rule = snob_ig::pace::Pace::default().slow;
        let recent = answers::recent_waits(store.conn(), rule.since(now), rule.samples)?;
        Answered {
            day: answers::last_day(store.conn(), now)?,
            slowing: slowing(rule.floor(&recent)),
        }
    } else {
        Answered::default()
    };

    // What agents were told yes to, through `snob mcp`: the day the cap on
    // their writes counts.
    let acts = if sections.agents {
        mcp_acts::last_day(store.conn(), now)?
    } else {
        Vec::new()
    };
    // The walks agents left running, read from their files and their locks,
    // which asking does not write (`snob_engine::jobs::look`).
    let jobs: Vec<snob_engine::jobs::Seen> = if sections.jobs {
        snob_engine::jobs::all(&account, now.get())?
            .into_iter()
            .filter(|seen| !seen.status.finished())
            .collect()
    } else {
        Vec::new()
    };

    let found = Found {
        sections,
        session: &session,
        storage: storage.as_str(),
        budget: &budget,
        answered: &answered,
        held,
        braked: &braked,
        lists: &lists,
        runs: &runs,
        acts: &acts,
        jobs: &jobs,
        now,
    };
    if args.output.json {
        let mut out = found.json();
        out["viewer"] = viewer.unwrap_or(Value::Null);
        if !args.names_a_section() {
            out["update"] = snob_engine::update::report(paths, sandboxed);
        }
        crate::ui::say!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        crate::ui::say!("{}", found.text());
    }

    // The account's state, whatever sections were printed: a script asks
    // "may I send" with any of them.
    Ok(if held.is_some() {
        ExitCode::RateLimited
    } else {
        ExitCode::Ok
    })
}

/// What is said with no session to report on, `why` being the reason: the
/// sentence on standard error with its advice, and with `--json` every
/// section asked for there and null, as `whoami` gives every field with no
/// session, beside the error.
fn no_session(
    args: &StatusArgs,
    viewer: Option<Value>,
    why: &str,
    paths: &AppPaths,
    sandboxed: bool,
) -> Result<ExitCode> {
    eprintln!("{}. Run \"snob login\".", report::opening(why));
    if args.output.json {
        let mut out = serde_json::Map::new();
        for key in args.sections().names() {
            out.insert(key.into(), Value::Null);
        }
        out.insert(
            "error".into(),
            json!({
                "code": ExitCode::NoSession.as_str(),
                "message": why,
            }),
        );
        out.insert("viewer".into(), viewer.unwrap_or(Value::Null));
        if !args.names_a_section() {
            out.insert(
                "update".into(),
                snob_engine::update::report(paths, sandboxed),
            );
        }
        crate::ui::say!("{}", serde_json::to_string_pretty(&Value::Object(out))?);
    }
    Ok(ExitCode::NoSession)
}

/// The accounts whose push-backs braked every account, named from the
/// registry, when the brake is what holds this one.
fn braked_names(budget: &BudgetState, held: Option<EpochMs>, paths: &AppPaths) -> Vec<String> {
    let Some(brake) = &budget.brake else {
        return Vec::new();
    };
    if held != Some(brake.until) {
        return Vec::new();
    }
    let registry = Registry::load(paths).unwrap_or_default();
    brake
        .accounts
        .iter()
        .map(|&pk| snob_engine::app::label(pk, registry.get(pk).map(|r| r.username.as_str())))
        .collect()
}

/// A line of the text form: the label padded to a column, then the value.
fn field(label: &str, value: &str) -> String {
    format!("{label:<10} {value}")
}

/// A line inside a section, indented under its heading.
fn row(label: &str, value: &str) -> String {
    format!("  {label:<14} {value}")
}

/// The answers of the day, and how much longer than its shortest the next
/// step would be for them.
#[derive(Default)]
struct Answered {
    day: AnswersDay,
    slowing: Option<std::time::Duration>,
}

/// The floor slow answers put under a step, when it is longer than the
/// shortest step the pace draws: only then does it lengthen any.
fn slowing(floor: Option<std::time::Duration>) -> Option<std::time::Duration> {
    let shortest = std::time::Duration::from_millis(snob_ig::pace::STEP_MS.0);
    floor.filter(|floor| *floor > shortest)
}

/// A length of time as a line of `status` says it: milliseconds under a
/// second, tenths of a second above.
fn took(wait: std::time::Duration) -> String {
    let ms = wait.as_millis();
    if ms < 1_000 {
        format!("{ms} ms")
    } else {
        format!("{:.1} s", wait.as_secs_f64())
    }
}

/// One of the account's own lists, as stored.
struct Stored {
    kind: ListKind,
    latest: Option<snapshots::Snapshot>,
    resumable: bool,
    counter: Option<u64>,
    polled_at: Option<Epoch>,
}

/// Everything read, for the two ways of saying it.
struct Found<'a> {
    sections: StatusSections,
    session: &'a snob_core::session::Session,
    storage: &'a str,
    budget: &'a BudgetState,
    answered: &'a Answered,
    held: Option<EpochMs>,
    braked: &'a [String],
    lists: &'a [Stored],
    runs: &'a [(watch_store::Run, Option<String>)],
    acts: &'a [mcp_acts::Act],
    jobs: &'a [snob_engine::jobs::Seen],
    now: EpochMs,
}

impl Found<'_> {
    /// Every value a stable token or a number; moments in seconds, as `whoami`
    /// gives them.
    fn json(&self) -> Value {
        let mut out = serde_json::Map::new();
        if self.sections.session {
            out.insert(
                "session".into(),
                json!({
                    "pk": self.session.ds_user_id,
                    "username": self.session.username,
                    "origin": self.session.origin.as_str(),
                    "storage": self.storage,
                    "created_at": self.session.created_at,
                    "validated_at": self.session.validated_at,
                }),
            );
        }
        if self.sections.budget {
            let b = self.budget;
            let bucket = |s: &BucketState| json!({ "left": s.left, "most": s.most, "free_at": s.free_at.to_epoch_not_before() });
            out.insert(
                "budget".into(),
                json!({
                    "requests_now": b.requests_now(),
                    "next_request_at": b.next_request_at().to_epoch_not_before(),
                    "pace": bucket(&b.pace),
                    "day": bucket(&b.daily),
                    "writes": bucket(&b.writes),
                    "next_write_at": b.next_write_at.to_epoch_not_before(),
                    "accounts_read": b.accounts_read,
                    "accounts_ceiling": b.accounts_ceiling,
                    "accounts_left": b.accounts_left(),
                }),
            );
        }
        if self.sections.answers {
            let day = &self.answered.day;
            let ms = |wait: Option<std::time::Duration>| wait.map(|w| w.as_millis() as u64);
            out.insert(
                "answers".into(),
                json!({
                    "count": day.answers,
                    "refused": day.refused,
                    "median_ms": ms(day.median),
                    "p95_ms": ms(day.p95),
                    "slowest_ms": ms(day.slowest),
                    "via": day.via.map(snob_core::budget::Via::as_str),
                    "load": day.load.as_ref().map(|(value, at)| json!({
                        "value": printable(value),
                        "at": at.to_epoch(),
                    })),
                    "slowing_ms": ms(self.answered.slowing),
                }),
            );
        }
        if self.sections.cooldown {
            let last = self.budget.last_cooldown.as_ref().map(|c| {
                json!({
                    "until": c.until.to_epoch_not_before(),
                    "set_at": c.set_at.to_epoch(),
                    "reason": c.reason,
                    "strikes": c.strikes,
                    "escalates_until": c.escalates_until().to_epoch_not_before(),
                })
            });
            let brake =
                self.budget.brake.as_ref().map(
                    |b| json!({ "until": b.until.to_epoch_not_before(), "accounts": b.accounts }),
                );
            out.insert(
                "cooldown".into(),
                json!({
                    "active": self.held.is_some(),
                    "until": self.held.map(EpochMs::to_epoch_not_before),
                    "last": last,
                    "brake": brake,
                }),
            );
        }
        if self.sections.lists {
            let mut lists = serde_json::Map::new();
            for list in self.lists {
                lists.insert(
                    list.kind.as_str().into(),
                    json!({
                        "taken_at": list.latest.as_ref().and_then(|s| s.taken_at),
                        "members": list.latest.as_ref().map(|s| s.member_count),
                        "declared": list.latest.as_ref().and_then(|s| s.declared_count),
                        "resumable": list.resumable,
                        "counter": list.counter,
                        "polled_at": list.polled_at,
                    }),
                );
            }
            out.insert("lists".into(), Value::Object(lists));
        }
        if self.sections.watch {
            let runs: Vec<Value> = self
                .runs
                .iter()
                .map(|(run, name)| {
                    json!({
                        "pk": run.account_pk,
                        // Through `printable`, as every name somebody else
                        // chose is before it reaches a JSON reader.
                        "username": name.as_deref().map(printable),
                        "at": run.started_at,
                        "outcome": run.outcome.as_ref().map(|o| o.as_str()),
                        "requests": run.requests,
                        "changes": run.changes,
                    })
                })
                .collect();
            out.insert("watch".into(), json!({ "last_runs": runs }));
        }
        if self.sections.agents {
            // The name is the one the agent's call gave, as the server
            // checked it, and the client is the server's reduction of what
            // the client said; both through `printable` all the same.
            let acts: Vec<Value> = self
                .acts
                .iter()
                .map(|act| {
                    json!({
                        "at": act.at.to_epoch(),
                        "action": act.action,
                        "write": act.is_write(),
                        "target": {
                            "pk": act.target_pk,
                            "username": act.target_name.as_deref().map(printable),
                        },
                        "via": act.via,
                        "client": act.client.as_deref().map(printable),
                    })
                })
                .collect();
            out.insert(
                "agents".into(),
                json!({ "writes": self.agent_writes(), "acts": acts }),
            );
        }
        if self.sections.jobs {
            // What `snob jobs --json` says of each, narrowed to what a status
            // is asked for: what, since when, how far, and for whom.
            let running: Vec<Value> = self
                .jobs
                .iter()
                .map(|seen| {
                    let record = snob_engine::jobs::show::record(
                        seen,
                        self.now.get(),
                        crate::jobs::argv::walk_of,
                    );
                    let mut kept = serde_json::Map::new();
                    for key in [
                        "job_id",
                        "status",
                        "command",
                        "account",
                        "created_at",
                        "started_by",
                        "progress",
                        "eta_seconds",
                    ] {
                        kept.insert(key.into(), record[key].clone());
                    }
                    Value::Object(kept)
                })
                .collect();
            out.insert("jobs".into(), json!({ "running": running }));
        }
        Value::Object(out)
    }

    fn text(&self) -> String {
        let mut blocks: Vec<Vec<String>> = Vec::new();
        if self.sections.session {
            let s = self.session;
            let who = match &s.username {
                Some(name) => format!("@{} ({})", printable(name), s.ds_user_id),
                None => s.ds_user_id.to_string(),
            };
            let checked = match s.validated_at {
                Some(at) => format!("last checked {}", snob_engine::sentences::stored_on(at)),
                None => "never checked".to_string(),
            };
            blocks.push(vec![
                field("Account", &who),
                field(
                    "Session",
                    &format!(
                        "{}, kept in {}; {checked}",
                        match s.origin {
                            snob_core::session::SessionOrigin::Browser => {
                                "signed in through the browser"
                            }
                            snob_core::session::SessionOrigin::Paste => "pasted by hand",
                        },
                        match self.storage {
                            "keyring" => "the system keyring",
                            _ => "a file",
                        }
                    ),
                ),
            ]);
        }
        if self.sections.budget {
            let b = self.budget;
            blocks.push(vec![
                "Budget".to_string(),
                row(
                    "Requests",
                    &format!(
                        "{} now without a wait{}",
                        b.requests_now(),
                        if b.requests_now() == 0 {
                            format!(", the next {}", self.when(b.next_request_at()))
                        } else {
                            String::new()
                        }
                    ),
                ),
                row(
                    "In a row",
                    &format!(
                        "{} of {}{}",
                        b.pace.left,
                        b.pace.most,
                        self.free_again(&b.pace)
                    ),
                ),
                row(
                    "Today",
                    &format!(
                        "{} of {}{}",
                        b.daily.left,
                        b.daily.most,
                        self.free_again(&b.daily)
                    ),
                ),
                row(
                    "Writes",
                    &format!(
                        "{} of {} in a row{}",
                        b.writes.left,
                        b.writes.most,
                        // Said only when the next one waits: on the write
                        // bucket, or on the reads a write pays as well.
                        if b.next_write_at > self.now {
                            format!("; the next {}", self.when(b.next_write_at))
                        } else {
                            String::new()
                        }
                    ),
                ),
                row(
                    "Accounts read",
                    &format!(
                        "{} of {} in the last 24 hours",
                        b.accounts_read, b.accounts_ceiling
                    ),
                ),
            ]);
        }
        if self.sections.answers {
            blocks.push(self.answers_text());
        }
        if self.sections.cooldown {
            let mut lines = vec!["Cooldown".to_string()];
            match self.held {
                Some(until) => {
                    let said = snob_engine::sentences::held_until(&snob_engine::app::Held {
                        until_ms: until,
                        braked: self.braked.to_vec(),
                    });
                    lines.push(format!("  {}", report::opening(&said)));
                }
                None => lines.push("  None; requests may go out".to_string()),
            }
            if let Some(last) = &self.budget.last_cooldown {
                lines.push(row(
                    "The last one",
                    &format!(
                        "{} on {}, strike {}",
                        printable(&last.reason),
                        snob_engine::sentences::stored_on(last.set_at.to_epoch()),
                        last.strikes
                    ),
                ));
                if self.now < last.escalates_until() {
                    lines.push(format!(
                        "  Another push-back before {} would be longer",
                        snob_engine::sentences::stored_on(last.escalates_until().to_epoch())
                    ));
                }
            }
            blocks.push(lines);
        }
        if self.sections.lists {
            let mut lines = vec!["Lists".to_string()];
            for list in self.lists {
                let name = match list.kind {
                    ListKind::Followers => "Followers",
                    ListKind::Following => "Following",
                };
                let stored = match &list.latest {
                    Some(s) => format!(
                        "{} stored {}",
                        s.member_count,
                        s.taken_at
                            .map_or("earlier".to_string(), snob_engine::sentences::stored_on)
                    ),
                    None => "none stored".to_string(),
                };
                let counter = match (list.counter, list.polled_at) {
                    (Some(n), Some(at)) => {
                        format!(
                            "; the counter said {n} {}",
                            snob_engine::sentences::stored_on(at)
                        )
                    }
                    _ => String::new(),
                };
                let resume = if list.resumable {
                    "; a walk is left to resume"
                } else {
                    ""
                };
                lines.push(row(name, &format!("{stored}{counter}{resume}")));
            }
            blocks.push(lines);
        }
        if self.sections.watch {
            let mut lines = vec!["Monitor".to_string()];
            if self.runs.is_empty() {
                lines.push("  It has not run from this account".to_string());
            }
            for (run, name) in self.runs {
                let outcome = run.outcome.as_ref().map_or("unfinished", |o| o.as_str());
                lines.push(format!(
                    "  {}  {outcome} {}, {} requests, {} changes",
                    snob_engine::app::label(run.account_pk, name.as_deref()),
                    snob_engine::sentences::stored_on(run.started_at),
                    run.requests,
                    run.changes
                ));
            }
            blocks.push(lines);
        }
        if self.sections.agents {
            let mut lines = vec!["Agents".to_string()];
            if self.acts.is_empty() {
                lines.push(
                    "  Told yes to nothing through snob mcp in the last 24 hours".to_string(),
                );
            } else {
                lines.push(row(
                    "Writes",
                    &format!("{} in the last 24 hours", self.agent_writes()),
                ));
            }
            for act in self.acts {
                lines.push(format!(
                    "  {}  {}",
                    snob_engine::sentences::stored_on(act.at.to_epoch()),
                    told_yes(act)
                ));
            }
            blocks.push(lines);
        }
        if self.sections.jobs {
            let mut lines = vec!["Jobs".to_string()];
            if self.jobs.is_empty() {
                lines.push("  No walk is running as a job".to_string());
            }
            for seen in self.jobs {
                lines.push(format!(
                    "  {}",
                    snob_engine::jobs::show::line(seen, self.now.get(), crate::jobs::argv::walk_of)
                ));
            }
            blocks.push(lines);
        }
        blocks
            .iter()
            .map(|block| block.join("\n"))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The answers of the day, as the text form says them.
    fn answers_text(&self) -> Vec<String> {
        let day = &self.answered.day;
        let mut lines = vec!["Answers".to_string()];
        if day.answers == 0 {
            lines.push("  None from Instagram in the last 24 hours".to_string());
            return lines;
        }
        let refused = match day.refused {
            0 => String::new(),
            n => format!(", {n} refused"),
        };
        lines.push(row(
            "Last 24 hours",
            &format!("{}{refused}", report::answers(day.answers)),
        ));
        if let (Some(median), Some(p95), Some(slowest)) = (day.median, day.p95, day.slowest) {
            lines.push(row(
                "Took",
                &format!(
                    "{} at the median, {} or less for 95 in 100, {} the slowest",
                    took(median),
                    took(p95),
                    took(slowest)
                ),
            ));
        }
        if let Some((load, at)) = &day.load {
            lines.push(row(
                "Load",
                &format!(
                    "{} {}",
                    printable(load),
                    snob_engine::sentences::stored_on(at.to_epoch())
                ),
            ));
        }
        lines.push(row(
            "Pace",
            &match self.answered.slowing {
                Some(floor) => format!(
                    "slowed: the latest answers were slow, so a step lasts at least {}",
                    took(floor)
                ),
                None => "as usual: the latest answers were not slow".to_string(),
            },
        ));
        lines
    }

    /// How many of the acts were follows or unfollows: what the cap counts.
    fn agent_writes(&self) -> usize {
        self.acts.iter().filter(|act| act.is_write()).count()
    }

    /// When a spent bucket lets the next one through, said after its count.
    fn free_again(&self, bucket: &BucketState) -> String {
        if bucket.left > 0 {
            String::new()
        } else {
            format!("; the next {}", self.when(bucket.free_at))
        }
    }

    /// "now", or the moment, rounded up to its second as the JSON's is.
    fn when(&self, at: EpochMs) -> String {
        if at <= self.now {
            "now".to_string()
        } else {
            snob_engine::sentences::stored_on(at.to_epoch_not_before())
        }
    }
}

/// One act an agent was told yes to, as a line: what, how the yes was
/// given, and through which client.
fn told_yes(act: &mcp_acts::Act) -> String {
    let who = act
        .target_name
        .as_deref()
        .map(|name| format!("@{}", printable(name)));
    let what = match (act.action.as_str(), &who) {
        (verb @ ("follow" | "unfollow"), Some(who)) => format!("{verb} {who}"),
        (list, Some(who)) => format!("read {who}'s {}", printable(list)),
        (list, None) => format!("read your {}", printable(list)),
    };
    use snob_store::store::mcp_acts::Via;
    let how = match Via::parse(&act.via) {
        Some(Via::Form | Via::Elicitation) => "yes given in a form".to_string(),
        Some(Via::Chat) => "yes given in the chat".to_string(),
        None => format!("yes given by {}", printable(&act.via)),
    };
    match &act.client {
        Some(client) => format!("{what}, {how}, through {}", printable(client)),
        None => format!("{what}, {how}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(flags: &[&str]) -> StatusArgs {
        use clap::Parser;
        let mut argv = vec!["snob", "status"];
        argv.extend_from_slice(flags);
        match crate::cli::Cli::parse_from(argv).command {
            crate::cli::Command::Status(args) => args,
            other => panic!("{other:?}"),
        }
    }

    /// No section named is every section; one named is that one alone.
    #[test]
    fn the_sections_are_all_of_them_or_the_ones_named() {
        let all = args(&[]).sections();
        assert!(all.session && all.budget && all.cooldown && all.lists && all.watch && all.agents);
        assert!(all.jobs && all.answers);
        let some = args(&["--budget", "--cooldown"]).sections();
        assert!(some.budget && some.cooldown);
        assert!(!some.session && !some.lists && !some.watch && !some.agents && !some.jobs);
        assert!(!some.answers);
        assert_eq!(args(&["--jobs"]).sections().names(), ["jobs"]);
        assert_eq!(args(&["--answers"]).sections().names(), ["answers"]);
    }

    /// The JSON carries only what was asked for.
    #[test]
    fn the_json_carries_the_sections_asked_for() {
        let session = snob_core::session::Session::from_sessionid(
            "42%3AAbCdEfGh%3A20",
            "Mozilla/5.0",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        let store = Store::in_memory().unwrap();
        let now = snob_core::clock::now_ms();
        let budget = rate_budget::state(store.conn(), None, now).unwrap();
        let found = Found {
            sections: args(&["--budget"]).sections(),
            session: &session,
            storage: "file",
            budget: &budget,
            answered: &Answered::default(),
            held: None,
            braked: &[],
            lists: &[],
            runs: &[],
            acts: &[],
            jobs: &[],
            now,
        };
        let out = found.json();
        let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["budget"]);
        assert_eq!(out["budget"]["day"]["left"], 2_001);
        assert_eq!(
            out["budget"]["requests_now"], 21,
            "the pace binds before the day does"
        );
        assert_eq!(out["budget"]["accounts_left"], 2_000);
        assert!(found.text().starts_with("Budget"));
    }

    #[test]
    fn what_an_agent_was_told_yes_to_is_said_in_a_line_and_in_the_json() {
        let now = snob_core::clock::now_ms();
        let act =
            |action: &str, target: Option<&str>, via: &str, client: Option<&str>| mcp_acts::Act {
                at: now,
                action: action.into(),
                target_pk: None,
                target_name: target.map(str::to_string),
                via: via.into(),
                client: client.map(str::to_string),
            };
        let acts = [
            act("unfollow", Some("someone"), "chat", Some("test 0")),
            act("followers", Some("other"), "form", None),
            act("fans", None, "elicitation", Some("x")),
        ];
        assert_eq!(
            told_yes(&acts[0]),
            "unfollow @someone, yes given in the chat, through test 0"
        );
        assert_eq!(
            told_yes(&acts[1]),
            "read @other's followers, yes given in a form"
        );
        assert_eq!(
            told_yes(&acts[2]),
            "read your fans, yes given in a form, through x"
        );

        let session = snob_core::session::Session::from_sessionid(
            "42%3AAbCdEfGh%3A20",
            "Mozilla/5.0",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        let store = Store::in_memory().unwrap();
        let budget = rate_budget::state(store.conn(), None, now).unwrap();
        let found = Found {
            sections: args(&["--agents"]).sections(),
            session: &session,
            storage: "file",
            budget: &budget,
            answered: &Answered::default(),
            held: None,
            braked: &[],
            lists: &[],
            runs: &[],
            acts: &acts,
            jobs: &[],
            now,
        };
        let out = found.json();
        assert_eq!(out["agents"]["writes"], 1);
        assert_eq!(out["agents"]["acts"][0]["action"], "unfollow");
        assert_eq!(out["agents"]["acts"][0]["write"], true);
        assert_eq!(out["agents"]["acts"][0]["target"]["username"], "someone");
        assert_eq!(out["agents"]["acts"][0]["via"], "chat");
        assert_eq!(out["agents"]["acts"][1]["client"], Value::Null);
        let text = found.text();
        assert!(text.starts_with("Agents\n"), "{text}");
        assert!(text.contains("1 in the last 24 hours"), "{text}");
    }

    /// The answers of the day, and the floor the latest of them put under
    /// a step, said in the JSON and in lines.
    #[test]
    fn the_answers_of_the_day_are_said_in_the_json_and_in_lines() {
        use snob_core::budget::Via;
        let session = snob_core::session::Session::from_sessionid(
            "42%3AAbCdEfGh%3A20",
            "Mozilla/5.0",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        let store = Store::in_memory().unwrap();
        let now = snob_core::clock::now_ms();
        let budget = rate_budget::state(store.conn(), None, now).unwrap();
        let answered = |day| Answered {
            day,
            slowing: slowing(Some(std::time::Duration::from_millis(6_400))),
        };
        let day = AnswersDay {
            answers: 12,
            refused: 1,
            median: Some(std::time::Duration::from_millis(420)),
            p95: Some(std::time::Duration::from_millis(3_200)),
            slowest: Some(std::time::Duration::from_millis(3_400)),
            load: Some(("x-ig-capacity-level=3".to_string(), now)),
            via: Some(Via::Browser),
        };
        let answered = answered(day);
        let found = Found {
            sections: args(&["--answers"]).sections(),
            session: &session,
            storage: "file",
            budget: &budget,
            answered: &answered,
            held: None,
            braked: &[],
            lists: &[],
            runs: &[],
            acts: &[],
            jobs: &[],
            now,
        };
        let out = found.json();
        let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["answers"]);
        assert_eq!(out["answers"]["count"], 12);
        assert_eq!(out["answers"]["refused"], 1);
        assert_eq!(out["answers"]["median_ms"], 420);
        assert_eq!(out["answers"]["p95_ms"], 3_200);
        assert_eq!(out["answers"]["via"], "browser");
        assert_eq!(out["answers"]["load"]["value"], "x-ig-capacity-level=3");
        assert_eq!(out["answers"]["slowing_ms"], 6_400);
        let text = found.text();
        assert!(text.starts_with("Answers\n"), "{text}");
        assert!(text.contains("12 answers, 1 refused"), "{text}");
        assert!(
            text.contains("420 ms at the median, 3.2 s or less"),
            "{text}"
        );
        assert!(text.contains("at least 6.4 s"), "{text}");

        // An answer at no more than the shortest step slows nothing, and
        // a day with none says so.
        assert_eq!(slowing(Some(std::time::Duration::from_millis(900))), None);
        let none = Answered::default();
        let found = Found {
            answered: &none,
            ..found
        };
        assert_eq!(found.json()["answers"]["slowing_ms"], Value::Null);
        assert!(
            found.text().contains("None from Instagram"),
            "{}",
            found.text()
        );
    }

    #[test]
    fn a_populated_status_serializes_to_the_contract_shape() {
        use snob_core::Pk;
        use snob_core::budget::{Brake, Via};
        use snob_core::watch::{RecordedOutcome, RunOutcome};
        use snob_engine::jobs::{Detached, Job, Seen, State, Status as JobStatus};
        use snob_store::paths::JobId;
        use snob_store::store::rate_budget::CooldownRecord;
        use std::time::Duration;

        let now = EpochMs::new(1_773_576_000_000);
        let mut session = snob_core::session::Session::from_sessionid(
            "42%3AAbCdEfGh%3A20",
            "Mozilla/5.0",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        session.username = Some("me".into());
        session.created_at = Epoch::new(1_773_489_600);
        session.validated_at = Some(Epoch::new(1_773_570_000));

        let budget = BudgetState {
            pace: BucketState {
                left: 19,
                most: 20,
                free_at: now + Duration::from_secs(4),
            },
            daily: BucketState {
                left: 1_980,
                most: 2_000,
                free_at: now + Duration::from_secs(45),
            },
            writes: BucketState {
                left: 2,
                most: 3,
                free_at: now + Duration::from_secs(900),
            },
            next_write_at: now + Duration::from_secs(900),
            accounts_read: 120,
            accounts_ceiling: 1_000,
            last_cooldown: Some(CooldownRecord {
                until: now + Duration::from_secs(3_600),
                set_at: now - Duration::from_secs(3_600),
                reason: "rate_limit".into(),
                strikes: 1,
            }),
            brake: Some(Brake {
                until: now + Duration::from_secs(3_600),
                accounts: vec![Pk::new(42), Pk::new(99)],
            }),
            sitting: None,
        };
        let answered = Answered {
            day: AnswersDay {
                answers: 15,
                refused: 1,
                median: Some(Duration::from_millis(250)),
                p95: Some(Duration::from_millis(900)),
                slowest: Some(Duration::from_millis(1_400)),
                load: Some(("1/1".into(), now - Duration::from_secs(60))),
                via: Some(Via::Browser),
            },
            slowing: Some(Duration::from_millis(500)),
        };
        let lists = vec![Stored {
            kind: ListKind::Followers,
            latest: Some(snapshots::Snapshot {
                id: 1,
                account_pk: Pk::new(42),
                kind: ListKind::Followers,
                started_at: Epoch::new(1_773_570_000),
                taken_at: Some(Epoch::new(1_773_570_120)),
                member_count: 244,
                declared_count: Some(245),
                next_cursor: None,
            }),
            resumable: true,
            counter: Some(245),
            polled_at: Some(Epoch::new(1_773_575_000)),
        }];
        let runs = vec![(
            watch_store::Run {
                account_pk: Pk::new(7),
                started_at: Epoch::new(1_773_572_000),
                finished_at: Some(Epoch::new(1_773_572_020)),
                requests: 4,
                outcome: Some(RecordedOutcome::Known(RunOutcome::Ok)),
                changes: 2,
                posts_skipped: None,
                due_at: Some(Epoch::new(1_773_571_900)),
                began_at: Some(Epoch::new(1_773_571_960)),
            },
            Some("someone".into()),
        )];
        let acts = vec![mcp_acts::Act {
            at: now - Duration::from_secs(300),
            action: "unfollow".into(),
            target_pk: Some(Pk::new(7)),
            target_name: Some("someone".into()),
            via: "form".into(),
            client: Some("claude".into()),
        }];

        let job_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            job_dir.path().join("err.log"),
            concat!(
                r#"{"progress":{"event":"page","list":"followers","page":3,"accounts":36,"of":120,"sitting":3,"at":1773575990000}}"#,
                "\n"
            ),
        )
        .unwrap();
        let jobs = vec![Seen {
            id: JobId::parse("j0123456789ABCDEFGHJKMNPQRS").unwrap(),
            dir: job_dir.path().to_path_buf(),
            job: Some(Job {
                schema: 1,
                id: "j0123456789ABCDEFGHJKMNPQRS".into(),
                created_at: now.get() - 60_000,
                viewer_pk: Pk::new(42),
                tool: Some("snob_followers".into()),
                arguments: json!({ "account": "someone" }),
                argv: vec!["followers".into(), "someone".into()],
                globals: vec![],
                estimate: None,
                detached: Some(Detached::LeftJob),
                started_by: Some("claude".into()),
                max_wait_seconds: Some(3_600),
            }),
            state: Some(State {
                status: JobStatus::Running,
                started_at: now.get() - 55_000,
                updated_at: now.get() - 5_000,
                finished_at: None,
                exit_number: None,
                cancel: false,
                failure: None,
                supervisor_pid: Some(1234),
                child_pid: Some(5678),
            }),
            held: true,
            stop_asked: false,
            status: JobStatus::Running,
        }];

        let found = Found {
            sections: args(&[]).sections(),
            session: &session,
            storage: "keyring",
            budget: &budget,
            answered: &answered,
            held: Some(now + Duration::from_secs(3_600)),
            braked: &["me".into(), "other".into()],
            lists: &lists,
            runs: &runs,
            acts: &acts,
            jobs: &jobs,
            now,
        };
        insta::assert_snapshot!(serde_json::to_string_pretty(&found.json()).unwrap());
    }
}
