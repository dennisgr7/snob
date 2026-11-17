//! One run of the monitor: the lists, then the posts, then what changed
//! since the last report.

use anyhow::Result;
use snob_core::Pk;
use snob_core::model::{ListKind, StopReason};

use super::{
    App, ExitCode, ListOutcome, PostsSkipped, Provenance, Skipped, TickList, TickPosts, TickReport,
    Watched, accounts, compare, engine, store, target,
};

/// The most specific reason every list on a run was refused.
pub(super) fn refused_outcome(lists: &[TickList], posts: Option<PostsSkipped>) -> ExitCode {
    let of_posts = match posts {
        Some(PostsSkipped::Cooldown) => Some(ExitCode::RateLimited),
        Some(PostsSkipped::Incomplete(reason, said)) => {
            Some(said.unwrap_or_else(|| crate::exit::from_stop_reason(reason)))
        }
        _ => None,
    };
    lists
        .iter()
        .find_map(|list| match list.skipped {
            Some(Skipped::NobodyLooked(Provenance::Cooldown)) => Some(ExitCode::RateLimited),
            // What Instagram said beats what the store had to record
            // (`ListOutcome::stopped_by`).
            Some(Skipped::Incomplete(reason, said)) => {
                Some(said.unwrap_or_else(|| crate::exit::from_stop_reason(reason)))
            }
            _ => None,
        })
        .or(of_posts)
        .unwrap_or(ExitCode::Error)
}

/// Goes and looks, then reports what changed since the last time it did.
///
/// Nothing is concluded from a list this run did not verify, or one that came
/// back short. **The mark does not move for a refused list**: what was never
/// reported stays unreported, and the next run says it.
pub async fn tick(app: &mut App, watched: &Watched) -> Result<TickReport> {
    let args = watched.list_args();
    let before = app.client().pacer().spent();
    // A tick is an action: counters a previous tick read, on this App, are
    // not this tick's.
    app.client().pacer().begin_action();

    let mut lists = Vec::new();
    let mut usable = Vec::new();
    // Whose account this turned out to be, taken from the engine's answer
    // rather than from the viewer: on a third party they are different, and the
    // id the engine reports is the one that cannot be wrong about it.
    let mut pk: Option<Pk> = None;

    let kinds: &[ListKind] = if watched.lists() {
        &[ListKind::Followers, ListKind::Following]
    } else {
        &[]
    };
    for &kind in kinds {
        // A canceled walk comes back `Ok`, so without this the loop would go on
        // to the second list, and `App::resolved_target` drops the remembered
        // counters once the first walk has begun, so it would ask Instagram
        // again after the user was told "Stopping and saving what has been
        // fetched…".
        //
        // **The monitor's battery stop too**, asked again here rather than only
        // between pages.
        let power_stop =
            !app.cancel().is_canceled() && app.stop_between_pages().is_some_and(|stop| stop());
        if app.cancel().is_canceled() || power_stop {
            // Said once: a walk the battery stopped has already said so.
            if power_stop && !a_list_was_canceled(&lists) {
                app.warn(crate::sentences::BATTERY_CRITICAL_SKIP);
            }
            lists.push(TickList {
                kind,
                skipped: Some(Skipped::Incomplete(StopReason::Canceled, None)),
            });
            continue;
        }

        // A cooldown stops *this list*; everything else stops the run.
        let outcome = match engine::list(app, &args, kind).await {
            Ok((_, outcome)) => outcome,
            Err(e) if crate::exit::from_chain(&e) == Some(ExitCode::RateLimited) => {
                lists.push(TickList {
                    kind,
                    skipped: Some(Skipped::Incomplete(
                        StopReason::RateLimit,
                        Some(ExitCode::RateLimited),
                    )),
                });
                continue;
            }
            Err(e) => return Err(e),
        };
        pk = Some(outcome.account_pk);

        let skipped = refusal(&outcome);
        if skipped.is_none() {
            usable.push((kind, outcome.snapshot_id));
        }
        lists.push(TickList { kind, skipped });
    }

    // The posts, after the lists, in the same action: the profile the lists
    // opened is the one the grid follows (`posts::look` says the order).
    let after_lists = app.client().pacer().spent() > before;
    let battery_said = a_list_was_canceled(&lists);
    let looked_at_posts = super::posts::look(app, watched, after_lists, battery_said).await?;
    let posts_skipped = looked_at_posts.as_ref().and_then(|look| look.skipped);

    let pk = match (
        pk.or(looked_at_posts.as_ref().and_then(|look| look.pk)),
        watched.name(),
    ) {
        (Some(pk), _) => pk,
        (None, None) => app.viewer().pk,
        (None, Some(name)) => {
            let name = target::clean(name);
            // The refusal carries the code `outcome()` would have produced,
            // not a bare error, so a first tick during a cooldown exits 5 like
            // the same tick on a walked account, and the `--json` stream does
            // not say `"error"` about a state that lifts on its own.
            accounts::find_pk_by_username(app.db().conn(), name)?.ok_or_else(|| {
                crate::sentences::refuse_nothing_looked_at(
                    name,
                    refused_outcome(&lists, posts_skipped),
                )
            })?
        }
    };

    // Read before the comparison, and both handed back, so that whatever
    // commits this report writes the same two numbers the renames were read
    // against.
    let at = snob_core::clock::now();
    let head = store::history_head(app.db().conn())?;

    // Nothing is written here.
    let mut compared = compare(app, pk, &usable, head)?;
    let (tick_posts, read_posts, posts_commit) = match looked_at_posts {
        Some(look) => {
            compared.report.posts = look.report;
            (
                Some(TickPosts {
                    skipped: look.skipped,
                }),
                look.read,
                look.commit,
            )
        }
        None => (None, Vec::new(), None),
    };

    Ok(TickReport {
        viewer: app.viewer().clone(),
        report: compared.report,
        requests: app.client().pacer().spent().saturating_sub(before),
        lists,
        posts: tick_posts,
        timing: None,
        read_posts,
        posts_commit,
        committable: compared.marks,
        at,
        rename_cursor: compared.rename_cursor,
        renames_sent: compared.renames_sent,
    })
}

/// Whether a list of this run was stopped before it ended: by Ctrl+C, or by
/// the battery, which the walk it stopped has said already.
fn a_list_was_canceled(lists: &[TickList]) -> bool {
    lists.iter().any(|list| {
        matches!(
            list.skipped,
            Some(Skipped::Incomplete(StopReason::Canceled, _))
        )
    })
}

/// Whether a list may be the basis of a comparison, and why not when it may not.
pub(super) fn refusal(outcome: &ListOutcome) -> Option<Skipped> {
    if !outcome.provenance.describes_now() {
        return Some(Skipped::NobodyLooked(outcome.provenance));
    }
    if !outcome.is_complete() {
        // `stopped_by` rather than the reason alone.
        return Some(Skipped::Incomplete(outcome.reason, outcome.stopped_by));
    }
    None
}
