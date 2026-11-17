//! `snob follow` and `snob unfollow`: the only two commands that change anything
//! on Instagram.
//!
//! **One account per run, and there is no flag that changes it.** The reasoning
//! is in `AGENTS.md` with the rest of the write regime, and the short version is
//! that a burst is what strains a service, not a daily total — so the shape of
//! this command is the safeguard, not a limit inside it.
//!
//! The write is [`snob_engine::follow`]. This command asks only by handing the
//! engine a front, records an agent's yes, and says what happened.

use anyhow::Result;
use snob_core::model::printable;
use snob_engine::exit::ExitCode;
use snob_engine::follow::{self, Outcome};
use snob_ig::model::FriendshipStatus;
use snob_store::paths::AccountPaths;
use snob_store::secrets::SessionStore;

use crate::cli::FollowArgs;
use crate::commands::common;
use crate::ui;

pub use snob_engine::follow::Verb;

pub async fn run(
    args: FollowArgs,
    verb: Verb,
    secrets: SessionStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    // No progress bar: two requests, and the write may sit on the write
    // budget for a quarter of an hour (from the browser, before the profile
    // is loaded), which is a wait to be told about in a sentence rather than
    // animated.
    if args.check_ahead {
        return check_ahead(&args, verb, &secrets, paths);
    }
    let mut app = common::app(&secrets, paths, false)?;

    // Before an agent's act is recorded: a session that cannot write, or a
    // cooldown, sends nothing and is not an act.
    follow::refuse_if_it_cannot_send(&app, verb)?;
    // Whether anybody can answer is known now, and a run that will refuse
    // for want of an answer should not pay for a lookup first. The advice
    // goes in the message: a hint on exit 130 is never shown.
    if !args.consent.yes && !app.can_ask() {
        return Err(snob_engine::sentences::refuse_unattended(
            format!("nothing was {}ed: it needs confirmation", verb.present()),
            "Pass -y to confirm in advance.".to_string(),
        ));
    }

    // **Before the first request**, for a run an agent was told yes to through
    // `snob mcp`: recorded where the person sees it, and refused with nothing
    // sent past the day's cap on an agent's writes.
    let act = common::record_agent_act(
        &mut app,
        &args.agent,
        verb.present(),
        Some(&args.target),
        args.mcp_write_cap,
    )?;

    let done = follow::run(&mut app, verb, &args.target, args.consent.yes).await?;
    if let Some(id) = act
        && let Err(e) = snob_store::store::mcp_acts::resolved(app.db().conn(), id, done.profile.id)
    {
        // The act stays recorded by the name it was asked about.
        tracing::debug!(error = %e, "the account an act was about could not be stored");
    }

    let name = printable(&done.profile.username);
    if done.outcome == Outcome::Declined {
        tell(&args, verb, &done.profile, &name, done.outcome, None)?;
        return Ok(ExitCode::Interrupted);
    }
    tell(
        &args,
        verb,
        &done.profile,
        &name,
        done.outcome,
        done.status.as_ref(),
    )
}

/// `--check-ahead`: what would refuse this write now if an agent made it,
/// **sending nothing, asking nothing and writing nothing**, or `{"would":
/// "send"}` on standard output when nothing would. `snob mcp` runs it before it
/// asks the person for a yes to a follow or an unfollow, so nobody says yes to
/// one that is then refused, and no ticket is spent on one (AGENTS.md, "The MCP
/// server").
///
/// **Not known ahead**: what the profile says (already so, the viewer's own
/// account, a name that resolves to nothing), and whatever changes between this
/// and the yes; except a follow of an account the following list walked in the
/// last hour holds, which is answered as already so, `{"would": "nothing",
/// ...}`, so no yes is asked for a write that would change nothing
/// (`followed_as_stored`).
fn check_ahead(
    args: &FollowArgs,
    verb: Verb,
    secrets: &SessionStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    let now = snob_core::clock::now_ms();
    let session = common::alone_ahead(secrets, paths, now)?;
    if session.csrftoken.is_none() && !snob_ig::client::page::a_client_would_use_a_page() {
        return Err(follow::cannot_write(verb));
    }
    let (store, budget) = snob_engine::app::standing(paths, now)?;
    if let Some(held) = snob_engine::app::held_in(&budget, paths, now) {
        return Err(common::in_cooldown(&held, follow::NOTHING_SENT));
    }
    let cap = args
        .mcp_write_cap
        .unwrap_or(snob_store::store::mcp_acts::WRITES_A_DAY);
    if let Some(snob_store::store::mcp_acts::Recorded::Capped { writes, free_at }) =
        snob_store::store::mcp_acts::over_the_cap(store.conn(), cap, now)?
    {
        return Err(common::write_cap_reached(writes, free_at));
    }
    // The write pays the pace and the day as the profile before it does, and
    // the write bucket on top, so its wait is the longest of the run's.
    let owed = budget.next_write_at.get().saturating_sub(now.get());
    let wait = std::time::Duration::from_millis(u64::try_from(owed).unwrap_or(0));
    if let Some(limit) = snob_engine::app::max_wait()
        && wait > limit
    {
        return Err(snob_ig::error::IgError::WaitTooLong { wait, limit }.into());
    }
    // A follow of an account a just-walked following list holds would change
    // nothing, and is answered as such rather than put to the person.
    if verb == Verb::Follow
        && let Some((user, taken_at)) =
            follow::followed_as_stored(&store, paths.pk(), &args.target, now.to_epoch())?
    {
        crate::ui::say!(
            "{}",
            serde_json::json!({
                "would": "nothing",
                "action": verb.present(),
                "target": {
                    "pk": user.pk,
                    "username": printable(&user.username),
                    "is_private": user.is_private,
                },
                "outcome": Outcome::AlreadyFollowing.token(),
                "changed": false,
                "following": true,
                "requested": null,
                "viewer": snob_engine::sentences::acting_as().map(|viewer| viewer.json()),
                "as_stored": taken_at,
                "message": format!(
                    "nothing was asked or sent: the following list read at {} (as_stored) \
                     already has this account. If that has changed since, a walk of the \
                     following list reads it again.",
                    snob_engine::sentences::stored_on(taken_at)
                ),
            })
        );
        return Ok(ExitCode::Ok);
    }
    crate::ui::say!("{}", serde_json::json!({ "would": "send" }));
    Ok(ExitCode::Ok)
}

/// Says how the run ended, as a sentence on standard error or, with `--json`,
/// as one object on standard output.
fn tell(
    args: &FollowArgs,
    verb: Verb,
    profile: &snob_ig::model::WebProfileInfo,
    name: &str,
    outcome: Outcome,
    status: Option<&FriendshipStatus>,
) -> Result<ExitCode> {
    if args.output.json {
        // What the relationship is now: Instagram's answer to the write when
        // there was one, and the profile's own fields when there was not
        // (`null` where the profile left them out, as `already_done` reads
        // them).
        let (following, requested) = match status {
            Some(status) => (Some(status.following), Some(status.outgoing_request)),
            None => (profile.followed_by_viewer, profile.requested_by_viewer),
        };
        ui::say!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "action": verb.present(),
                "target": {
                    "pk": profile.id,
                    "username": name,
                    "is_private": profile.is_private,
                },
                "outcome": outcome.token(),
                "changed": outcome.changed(),
                "following": following,
                "requested": requested,
                "viewer": snob_engine::sentences::acting_as().map(|viewer| viewer.json()),
            }))?
        );
    } else if let Some(sentence) = outcome.sentence(verb, name) {
        ui::info(&sentence);
    }
    Ok(ExitCode::Ok)
}
