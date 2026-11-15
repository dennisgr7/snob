//! `snob follow` and `snob unfollow`: the only two commands that change anything
//! on Instagram.
//!
//! **One account per run, and there is no flag that changes it.** The reasoning
//! is in `AGENTS.md` with the rest of the write regime, and the short version is
//! that a burst is what strains a service, not a daily total — so the shape of
//! this command is the safeguard, not a limit inside it.

use anyhow::{Result, anyhow};
use snob_core::model::printable;
use snob_engine::engine::target;
use snob_engine::exit::{ExitCode, ExitError};
use snob_ig::graphql::DocIds;
use snob_ig::model::FriendshipStatus;
use snob_store::paths::AccountPaths;
use snob_store::secrets::SessionStore;

use crate::cli::FollowArgs;
use crate::commands::common;
use crate::ui;

/// Which of the two verbs is being run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Follow,
    Unfollow,
}

impl Verb {
    fn present(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Unfollow => "unfollow",
        }
    }
}

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

    // **Before anything is spent.** A session cannot write only when requests
    // are sent directly (SNOB_NO_BROWSER) and it was pasted without
    // `--csrftoken`, and finding out from Instagram's 403 would cost a
    // request, a slot of write budget, and a message telling the user their
    // session had expired when it had not.
    if !app.client().can_write() {
        return Err(cannot_write(verb));
    }

    common::refuse_during_cooldown(&app, NOTHING_SENT)?;

    // **Also before anything is spent.** Whether anybody can answer is a
    // property of this process's streams, not of the account, so it is known
    // now — and a run that is going to refuse for want of an answer should not
    // pay for a lookup first.
    if !args.consent.yes && !ui::can_be_asked() {
        // The advice goes in the message, where it arrives: a hint on exit 130
        // is never shown, because `report::rendered` returns early for that
        // code.
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

    let known = target::known_pk(&app, &args.target)?;
    let profile = app
        .client()
        .profile_named(target::clean(&args.target), known)
        .await?;
    if let Some(id) = act
        && let Err(e) = snob_store::store::mcp_acts::resolved(app.db().conn(), id, profile.id)
    {
        // The act stays recorded by the name it was asked about.
        tracing::debug!(error = %e, "the account an act was about could not be stored");
    }

    // Instagram spells the name; the user typed a spelling of it.
    let name = printable(&profile.username);

    // Refusing to follow yourself here rather than letting Instagram do it:
    // its answer is a generic 400 that `classify` cannot tell apart from a
    // real failure, and the user would be told the account is throttled.
    if Some(profile.id) == Some(app.client().session().ds_user_id) {
        return Err(anyhow!("you cannot {} your own account", verb.present()));
    }

    // Nothing to do is worth saying rather than doing.
    if let Some(settled) = already_done(verb, &profile) {
        return tell(&args, verb, &profile, &name, settled, None);
    }

    let question = match verb {
        Verb::Follow if profile.is_private == Some(true) => {
            format!("Send @{name} a follow request?")
        }
        Verb::Follow => format!("Follow @{name}?"),
        Verb::Unfollow => format!("Unfollow @{name}?"),
    };

    // `confirm` answers with its default the moment nobody can answer, which
    // would turn "nobody was there" into "they said no" — two different events.
    if !args.consent.yes && !ui::confirm(&question, false)? {
        tell(&args, verb, &profile, &name, Outcome::Declined, None)?;
        return Ok(ExitCode::Interrupted);
    }

    let ids = RememberedIds { db: app.db() };
    let (client, pk, username) = (app.client(), profile.id, &profile.username);
    let status = match verb {
        Verb::Follow => client.follow(pk, username, &ids).await?,
        Verb::Unfollow => client.unfollow(pk, username, &ids).await?,
    };
    let outcome = outcome(verb, &status);
    remember_own_write(&app, &profile, outcome);
    tell(&args, verb, &profile, &name, outcome, Some(&status))
}

/// Writes down a write that changed the account's following, so a stored
/// following list is handed out with it applied until the next walk reads it
/// (`own_writes`, `engine::serve_stored`).
///
/// **Not a reason to fail the run**: the write has happened, and saying it
/// failed would invite sending it again.
fn remember_own_write(
    app: &snob_engine::app::App,
    profile: &snob_ig::model::WebProfileInfo,
    outcome: Outcome,
) {
    use snob_store::store::own_writes::{self, Verb as Written};
    let verb = match outcome {
        Outcome::Followed => Written::Follow,
        Outcome::Unfollowed => Written::Unfollow,
        _ => return,
    };
    let conn = app.db().conn();
    let user = snob_core::model::User::named(profile.id, profile.username.clone());
    let recorded = snob_store::store::users::upsert(conn, &user)
        .and_then(|_| own_writes::record(conn, profile.id, verb, snob_core::clock::now()));
    if let Err(e) = recorded {
        tracing::debug!(error = %e, "the write could not be written down for the stored lists");
    }
}

/// What a write refused in a cooldown says it did not do.
const NOTHING_SENT: &str = "nothing can be sent";

/// The refusal of a session that has no CSRF token to write with.
fn cannot_write(verb: Verb) -> anyhow::Error {
    ExitError::new(
        ExitCode::NoSession,
        format!(
            "this session can read but not {}: it has no CSRF token",
            verb.present()
        ),
    )
    .with_hint(
        "run \"snob login --browser\", which captures it, or \
         \"snob login --paste --csrftoken <token>\"",
    )
    .into()
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
        return Err(cannot_write(verb));
    }
    let (store, budget) = snob_engine::app::standing(paths, now)?;
    if let Some(held) = snob_engine::app::held_in(&budget, paths, now) {
        return Err(common::in_cooldown(&held, NOTHING_SENT));
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
            followed_as_stored(&store, paths.pk(), &args.target, now.to_epoch())?
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

/// How recent a walk of the following list has to be for `--check-ahead` to
/// answer a follow as already so from it, with nothing asked.
const FOLLOWED_AS_STORED_FOR: i64 = 3_600;

/// The account `typed` names, when the account's own following list, walked
/// within [`FOLLOWED_AS_STORED_FOR`] and with its own writes since applied
/// (`own_writes`), holds it; with when that list was read. **Only for a
/// follow**: a stored list cannot tell an unfollow that would change nothing
/// from one that would withdraw a pending request, which no list holds.
fn followed_as_stored(
    store: &snob_store::store::Store,
    viewer: snob_core::Pk,
    typed: &str,
    now: snob_core::Epoch,
) -> Result<Option<(snob_core::model::User, snob_core::Epoch)>> {
    use snob_store::store::{own_writes, users};
    let conn = store.conn();
    let Some(pk) = users::pk_named(conn, target::clean(typed))? else {
        return Ok(None);
    };
    let not_before = snob_core::Epoch::new(now.get() - FOLLOWED_AS_STORED_FOR);
    Ok(match own_writes::followed(conn, viewer, pk, not_before)? {
        own_writes::Followed::Yes { user, taken_at } => Some((user, taken_at)),
        own_writes::Followed::No { .. } | own_writes::Followed::NoCapture => None,
    })
}

/// How a run ended, as a word a program can match on and a sentence a person
/// can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    AlreadyFollowing,
    AlreadyRequested,
    NotFollowing,
    /// The question was asked and answered no; nothing was sent.
    Declined,
    Followed,
    /// A private account: following it asks, and they have to accept.
    Requested,
    Unfollowed,
    /// Instagram took the write and reported the relationship as it was.
    Unchanged,
}

impl Outcome {
    /// The `outcome` field of `--json`. Part of the JSON contract.
    fn token(self) -> &'static str {
        match self {
            Self::AlreadyFollowing => "already_following",
            Self::AlreadyRequested => "already_requested",
            Self::NotFollowing => "not_following",
            Self::Declined => "declined",
            Self::Followed => "followed",
            Self::Requested => "requested",
            Self::Unfollowed => "unfollowed",
            Self::Unchanged => "unchanged",
        }
    }

    /// Whether this run changed anything on Instagram.
    fn changed(self) -> bool {
        matches!(self, Self::Followed | Self::Requested | Self::Unfollowed)
    }

    /// The sentence a person is told.
    fn sentence(self, verb: Verb, name: &str) -> Option<String> {
        Some(match self {
            Self::AlreadyFollowing => format!("You already follow @{name}."),
            Self::AlreadyRequested => {
                format!("You have already asked to follow @{name}, and they have not answered yet.")
            }
            Self::NotFollowing => format!("You do not follow @{name}."),
            Self::Declined => return None,
            Self::Followed => format!("Now following @{name}."),
            Self::Requested => format!("Asked to follow @{name}. They have to accept it."),
            Self::Unfollowed => format!("No longer following @{name}."),
            // Instagram answered, and answered that nothing is different.
            Self::Unchanged => match verb {
                Verb::Follow => {
                    format!("Instagram accepted the request, but @{name} is still not followed.")
                }
                Verb::Unfollow => {
                    format!("Instagram accepted the request, but @{name} is still followed.")
                }
            },
        })
    }
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

/// What the profile already says, when it says the request would change
/// nothing.
fn already_done(verb: Verb, profile: &snob_ig::model::WebProfileInfo) -> Option<Outcome> {
    match verb {
        Verb::Follow if profile.followed_by_viewer == Some(true) => Some(Outcome::AlreadyFollowing),
        Verb::Follow if profile.requested_by_viewer == Some(true) => {
            Some(Outcome::AlreadyRequested)
        }
        // Both facts have to be *known*: an absent `requested_by_viewer` is not
        // "no pending request", which is the assumption the doc-comment above
        // forbids -- an unfollow that would withdraw a pending request must
        // not be refused on a field Instagram merely left out.
        Verb::Unfollow
            if profile.followed_by_viewer == Some(false)
                && profile.requested_by_viewer == Some(false) =>
        {
            Some(Outcome::NotFollowing)
        }
        _ => None,
    }
}

/// The mutation ids, remembered in the `meta` table between runs.
///
/// **Discovery is what makes a rotated id fix itself, and caching is what makes
/// discovery affordable.**
struct RememberedIds<'a> {
    db: &'a snob_store::store::Store,
}

impl DocIds for RememberedIds<'_> {
    fn get(&self, friendly_name: &str) -> Option<String> {
        self.db.remembered(&key(friendly_name)).ok().flatten()
    }

    fn put(&self, friendly_name: &str, doc_id: &str) {
        if let Err(e) = self.db.remember(&key(friendly_name), doc_id) {
            tracing::debug!(error = %e, "the mutation id could not be remembered");
        }
    }
}

/// Namespaced, because `meta` holds facts about the database and these are
/// facts about Instagram.
fn key(friendly_name: &str) -> String {
    format!("graphql.doc_id.{friendly_name}")
}

/// What actually happened, as Instagram describes it afterwards.
///
/// **Following a private account does not follow it — it asks.**
fn outcome(verb: Verb, status: &FriendshipStatus) -> Outcome {
    match verb {
        Verb::Follow if status.following => Outcome::Followed,
        Verb::Follow if status.outgoing_request => Outcome::Requested,
        Verb::Unfollow if !status.following && !status.outgoing_request => Outcome::Unfollowed,
        _ => Outcome::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Pk;
    use snob_ig::model::WebProfileInfo;

    fn profile(followed: Option<bool>, requested: Option<bool>) -> WebProfileInfo {
        WebProfileInfo {
            id: Pk::new(7),
            username: "someone".into(),
            full_name: None,
            is_private: None,
            is_verified: None,
            followed_by_viewer: followed,
            requested_by_viewer: requested,
            profile_pic_url: None,
            profile_pic_url_hd: None,
            followers: None,
            following: None,
            follows_viewer: None,
            has_requested_viewer: None,
            biography: None,
            external_url: None,
            posts: None,
            mutual: None,
            highlight_reel_count: None,
            is_business_account: None,
            category_name: None,
            anonymous_picture: None,
            latest_reel_media: None,
            // The route that answered. Search is the fallback for the accounts
            // `web_profile_info` cannot serialize; these fixtures are the
            // ordinary case.
            via: snob_ig::model::Via::Profile,
        }
    }

    #[test]
    fn a_relationship_that_already_holds_costs_no_request() {
        assert!(already_done(Verb::Follow, &profile(Some(true), None)).is_some());
        assert!(already_done(Verb::Unfollow, &profile(Some(false), Some(false))).is_some());
        assert!(already_done(Verb::Follow, &profile(Some(false), Some(true))).is_some());
    }

    /// The endpoint omits these fields on some answers. Unknown has to mean
    /// "send it and find out", or the command stops working the day Instagram
    /// drops the field.
    #[test]
    fn an_unknown_relationship_is_not_treated_as_settled() {
        assert!(already_done(Verb::Follow, &profile(None, None)).is_none());
        assert!(already_done(Verb::Unfollow, &profile(None, None)).is_none());
        // Half-known is still unknown: not following, but with the pending
        // request unreported, unfollow may still have a request to withdraw.
        assert!(already_done(Verb::Unfollow, &profile(Some(false), None)).is_none());
    }

    /// A pending request is not a follow, and unfollow has something to do
    /// about it: withdrawing the request.
    #[test]
    fn a_pending_request_is_still_something_to_withdraw() {
        assert!(already_done(Verb::Unfollow, &profile(Some(false), Some(true))).is_none());
    }

    #[test]
    fn following_a_private_account_is_reported_as_the_request_it_is() {
        let asked = FriendshipStatus {
            following: false,
            outgoing_request: true,
            ..Default::default()
        };
        let told = outcome(Verb::Follow, &asked);
        assert_eq!(told, Outcome::Requested);
        let said = told.sentence(Verb::Follow, "someone").unwrap();
        assert!(said.contains("Asked to follow"), "{said}");
        assert!(!said.contains("Now following"), "{said}");
    }

    #[test]
    fn a_write_instagram_accepted_but_did_not_apply_is_not_called_a_success() {
        let unchanged = FriendshipStatus::default();
        let told = outcome(Verb::Follow, &unchanged);
        assert!(!told.changed());
        let said = told.sentence(Verb::Follow, "someone").unwrap();
        assert!(said.contains("still not followed"), "{said}");
    }
}
