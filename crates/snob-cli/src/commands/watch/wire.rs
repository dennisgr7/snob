//! What a report looks like on the wire.
//!
//! Its own module because it is **protocol, not presentation**: `payload`
//! builds the exact string that gets signed and stored, so an edit here is an
//! edit to what a receiver deduplicates on and to what a signature covers,
//! which is not the same kind of change as rewording a sentence.
//!
//! The posts the monitor reports (`posts`, `counts.posts_new`,
//! `events.posts_new`, `run.posts`) arrived additively, beside the lists:
//! neither number moved, and **`counts.total` still counts the lists' changes
//! and the renames alone**, so no receiver sees a number change its meaning.

use snob_core::Epoch;
use snob_core::model::{User, printable};
use snob_core::watch::{Basis, Rename};

use snob_engine::app::Viewer;
use snob_engine::engine::watch::{
    ListReport, RunTiming, Skipped, TickReport, WatchReport, Watched,
};
use snob_engine::engine::watch_posts::{PostsReport, SavedPost};
use snob_engine::exit::ExitCode;
use snob_engine::posts::{Post, PostKind};

use super::status::{MarkOf, RunOf};

/// `viewer` is the account the check was run as, when one was chosen.
pub(super) fn check_json(
    report: &snob_engine::engine::check::CheckReport,
    viewer: Option<&Viewer>,
) -> serde_json::Value {
    use snob_engine::engine::check::What;

    serde_json::json!({
        "verdict": report.verdict().as_str(),
        "viewer": viewer.map(Viewer::json),
        "checks": report.checked.iter().map(|checked| {
            let (what, detail) = match &checked.what {
                What::NotConfigured => ("config", serde_json::json!(null)),
                What::Schedule { next } => ("schedule", serde_json::json!({ "next": next })),
                What::Session { viewer, backend } => (
                    "session",
                    serde_json::json!({ "viewer": viewer, "storage": backend }),
                ),
                What::Account { target, pk, followers, following, may_run_unattended, lists } => (
                    "account",
                    serde_json::json!({
                        // As the file spells it, through `printable` as every
                        // name is before it reaches a JSON reader.
                        "target": target.as_deref().map(printable),
                        "pk": pk,
                        "followers": followers,
                        "following": following,
                        "may_run_unattended": may_run_unattended,
                        "lists": lists,
                    }),
                ),
                What::Webhook { destination, status, signed } => (
                    "webhook",
                    serde_json::json!({
                        "destination": destination,
                        "status": status,
                        "signed": signed,
                    }),
                ),
                What::Baseline { taken_at } => (
                    "baseline",
                    serde_json::json!({
                        "lists": taken_at.iter()
                            .map(|(kind, at)| serde_json::json!({ "kind": kind.as_str(), "taken_at": at }))
                            .collect::<Vec<_>>(),
                    }),
                ),
                // Additive: a line of its own for an account whose posts are
                // watched, beside its `account` line.
                What::Posts { target, save_to, baseline_at } => (
                    "posts",
                    serde_json::json!({
                        "target": target.as_deref().map(printable),
                        "save_to": save_to,
                        "baseline_at": baseline_at,
                    }),
                ),
            };
            serde_json::json!({
                "what": what,
                "verdict": checked.verdict.as_str(),
                "detail": detail,
                // The sentence a person reads, so it comes from where the
                // sentences are, and this and the terminal report agree by
                // construction.
                "problem": checked.problem.as_ref().map(|p| printable(&super::say::problem_line(p))),
            })
        }).collect::<Vec<_>>(),
    })
}

/// A report, in the one shape every emitter of one uses: the body a webhook
/// receives, each line of the `--json` stream and `diff --json`.
///
/// - **`counts` is separate from `events`**, and redundant with the array
///   lengths on purpose.
/// - **`schema` and `event` are at the top**, so fields can be added later
///   without breaking anybody and a Switch node can tell a heartbeat from a
///   report without looking inside.
/// - **`looked` is not derivable from the arrays.**
/// - **`run.lists` is `looked` at the granularity a receiver needs.**
/// - **The posts sit beside the lists, never inside them**: `posts` at the top,
///   so a reader walking the keys of `lists` meets no third shape, and
///   `run.posts` beside `run.lists`, `null` for an account whose posts are not
///   watched and otherwise `{"skipped": null | "not_verified" | "incomplete" |
///   "hidden" | "no_browser"}`: what tells "no new post" from "could not look".
///   **`counts.total` does not count the new posts**: it is the lists' changes
///   and the renames, as it was before posts were watched, so a receiver's
///   `total > 0` means what it always meant; `counts.posts_new` counts them
///   beside it, and `event` is `watch.changes` when either has news.
fn report_json(
    schema: u32,
    event: &str,
    run: serde_json::Value,
    viewer: &Viewer,
    report: &WatchReport,
) -> serde_json::Value {
    let changes = report.changes();

    serde_json::json!({
        "schema": schema,
        "event": event,
        "run": run,
        // Who read it, which on somebody else's account is not `account`.
        "viewer": viewer.json(),
        // Through `printable`, like every name in this body (see
        // [`account_json`]).
        "account": {
            "pk": report.account_pk,
            "username": report.username.as_deref().map(printable),
            "is_self": report.is_self,
        },
        "lists": {
            "followers": list_json(report.followers.as_ref()),
            "following": list_json(report.following.as_ref()),
        },
        "posts": posts_json(report.posts.as_ref()),
        "counts": {
            "followers_gained": changes.followers.gained.len(),
            "followers_lost":   changes.followers.lost.len(),
            "following_gained": changes.following.gained.len(),
            "following_lost":   changes.following.lost.len(),
            "renamed": changes.renamed.len(),
            "posts_new": report.new_posts(),
            // The lists' and the renames' alone, as before posts were watched.
            "total": changes.len(),
        },
        "events": {
            // The accounts are the same `User` that `snob followers --format
            // json` already emits, plus the address: whoever receives this is
            // usually about to put it in a message, and rebuilding the URL at
            // the other end is exactly where somebody pastes a name without
            // encoding it.
            "followers_gained": changes.followers.gained.iter().map(account_json).collect::<Vec<_>>(),
            "followers_lost":   changes.followers.lost.iter().map(account_json).collect::<Vec<_>>(),
            "following_gained": changes.following.gained.iter().map(account_json).collect::<Vec<_>>(),
            "following_lost":   changes.following.lost.iter().map(account_json).collect::<Vec<_>>(),
            "renamed": changes.renamed.iter().map(rename_json).collect::<Vec<_>>(),
            "posts_new": report.posts.iter()
                .flat_map(|posts| posts.news.iter().map(|post| post_json(post, posts.saved_of(post.pk))))
                .collect::<Vec<_>>(),
        },
    })
}

/// The `run` object, the same in every report and every line of the stream, a
/// failed tick's included, so one reader takes `run.at` off any of them.
///
/// **When it was due, began and ended**, additively: `due_at` is the moment the
/// schedule named (`null` for a run started by hand), `began_at` when the run
/// began and `ended_at` when this account's part of it ended, each with its
/// `_iso`; `late_by` is how many seconds after `due_at` it began, and `late`
/// whether that is past what a timer's look and the jitter explain
/// (`engine::watch::LATE_AFTER`).
fn run_json(
    at: Epoch,
    looked: bool,
    requests: u32,
    lists: serde_json::Value,
    posts: serde_json::Value,
    timing: Option<&RunTiming>,
) -> serde_json::Value {
    serde_json::json!({
        "at": at,
        "at_iso": iso(at),
        "looked": looked,
        "requests": requests,
        "lists": lists,
        "posts": posts,
        "due_at": timing.and_then(|t| t.due_at),
        "due_at_iso": timing.and_then(|t| t.due_at).and_then(iso),
        "began_at": timing.map(|t| t.began_at),
        "began_at_iso": timing.and_then(|t| iso(t.began_at)),
        "ended_at": timing.and_then(|t| t.ended_at),
        "ended_at_iso": timing.and_then(|t| t.ended_at).and_then(iso),
        "late_by": timing.and_then(RunTiming::late_by),
        "late": timing.map(RunTiming::late),
        "tool": { "name": "snob", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// A tick's `run`.
fn tick_run_json(tick: &TickReport) -> serde_json::Value {
    run_json(
        tick.at(),
        tick.looked(),
        tick.requests,
        run_lists_json(tick),
        match tick.posts {
            Some(posts) => serde_json::json!({ "skipped": posts.skipped.map(|s| s.as_str()) }),
            None => serde_json::Value::Null,
        },
        tick.timing.as_ref(),
    )
}

/// The event a report is: news when it holds a change, and otherwise the
/// heartbeat a quiet run is sent as when one is asked for.
pub(super) fn event(has_changes: bool) -> &'static str {
    if has_changes {
        "watch.changes"
    } else {
        "watch.heartbeat"
    }
}

/// What `snob watch status --json` prints.
#[expect(
    clippy::too_many_arguments,
    reason = "one document's worth of what status read, each part its own key"
)]
pub(super) fn status_json(
    configured: bool,
    config_path: &std::path::Path,
    owed: &snob_store::store::deliveries::Owed,
    health: &super::status::Health,
    last_runs: &[RunOf],
    marks: &[MarkOf],
    posts: &[super::status::PostsOf],
    next: Option<super::status::NextRun>,
) -> serde_json::Value {
    use super::status::NextRun;
    serde_json::json!({
        "configured": configured,
        "config_path": config_path.display().to_string(),
        // When the schedule next comes round, before any jitter, worked out
        // with no request: `at` the moment (null while it is due now, or an
        // interval waits for the monitor's first start), `due_now` whether
        // it is owed already. Null with no schedule that can come round.
        "next_run": next.map(|next| match next {
            NextRun::At(at) => serde_json::json!({ "at": at, "at_iso": iso(at), "due_now": false }),
            NextRun::Now => serde_json::json!({ "at": null, "at_iso": null, "due_now": true }),
            NextRun::AfterFirstStart(_) => {
                serde_json::json!({ "at": null, "at_iso": null, "due_now": false })
            }
        }),
        "deliveries": {
            // Split, because "the next run tries these" and "nothing here can
            // send these" are two facts.
            "waiting": owed.waiting,
            "elsewhere": owed.elsewhere,
            // Not owed -- owing has ended.
            "given_up": owed.given_up,
        },
        // The verdict, so a caller reading this does not have to reimplement
        // which combinations of the fields below mean the monitor has stopped
        // doing its job.
        "health": {
            "verdict": health.verdict.as_str(),
            "notes": health.notes,
        },
        // Told apart from the marks below on purpose.
        "last_runs": last_runs.iter().map(|RunOf { run, viewer, .. }| serde_json::json!({
            "viewer": viewer.json(),
            "pk": run.account_pk,
            "at": run.started_at,
            // The token, which is the string this field has always carried —
            // including for a row spelled by a build that is not this one,
            // which is printed back as it was rather than as "unknown".
            "outcome": run.outcome.as_ref().map(|outcome| outcome.as_str()),
            "requests": run.requests,
            "changes": run.changes,
            "posts_skipped": run.posts_skipped,
            // When it was due, began and ended, and how late it began
            // (`watch_runs`, migration 020): null for a run by hand, and for
            // a row from before.
            "due_at": run.due_at,
            "began_at": run.began_at,
            "ended_at": run.finished_at,
            "late_by": super::status::timing_of(run).and_then(|t| t.late_by()),
            "late": super::status::timing_of(run).map(|t| t.late()),
        })).collect::<Vec<_>>(),
        "accounts": marks.iter().map(|MarkOf { mark: m, viewer, .. }| serde_json::json!({
            "viewer": viewer.json(),
            "pk": m.account_pk,
            "kind": m.kind.as_str(),
            "last_reported_at": m.compared_at,
            "has_baseline": m.snapshot_id.is_some(),
        })).collect::<Vec<_>>(),
        // Additive: one row per account whose posts have a baseline, beside
        // `accounts`, which is about the lists.
        "posts": posts.iter().map(|of| serde_json::json!({
            "viewer": of.viewer.json(),
            "pk": of.mark.account_pk,
            "baseline_at": of.mark.baseline_at,
            "read_at": of.mark.read_at,
            "read_at_iso": iso(of.mark.read_at),
            "last_reported_at": of.mark.reported_at,
            "owed_files": of.owed,
        })).collect::<Vec<_>>(),
    })
}

/// A tick's line in the `--json` stream: the body a webhook would be sent for
/// it, less `run.id`.
///
/// **`schema` is here for the same reason it is on the wire.**
pub(super) fn tick_json(tick: &TickReport) -> serde_json::Value {
    report_json(
        PRINTED_SCHEMA,
        event(tick.report.has_changes()),
        tick_run_json(tick),
        &tick.viewer,
        &tick.report,
    )
}

/// What `snob watch diff --json` prints: the stored report, as a line of the
/// stream would carry it, so one reader takes both.
pub(super) fn stored_json(viewer: &Viewer, report: &WatchReport, now: Epoch) -> serde_json::Value {
    report_json(
        PRINTED_SCHEMA,
        event(report.has_changes()),
        run_json(
            now,
            false,
            0,
            serde_json::json!([]),
            serde_json::Value::Null,
            None,
        ),
        viewer,
        report,
    )
}

/// Which lists this run read, and which it refused.
fn run_lists_json(tick: &TickReport) -> serde_json::Value {
    tick.lists
        .iter()
        .map(|l| {
            serde_json::json!({
                "kind": l.kind.as_str(),
                "skipped": l.skipped.map(skipped_token),
            })
        })
        .collect::<Vec<_>>()
        .into()
}

/// The line a tick that failed leaves in the stream.
pub(super) fn failed_tick_json(
    viewer: Option<&Viewer>,
    watched: &Watched,
    error: &anyhow::Error,
    at: Epoch,
    requests: u32,
) -> serde_json::Value {
    serde_json::json!({
        // The failure line carries it too: the README says every line of the
        // `--json` stream does, and a reader checks it before anything else.
        "schema": PRINTED_SCHEMA,
        "viewer": viewer.map(Viewer::json),
        "account": {
            "username": watched.name().map(printable),
            "is_self": watched.name().is_none(),
        },
        "run": run_json(at, false, requests, serde_json::json!([]), serde_json::Value::Null, None),
        "error": {
            "code": snob_engine::exit::from_chain(error).unwrap_or(ExitCode::Error).as_str(),
            "message": printable(&format!("{error:#}")),
        },
    })
}

/// One JSON line.
pub(super) fn json_line(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// The version of the shape a receiver is sent: the webhook body and the
/// preflight `snob watch check` posts.
///
/// One number for both, in one place: separate literals could tell a receiver a
/// report is schema 1 and a preflight schema 2 for the same release.
pub(super) const SENT_SCHEMA: u32 = 1;

/// The version of the shape this tool prints: every line of the `--json`
/// stream, a failed tick's included, and `diff --json`.
pub(super) const PRINTED_SCHEMA: u32 = 2;

/// What goes on the wire: the report, and the id a receiver deduplicates on.
pub(super) fn payload(tick: &TickReport, run_id: &str, event: &str) -> serde_json::Value {
    let mut body = report_json(
        SENT_SCHEMA,
        event,
        tick_run_json(tick),
        &tick.viewer,
        &tick.report,
    );
    body["run"]["id"] = serde_json::json!(run_id);
    body
}

/// What `snob watch check` posts.
pub(super) fn preflight_body(run_id: &str, at: Epoch) -> serde_json::Value {
    serde_json::json!({
        "schema": SENT_SCHEMA,
        "event": snob_engine::engine::check::PREFLIGHT_EVENT,
        "run": {
            "id": run_id,
            "at": at,
            "tool": { "name": "snob", "version": env!("CARGO_PKG_VERSION") },
        },
        "note": "snob watch check: this is not a report, and nothing is queued",
    })
}

/// What `snob watch deliver` did: how many owed reports it tried, at which
/// address (as `check` shows it, any path secret cut), and what is still
/// queued, for this address and for others.
pub(super) fn deliver_json(
    destination: &str,
    tried: usize,
    left: &snob_store::store::deliveries::Owed,
) -> serde_json::Value {
    serde_json::json!({
        "destination": destination,
        "tried": tried,
        "waiting": left.waiting,
        "elsewhere": left.elsewhere,
    })
}

/// What a preflight says to a person, in every format: in `json` beside
/// `note`, and alone where the format is the summary.
pub(super) const PREFLIGHT_SUMMARY: &str =
    "snob watch check: this address works. This is a test, not a report.";

/// One account, as an automation wants it.
///
/// **Both names through `printable`, as `snob followers --format json` has
/// them** (`output::readable`, and AGENTS.md, "Output contract").
fn account_json(user: &User) -> serde_json::Value {
    let readable = User {
        username: user.safe_username(),
        full_name: user.safe_full_name(),
        ..user.clone()
    };
    let mut value = serde_json::to_value(&readable).unwrap_or(serde_json::Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "profile_url".to_string(),
            serde_json::Value::String(user.profile_url()),
        );
    }
    value
}

/// The stable name of why a list was left out.
fn skipped_token(skipped: Skipped) -> &'static str {
    match skipped {
        Skipped::NobodyLooked(_) => "not_verified",
        Skipped::Incomplete(..) => "incomplete",
    }
}

fn list_json(report: Option<&ListReport>) -> serde_json::Value {
    let Some(report) = report else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        // A stable token, so a caller can tell "nothing changed" from "this is
        // the first look" without reading a sentence.
        "basis": basis_token(report.basis),
        "since": report.since,
        "since_iso": report.since.and_then(iso),
        "until": report.until,
        "until_iso": iso(report.until),
        "count": report.total,
    })
}

/// What a run concluded about an account's posts; `null` when it does not
/// watch them, or concluded nothing about them this run (`run.posts` says
/// why).
fn posts_json(report: Option<&PostsReport>) -> serde_json::Value {
    let Some(report) = report else {
        return serde_json::Value::Null;
    };
    serde_json::json!({
        "basis": match report.basis {
            snob_core::watch::posts::Basis::Baseline => "baseline",
            snob_core::watch::posts::Basis::Compared => "compared",
        },
        "since": report.since,
        "since_iso": report.since.and_then(iso),
        "until": report.until,
        "until_iso": iso(report.until),
        "pages_read": report.pages,
        "complete": report.complete,
    })
}

/// One new post, as a receiver building a message wants it.
///
/// **No media address**, for the reason `answer::without_media_addresses` gives
/// an agent: a signed CDN address expires and weighs hundreds of bytes of query
/// string.
fn post_json(post: &Post, saved: Option<&SavedPost>) -> serde_json::Value {
    serde_json::json!({
        "pk": post.pk.to_string(),
        "code": post.code,
        "url": post.link(),
        "kind": kind_token(post.kind),
        "taken_at": post.taken_at,
        "taken_at_iso": iso(post.taken_at),
        "pinned": post.pinned,
        "item_count": post.items.len(),
        "caption": printable(&post.caption.join("\n")),
        "saved": saved.map(|saved| serde_json::json!({
            "directory": saved.directory,
            "files": saved.files,
            "failed": saved.failed,
            "left": saved.left,
        })),
    })
}

/// The stable word for what a post is.
fn kind_token(kind: PostKind) -> &'static str {
    match kind {
        PostKind::Photo => "photo",
        PostKind::Video => "video",
        PostKind::Reel => "reel",
        PostKind::Carousel(_) => "carousel",
    }
}

/// A rename, both names through `printable` for the reason [`account_json`]
/// gives.
fn rename_json(rename: &Rename) -> serde_json::Value {
    serde_json::json!({
        "pk": rename.pk,
        "from": printable(&rename.from),
        "to": printable(&rename.to),
        "changed_at": rename.at,
        "changed_at_iso": iso(rename.at),
    })
}

/// A moment as RFC 3339 in UTC, beside the epoch seconds every moment here is
/// already given in: a receiver building a message or a spreadsheet row wants a
/// date it can show, and converting an epoch is the step a no-code tool makes
/// hard.
fn iso(at: Epoch) -> Option<String> {
    chrono::DateTime::from_timestamp(at.get(), 0)
        .map(|moment| moment.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// A body ready to be signed and sent, and the type it is sent as.
pub(super) struct Outgoing {
    pub(super) body: String,
    pub(super) content_type: &'static str,
}

/// The longest summary each service shows whole, in characters for the chat
/// services and bytes for ntfy, from their documentation: Discord refuses
/// `content` over 2,000 characters; Slack truncates `text` past 40,000 and
/// recommends staying under 4,000; ntfy turns a message over 4,096 bytes into
/// an attachment. A little under each, for the line that says what was cut.
const DISCORD_MOST: usize = 1_900;
const SLACK_MOST: usize = 3_900;
const NTFY_MOST_BYTES: usize = 3_900;

/// What a webhook in `format` is sent, from the JSON a report or a preflight is
/// and the summary a person reads.
///
/// **This is protocol**: what comes out is the exact string that is signed,
/// queued and sent again on a retry.
pub(super) fn outgoing(
    format: snob_store::config::WebhookFormat,
    mut json: serde_json::Value,
    summary: &str,
) -> Outgoing {
    use snob_store::config::WebhookFormat;

    const PLAIN: &str = "text/plain; charset=utf-8";
    let summary = printable_lines(summary);
    match format {
        WebhookFormat::Json => {
            json["summary"] = serde_json::Value::String(summary);
            Outgoing {
                body: json_line(&json),
                content_type: snob_engine::watch::webhook::JSON,
            }
        }
        WebhookFormat::Slack => {
            let text = fit(&summary, SLACK_MOST, chars)
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            Outgoing {
                body: json_line(&serde_json::json!({ "text": text })),
                content_type: snob_engine::watch::webhook::JSON,
            }
        }
        WebhookFormat::Discord => Outgoing {
            body: json_line(&serde_json::json!({
                // Discord draws `[words](address)` as the words alone, linked:
                // a display name or a caption written so would put a link to
                // anywhere in the report under words of its choosing. With the
                // parenthesis escaped, both are shown as they were written.
                "content": fit(&summary.replace("](", "]\\("), DISCORD_MOST, chars),
                "allowed_mentions": { "parse": [] },
            })),
            content_type: snob_engine::watch::webhook::JSON,
        },
        WebhookFormat::Ntfy => Outgoing {
            body: fit(&summary, NTFY_MOST_BYTES, str::len),
            content_type: PLAIN,
        },
        WebhookFormat::Text => Outgoing {
            body: summary,
            content_type: PLAIN,
        },
    }
}

/// Every line through `printable`, the line breaks kept: the summary is made
/// of snob's sentences and names already filtered, and this is the last
/// look before a chat service draws it.
fn printable_lines(text: &str) -> String {
    text.lines().map(printable).collect::<Vec<_>>().join("\n")
}

/// The whole lines of `text` that fit in `most`, as `size` measures, and a
/// line saying how many were left out.
fn fit(text: &str, most: usize, size: fn(&str) -> usize) -> String {
    if size(text) <= most {
        return text.to_string();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = String::new();
    let mut count = 0;
    for line in &lines {
        if size(&kept) + size(line) + 1 > most {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
        count += 1;
    }
    kept.push_str(&format!(
        "... and {} more lines, left out here; the json format carries every account.",
        lines.len() - count
    ));
    kept
}

/// What a chat service counts.
fn chars(text: &str) -> usize {
    text.chars().count()
}

/// The stable name of what a list's report is.
fn basis_token(basis: Basis) -> &'static str {
    match basis {
        Basis::Baseline { .. } => "baseline",
        Basis::Unchanged { .. } => "unchanged",
        Basis::Compare { .. } => "compared",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::watch::fixtures::{list, post, posts_report, report_with, user};
    use snob_core::Pk;
    use snob_core::model::ListKind;
    use snob_core::watch::{Basis, ListDiff, Rename};
    use snob_engine::engine::Provenance;
    use snob_engine::engine::watch::TickList;

    /// The tokens are what a caller branches on, so they are asserted rather
    /// than left to whatever the enum happens to be called.
    #[test]
    fn the_json_carries_stable_tokens_for_each_basis() {
        for (basis, token) in [
            (Basis::Baseline { snapshot_id: 1 }, "baseline"),
            (Basis::Unchanged { snapshot_id: 1 }, "unchanged"),
            (
                Basis::Compare {
                    before: 1,
                    after: 2,
                },
                "compared",
            ),
        ] {
            assert_eq!(basis_token(basis), token);
        }
    }

    fn a_body() -> serde_json::Value {
        payload(
            &TickReport::for_test(report_with(None, vec![]), 14, Epoch::new(1_700_000_000)),
            "run-1",
            "watch.changes",
        )
    }

    /// The JSON a receiver already reads, with the sentence and the dates
    /// added beside it and nothing taken away.
    #[test]
    fn the_json_format_is_the_report_with_a_summary_and_dates_beside_the_epochs() {
        use snob_store::config::WebhookFormat;
        let made = outgoing(WebhookFormat::Json, a_body(), "Changes for @me\n  one");
        assert_eq!(made.content_type, "application/json");
        let sent: serde_json::Value = serde_json::from_str(&made.body).unwrap();
        assert_eq!(sent["schema"], SENT_SCHEMA);
        assert_eq!(sent["summary"], "Changes for @me\n  one");
        assert_eq!(sent["run"]["at"], 1_700_000_000);
        assert_eq!(sent["run"]["at_iso"], "2023-11-14T22:13:20Z");
    }

    /// Each chat service gets the summary in the one shape its incoming
    /// webhook takes, and none of them is handed a way to notify anybody.
    #[test]
    fn the_chat_formats_carry_the_summary_in_each_services_shape() {
        use snob_store::config::WebhookFormat;
        let summary = "@everyone <b> & co\u{202e}";

        let slack = outgoing(WebhookFormat::Slack, a_body(), summary);
        let slack: serde_json::Value = serde_json::from_str(&slack.body).unwrap();
        assert_eq!(slack["text"], "@everyone &lt;b&gt; &amp; co", "{slack}");
        assert_eq!(slack.as_object().map(|o| o.len()), Some(1));

        let discord = outgoing(WebhookFormat::Discord, a_body(), summary);
        let discord: serde_json::Value = serde_json::from_str(&discord.body).unwrap();
        assert_eq!(discord["content"], "@everyone <b> & co");
        assert_eq!(discord["allowed_mentions"]["parse"], serde_json::json!([]));

        for format in [WebhookFormat::Ntfy, WebhookFormat::Text] {
            let made = outgoing(format, a_body(), summary);
            assert_eq!(made.body, "@everyone <b> & co", "filtered, and plain");
            assert!(made.content_type.starts_with("text/plain"));
        }
    }

    /// A display name or a caption written as a Discord link is shown as it
    /// was written, never as a link under words of its author's choosing; the
    /// other formats, which draw no such link, carry it untouched.
    #[test]
    fn a_name_written_as_a_discord_link_is_not_drawn_as_one() {
        use snob_store::config::WebhookFormat;
        let summary = "    @someone ([Instagram login](https://elsewhere.test))";

        let discord = outgoing(WebhookFormat::Discord, a_body(), summary);
        let discord: serde_json::Value = serde_json::from_str(&discord.body).unwrap();
        assert_eq!(
            discord["content"],
            "    @someone ([Instagram login]\\(https://elsewhere.test))"
        );

        let json = outgoing(WebhookFormat::Json, a_body(), summary);
        let json: serde_json::Value = serde_json::from_str(&json.body).unwrap();
        assert_eq!(json["summary"], summary);
        assert_eq!(
            outgoing(WebhookFormat::Text, a_body(), summary).body,
            summary
        );
    }

    /// Past what the service shows, the summary is cut at a line, and says
    /// how much was left out rather than ending mid-name.
    #[test]
    fn a_summary_too_long_for_discord_is_cut_at_a_line_and_says_so() {
        use snob_store::config::WebhookFormat;
        let summary = (0..400)
            .map(|n| format!("    @user{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let made = outgoing(WebhookFormat::Discord, a_body(), &summary);
        let content = serde_json::from_str::<serde_json::Value>(&made.body).unwrap()["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(content.chars().count() <= 2_000, "{}", content.len());
        assert!(content.ends_with("the json format carries every account."));
        assert!(content.lines().all(|line| !line.ends_with("@use")));
    }

    /// Where the README publishes the body, it has to be the body: a receiver is
    /// written from the document, and `run.id` and `run.at` are what the
    /// surrounding prose depends on.
    ///
    /// It compares keys and not values, because the block is an example and
    /// abbreviates the arrays. What it may not do is name a key the payload does
    /// not emit, or leave one out of `run`.
    ///
    /// The version inside `tool` is deliberately not asserted: doing that makes
    /// every release a README edit.
    #[test]
    fn the_readme_publishes_the_body_that_goes_out() {
        // Walks up from this crate until a `Cargo.lock` shows up, the way the
        // source-reading guards in `snob-core` do, and answers nothing from a
        // packaged build where there is no repository to read.
        let Some(root) = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .find(|d| d.join("Cargo.lock").is_file())
        else {
            return;
        };

        let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
        // A README that shows no payload publishes nothing to keep in step.
        if !readme.contains("```json") {
            return;
        }
        assert_eq!(
            readme.matches("```json").count(),
            1,
            "the payload is the only JSON block, and this test takes the first"
        );
        let block = readme
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("the README shows the payload");
        let published: serde_json::Value =
            serde_json::from_str(block).expect("the README's example has to be JSON");

        let real = {
            let mut report = report_with(None, vec![]);
            report.posts = Some(posts_report(vec![post(1, "a post")]));
            let mut tick = TickReport::for_test(report, 14, Epoch::new(1_700_000_000));
            tick.posts = Some(snob_engine::engine::watch::TickPosts { skipped: None });
            payload(&tick, "run-1", "watch.changes")
        };

        let keys = |value: &serde_json::Value| {
            let mut names: Vec<String> = value
                .as_object()
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            names.sort();
            names
        };
        assert_eq!(
            keys(&published["run"]),
            keys(&real["run"]),
            "the `run` object is published in full or not at all"
        );

        // One direction only: the example abbreviates, and it is allowed to.
        // What it may not do is publish a field nothing sends.
        fn only_real_keys(published: &serde_json::Value, real: &serde_json::Value, path: &str) {
            let (Some(published), Some(real)) = (published.as_object(), real.as_object()) else {
                return;
            };
            for (key, value) in published {
                let here = format!("{path}.{key}");
                let counterpart = real.get(key).unwrap_or_else(|| {
                    panic!("the README publishes \"{here}\", which is not sent")
                });
                only_real_keys(value, counterpart, &here);
            }
        }
        only_real_keys(&published, &real, "");

        // Left out, a receiver serving several accounts would be written
        // without the one field that says whose run it was.
        assert!(
            published.get("viewer").is_some(),
            "the README shows who read the account"
        );
    }

    /// Every message this tool emits can be version-checked, and the two that
    /// go to a receiver put the id and the moment in the same place.
    ///
    /// The `--json` stream is the third emitter, under a number of its own. It
    /// carries no `run.id`, and that is right rather than an omission: an id
    /// exists to deduplicate an at-least-once delivery, and a line written once
    /// to a local file is not one.
    #[test]
    fn every_message_carries_the_schema() {
        let report = payload(
            &TickReport::for_test(report_with(None, vec![]), 0, Epoch::new(1_700_000_000)),
            "run-1",
            "watch.changes",
        );
        let preflight = preflight_body("run-2", Epoch::new(1_700_000_000));
        let streamed = tick_json(&TickReport::for_test(
            report_with(None, vec![]),
            0,
            Epoch::new(1_700_000_000),
        ));

        for (which, message, schema) in [
            ("report", &report, 1),
            ("preflight", &preflight, 1),
            ("stream line", &streamed, 2),
        ] {
            assert_eq!(
                message["schema"], schema,
                "{which} cannot be version-checked"
            );
            assert_eq!(
                message["run"]["at"], 1_700_000_000,
                "{which} does not say when"
            );
            assert!(
                message.get("at").is_none(),
                "{which} still has the moment at the top level"
            );
        }

        for (which, message) in [("report", &report), ("preflight", &preflight)] {
            assert!(
                message["event"].as_str().is_some(),
                "{which} has nothing for a Switch node to read"
            );
            assert!(
                message["run"]["id"].as_str().is_some(),
                "{which} does not say which run it is"
            );
            assert!(
                message.get("run_id").is_none(),
                "{which} still has the id at the top level"
            );
        }

        // And the name is one constant, so the header and the body cannot come
        // to disagree.
        assert_eq!(
            preflight["event"],
            snob_engine::engine::check::PREFLIGHT_EVENT
        );
        assert_eq!(
            preflight["event"], "watch.preflight",
            "the name on the wire"
        );
    }

    /// A refused list is not a quiet one, and the body has to say which it was.
    ///
    /// `tick` drops a list it could not verify before the comparison, so it
    /// reaches `payload` as `None` and `list_json` turns it into `null` — the
    /// same `null` an account with no capture of that list produces, with
    /// `counts.following_lost` at `0` in both.
    ///
    /// `run.looked` cannot resolve it, and it is asserted equal here to say so:
    /// it is `any`, not `all`, so a run that read followers and was refused
    /// following reports `true`. Which list is the question.
    #[test]
    fn a_refused_list_is_not_reported_as_a_quiet_one() {
        let body = |skipped| {
            let mut tick = TickReport::for_test(
                report_with(
                    Some(list(
                        Basis::Compare {
                            before: 1,
                            after: 2,
                        },
                        ListDiff {
                            gained: vec![user(Pk::new(1), "arrived")],
                            lost: vec![],
                        },
                        Some(Epoch::new(1_000)),
                    )),
                    vec![],
                ),
                7,
                Epoch::new(1_700_000_000),
            );
            tick.lists = vec![
                TickList {
                    kind: ListKind::Followers,
                    skipped: None,
                },
                TickList {
                    kind: ListKind::Following,
                    skipped,
                },
            ];
            payload(&tick, "run-1", "watch.changes")
        };

        let refused = body(Some(Skipped::NobodyLooked(Provenance::PollFailed)));
        let read = body(None);

        // The arrays cannot tell them apart and neither can the counts.
        assert_eq!(refused["lists"], read["lists"]);
        assert_eq!(refused["counts"], read["counts"]);
        assert_eq!(
            refused["run"]["looked"], read["run"]["looked"],
            "`looked` is `any`, so it says `true` for both"
        );

        assert_ne!(
            refused["run"]["lists"], read["run"]["lists"],
            "a receiver has no field to read the refusal from"
        );
        assert_eq!(refused["run"]["lists"][1]["kind"], "following");
        assert_eq!(refused["run"]["lists"][1]["skipped"], "not_verified");
        assert_eq!(
            read["run"]["lists"][1]["skipped"],
            serde_json::Value::Null,
            "a list that was read carries no refusal"
        );
    }

    /// An event line says when it happened. The lists' own moments go away
    /// with a list that is refused, so without `run.at` every line of a
    /// cooldown would be byte-identical, and the file could not be queried by
    /// time, windowed or deduplicated.
    #[test]
    fn an_event_line_says_when_it_happened() {
        let quiet_run_at = |at| {
            let mut tick = TickReport::for_test(report_with(None, vec![]), 0, at);
            tick.lists = vec![TickList {
                kind: ListKind::Followers,
                skipped: Some(Skipped::NobodyLooked(Provenance::Cooldown)),
            }];
            tick_json(&tick)
        };

        let first = quiet_run_at(Epoch::new(1_700_000_000));
        let second = quiet_run_at(Epoch::new(1_700_021_600));

        assert_eq!(first["run"]["at"], 1_700_000_000);
        assert_ne!(
            first, second,
            "six hours apart and the same bytes: nothing in the file can date a run"
        );

        // The moment is the tick's own, so the file and whatever the webhook
        // delivered can be joined on it.
        let tick = TickReport::for_test(report_with(None, vec![]), 0, Epoch::new(1_700_000_000));
        assert_eq!(
            tick_json(&tick)["run"]["at"],
            payload(&tick, "run-1", "watch.changes")["run"]["at"],
            "one event, one moment"
        );
    }

    /// A failed tick leaves a line in the stream, not only an English
    /// paragraph on standard error that a consumer of the file is not reading.
    ///
    /// The line has to be readable by the same reader the successful lines
    /// have, which is why `run` is the same object. `error` is what tells them
    /// apart, and a successful line must not have one.
    #[test]
    fn a_failed_tick_leaves_a_line_in_the_stream() {
        let error = anyhow::anyhow!("@friend's account is private");
        let line = failed_tick_json(
            None,
            &Watched::consented("friend".into(), snob_engine::engine::watch::Consent),
            &error,
            Epoch::new(1_700_000_000),
            1,
        );

        assert_eq!(
            line["run"]["at"], 1_700_000_000,
            "the gap in the file has to be datable, which is the whole of it"
        );
        assert_eq!(line["run"]["looked"], false);
        assert_eq!(line["run"]["requests"], 1, "the poll was charged");
        assert_eq!(line["account"]["username"], "friend");
        assert_eq!(
            line["error"]["code"], "error",
            "the vocabulary of the exit table, not free text"
        );

        // And the two kinds of line are told apart by the field itself, not by
        // what is missing from the rest of the object.
        let ok = tick_json(&TickReport::for_test(
            report_with(None, vec![]),
            0,
            Epoch::new(1_700_000_000),
        ));
        assert!(
            ok.get("error").is_none(),
            "a run that worked must not look like one that failed"
        );
        assert_eq!(line["schema"], ok["schema"], "one stream, one number");
        let keys = |run: &serde_json::Value| {
            run.as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert_eq!(
            keys(&line["run"]),
            keys(&ok["run"]),
            "the same `run`, so one reader takes it off every line"
        );

        // One object, one line, in both modes. `run_accounts` prints one per
        // account with nothing wrapping them, so a line laid out over several
        // of them stops parsing the moment a second account is watched.
        assert!(!json_line(&line).contains('\n'));
        assert!(
            !json_line(&ok).contains('\n'),
            "the successful line is a line too"
        );
    }

    /// A line of the stream is the body a webhook is sent, less the id: one
    /// shape, so one reader takes both, and the event is the one `--heartbeat`
    /// would send when nothing changed.
    #[test]
    fn a_line_is_the_body_without_its_id() {
        let changed = TickReport::for_test(
            report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff {
                        gained: vec![user(Pk::new(1), "arrived")],
                        lost: vec![],
                    },
                    Some(Epoch::new(1_000)),
                )),
                vec![],
            ),
            3,
            Epoch::new(1_700_000_000),
        );
        let quiet = TickReport::for_test(report_with(None, vec![]), 1, Epoch::new(1_700_000_000));

        for (tick, event) in [(&changed, "watch.changes"), (&quiet, "watch.heartbeat")] {
            let mut body = payload(tick, "run-1", event);
            let id = body["run"].as_object_mut().and_then(|run| run.remove("id"));
            assert_eq!(id, Some(serde_json::json!("run-1")));
            body["schema"] = serde_json::json!(PRINTED_SCHEMA);

            assert_eq!(tick_json(tick), body);
        }
    }

    /// `diff --json` prints what is stored in the stream's shape, and its `run`
    /// says that nothing was looked at or spent rather than being left out.
    #[test]
    fn a_stored_report_is_printed_in_the_streams_shape() {
        let report = report_with(
            Some(list(
                Basis::Compare {
                    before: 1,
                    after: 2,
                },
                ListDiff {
                    gained: vec![],
                    lost: vec![user(Pk::new(2), "left")],
                },
                Some(Epoch::new(1_000)),
            )),
            vec![],
        );
        let viewer = Viewer {
            pk: Pk::new(42),
            username: Some("me".into()),
        };
        let stored = stored_json(&viewer, &report, Epoch::new(1_700_000_000));

        let mut tick = TickReport::for_test(report, 0, Epoch::new(1_700_000_000));
        tick.viewer = viewer;
        let line = tick_json(&tick);
        let keys = |value: &serde_json::Value| {
            value
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
        };
        assert_eq!(keys(&stored), keys(&line), "one shape");
        assert_eq!(keys(&stored["run"]), keys(&line["run"]));

        assert_eq!(stored["schema"], 2);
        assert_eq!(stored["event"], "watch.changes");
        assert_eq!(stored["counts"]["followers_lost"], 1);
        assert_eq!(stored["events"]["followers_lost"][0]["username"], "left");
        assert_eq!(stored["run"]["at"], 1_700_000_000);
        assert_eq!(stored["run"]["looked"], false, "nothing was read");
        assert_eq!(stored["run"]["requests"], 0, "nothing was spent");
        assert_eq!(stored["run"]["lists"], serde_json::json!([]));
    }

    /// The shape a stranger's automation branches on, pinned to a literal.
    ///
    /// This is the one output of the tool that somebody else's workflow reads,
    /// and a key renamed by accident breaks something built months ago with no
    /// error anywhere. Comparing against a literal means a change to the
    /// contract has to be a change somebody made on purpose.
    ///
    /// Both of the fields that move in production are arguments here: `run.id`
    /// is random and `run.at` is the tick's own moment, so the literal can
    /// carry them rather than the comparison having to skip them.
    #[test]
    fn the_payload_has_the_shape_a_receiver_was_promised() {
        let tick = TickReport::for_test(
            report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff {
                        gained: vec![user(Pk::new(1), "arrived")],
                        lost: vec![user(Pk::new(2), "left")],
                    },
                    Some(Epoch::new(1_000)),
                )),
                vec![Rename {
                    pk: Pk::new(7),
                    history_id: 7,
                    from: "before".into(),
                    to: "after".into(),
                    at: Epoch::new(1_500),
                }],
            ),
            14,
            Epoch::new(1_700_000_000),
        );

        let payload = payload(&tick, "run-1", "watch.changes");

        assert_eq!(
            payload,
            serde_json::json!({
                "schema": 1,
                "event": "watch.changes",
                "run": {
                    "id": "run-1",
                    "at": 1_700_000_000,
                    "at_iso": "2023-11-14T22:13:20Z",
                    "looked": false,
                    "requests": 14,
                    "lists": [],
                    "posts": null,
                    "due_at": null,
                    "due_at_iso": null,
                    "began_at": null,
                    "began_at_iso": null,
                    "ended_at": null,
                    "ended_at_iso": null,
                    "late_by": null,
                    "late": null,
                    "tool": { "name": "snob", "version": env!("CARGO_PKG_VERSION") },
                },
                "viewer": { "pk": 42, "username": "me" },
                "account": { "pk": 42, "username": "me", "is_self": true },
                "lists": {
                    "followers": {
                        "basis": "compared",
                        "since": 1_000,
                        "since_iso": "1970-01-01T00:16:40Z",
                        "until": 2_000,
                        "until_iso": "1970-01-01T00:33:20Z",
                        "count": 10,
                    },
                    "following": null,
                },
                "posts": null,
                "counts": {
                    "followers_gained": 1,
                    "followers_lost": 1,
                    "following_gained": 0,
                    "following_lost": 0,
                    "renamed": 1,
                    "posts_new": 0,
                    "total": 3,
                },
                "events": {
                    "followers_gained": [{
                        "pk": 1,
                        "username": "arrived",
                        "profile_url": "https://www.instagram.com/arrived/",
                    }],
                    "followers_lost": [{
                        "pk": 2,
                        "username": "left",
                        "profile_url": "https://www.instagram.com/left/",
                    }],
                    "following_gained": [],
                    "following_lost": [],
                    "renamed": [{
                        "pk": 7,
                        "from": "before",
                        "to": "after",
                        "changed_at": 1_500,
                        "changed_at_iso": "1970-01-01T00:25:00Z",
                    }],
                    "posts_new": [],
                },
            })
        );
    }

    /// A report of new posts and nothing else is news: its event is
    /// `watch.changes`, `counts.posts_new` counts them, and `counts.total`,
    /// which counts the lists' changes, stays 0 as it always would have.
    #[test]
    fn a_report_of_posts_alone_is_news_and_counts_in_the_total() {
        let mut report = report_with(None, vec![]);
        report.posts = Some(posts_report(vec![post(1, "first"), post(2, "second")]));
        let tick = TickReport::for_test(report, 5, Epoch::new(1_700_000_000));
        assert!(tick.report.has_changes());
        let body = payload(&tick, "run-1", event(tick.report.has_changes()));
        assert_eq!(body["event"], "watch.changes");
        assert_eq!(body["counts"]["posts_new"], 2);
        assert_eq!(
            body["counts"]["total"], 0,
            "the lists' total keeps its meaning: it never counted posts"
        );
        assert_eq!(body["posts"]["basis"], "compared");
        assert_eq!(body["posts"]["pages_read"], 1);
        assert_eq!(body["posts"]["complete"], true);
        let first = &body["events"]["posts_new"][0];
        assert_eq!(first["pk"], "3800000000000000001");
        assert_eq!(first["kind"], "photo");
        assert_eq!(first["caption"], "first");
        assert!(
            first["url"]
                .as_str()
                .unwrap()
                .starts_with("https://www.instagram.com/p/"),
            "{first}"
        );
        assert_eq!(first["saved"], serde_json::Value::Null);
    }

    /// A run that watches posts and could not look at them says why, so it
    /// does not read as a run that found no new post.
    #[test]
    fn a_run_that_could_not_look_at_posts_does_not_read_as_no_new_posts() {
        use snob_engine::engine::watch::TickPosts;
        use snob_engine::engine::watch_posts::PostsSkipped;

        let mut tick =
            TickReport::for_test(report_with(None, vec![]), 0, Epoch::new(1_700_000_000));
        tick.posts = Some(TickPosts {
            skipped: Some(PostsSkipped::NoBrowser),
        });
        let body = payload(&tick, "run-1", "watch.heartbeat");
        assert_eq!(body["run"]["posts"]["skipped"], "no_browser");
        assert_eq!(body["posts"], serde_json::Value::Null);
        assert_eq!(body["run"]["looked"], false);

        tick.posts = Some(TickPosts { skipped: None });
        tick.report.posts = Some(posts_report(Vec::new()));
        let body = payload(&tick, "run-1", "watch.heartbeat");
        assert_eq!(body["run"]["posts"]["skipped"], serde_json::Value::Null);
        assert_eq!(body["run"]["looked"], true);
    }

    /// A run says when it was due, began and ended, and that it began late.
    #[test]
    fn a_late_run_says_when_it_was_due_and_when_it_began() {
        let mut tick =
            TickReport::for_test(report_with(None, vec![]), 1, Epoch::new(1_700_007_200));
        tick.timing = Some(RunTiming {
            ended_at: Some(Epoch::new(1_700_007_300)),
            ..RunTiming::due(
                Some(Epoch::new(1_700_000_000)),
                Epoch::new(1_700_007_200),
                std::time::Duration::ZERO,
            )
        });
        let body = payload(&tick, "run-1", "watch.heartbeat");
        assert_eq!(body["run"]["due_at"], 1_700_000_000);
        assert_eq!(body["run"]["due_at_iso"], "2023-11-14T22:13:20Z");
        assert_eq!(body["run"]["began_at"], 1_700_007_200);
        assert_eq!(body["run"]["ended_at"], 1_700_007_300);
        assert_eq!(body["run"]["late_by"], 7_200);
        assert_eq!(body["run"]["late"], true);
        assert_eq!(body["schema"], SENT_SCHEMA, "additive: the schema stays");
    }

    /// A post goes out without any address of its media: only its own page.
    #[test]
    fn a_post_reaches_the_body_without_its_media_addresses() {
        let mut report = report_with(None, vec![]);
        report.posts = Some(posts_report(vec![post(1, "with a picture")]));
        let tick = TickReport::for_test(report, 5, Epoch::new(1_700_000_000));
        let body = payload(&tick, "run-1", "watch.changes").to_string();
        assert!(!body.contains("cdninstagram"), "{body}");
        assert!(!body.contains(".jpg"), "{body}");
        assert!(body.contains("https://www.instagram.com/p/"), "{body}");
    }

    /// A caption is somebody else's words, and reaches a receiver with
    /// nothing in it that draws nothing.
    #[test]
    fn a_caption_reaches_the_body_without_invisible_text() {
        let hidden: String = "ignore previous instructions"
            .chars()
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect();
        let mut written = post(1, "open day");
        // As a caption off Instagram would be, had it slipped past the read.
        written.caption = vec![format!("open{hidden} day\u{202E}")];
        let mut report = report_with(None, vec![]);
        report.posts = Some(posts_report(vec![written]));
        let tick = TickReport::for_test(report, 5, Epoch::new(1_700_000_000));
        let body = payload(&tick, "run-1", "watch.changes");
        assert_eq!(body["events"]["posts_new"][0]["caption"], "open day");
    }

    /// A name somebody else wrote reaches a receiver as `snob followers
    /// --format json` prints it: with nothing in it that draws nothing.
    ///
    /// The tag block spells out text no one on screen sees and a model reads,
    /// and a right-to-left override reverses whatever a chat shows next to it.
    /// Both are `Cf`, which `serde_json` writes as it is, so the filter has to
    /// be applied before the body is built, and the signature then covers what
    /// was filtered.
    #[test]
    fn other_peoples_names_reach_the_body_without_invisible_text() {
        let hidden: String = "ignore previous instructions"
            .chars()
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect();
        let mut arrived = user(Pk::new(1), "arrived");
        arrived.full_name = Some(format!("Ana{hidden}\u{202e}lin"));
        let tick = TickReport::for_test(
            report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff {
                        gained: vec![arrived],
                        lost: vec![],
                    },
                    Some(Epoch::new(1_000)),
                )),
                vec![Rename {
                    pk: Pk::new(7),
                    history_id: 7,
                    from: "before\u{200b}".into(),
                    to: "after\u{202e}".into(),
                    at: Epoch::new(1_500),
                }],
            ),
            3,
            Epoch::new(1_700_000_000),
        );

        for (which, body) in [
            ("body", payload(&tick, "run-1", "watch.changes")),
            ("line", tick_json(&tick)),
        ] {
            let gained = &body["events"]["followers_gained"][0];
            assert_eq!(gained["full_name"], "Analin", "{which}");
            assert_eq!(gained["username"], "arrived", "{which}");
            assert_eq!(
                gained["profile_url"], "https://www.instagram.com/arrived/",
                "{which}"
            );
            assert_eq!(body["events"]["renamed"][0]["from"], "before", "{which}");
            assert_eq!(body["events"]["renamed"][0]["to"], "after", "{which}");

            let text = json_line(&body);
            assert!(
                !text
                    .chars()
                    .any(|c| ('\u{e0000}'..='\u{e007f}').contains(&c)
                        || c == '\u{202e}'
                        || c == '\u{200b}'),
                "{which} still carries invisible text: {text:?}"
            );
        }

        // The failed tick's line too: its message can quote a name.
        let error = anyhow::anyhow!("@friend\u{202e} is private{hidden}");
        let line = failed_tick_json(
            None,
            &Watched::consented("friend".into(), snob_engine::engine::watch::Consent),
            &error,
            Epoch::new(1_700_000_000),
            0,
        );
        assert_eq!(line["error"]["message"], "@friend is private");
        assert_eq!(line["account"]["username"], "friend");
    }

    /// Every line and body names the account it was read as, which on
    /// somebody else's account is not `account`: one webhook carries the
    /// reports of every viewer.
    #[test]
    fn every_report_names_the_account_it_was_read_as() {
        let mut report = report_with(None, vec![]);
        report.account_pk = Pk::new(7);
        report.username = Some("friend".into());
        report.is_self = false;
        let mut tick = TickReport::for_test(report, 0, Epoch::new(1_700_000_000));
        tick.viewer = Viewer {
            pk: Pk::new(42),
            username: Some("me".into()),
        };

        let reader = serde_json::json!({ "pk": 42, "username": "me" });
        assert_eq!(payload(&tick, "run-1", "watch.changes")["viewer"], reader);
        assert_eq!(tick_json(&tick)["viewer"], reader);
        assert_eq!(tick_json(&tick)["account"]["pk"], 7);

        let friend = Watched::consented("friend".into(), snob_engine::engine::watch::Consent);
        let error = anyhow::anyhow!("skipped this run");
        let at = Epoch::new(1_700_000_000);
        assert_eq!(
            failed_tick_json(Some(&tick.viewer), &friend, &error, at, 0)["viewer"],
            reader
        );
        assert!(
            failed_tick_json(None, &friend, &error, at, 0)["viewer"].is_null(),
            "nobody was signed in to read it as"
        );

        let checked = snob_engine::engine::check::CheckReport::default();
        assert_eq!(check_json(&checked, Some(&tick.viewer))["viewer"], reader);
        assert!(check_json(&checked, None)["viewer"].is_null());
    }
}
