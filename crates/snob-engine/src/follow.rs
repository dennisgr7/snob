//! The two writes: follow and unfollow. The request is sent by `snob_ig`'s
//! client; this module decides whether to send it.
//!
//! A front that has not already been told yes is asked through the front it
//! handed the engine (`App::confirm`). `-y`, and the yes `snob mcp` collected
//! before it ran the command, pass `confirmed` and are not asked again.

use anyhow::{Result, anyhow};
use snob_core::model::printable;
use snob_ig::graphql::DocIds;
use snob_ig::model::{FriendshipStatus, WebProfileInfo};

use crate::app::App;
use crate::exit::{ExitCode, ExitError};

/// Which of the two writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Follow,
    Unfollow,
}

impl Verb {
    /// The word a program matches on, and the one in a sentence.
    pub fn present(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Unfollow => "unfollow",
        }
    }
}

/// What a write refused in a cooldown says it did not do.
pub const NOTHING_SENT: &str = "nothing can be sent";

/// The account, what became of the write, and Instagram's answer when one
/// was sent.
pub struct Done {
    pub profile: WebProfileInfo,
    pub outcome: Outcome,
    /// Instagram's answer to the write. Absent when nothing was sent.
    pub status: Option<FriendshipStatus>,
}

/// Follows or unfollows `typed`, or reports that it would change nothing.
///
/// `confirmed` is a yes already given (`-y`, or the one `snob mcp` asked
/// for). Without it, and with nobody there to ask, nothing is sent.
pub async fn run(app: &mut App, verb: Verb, typed: &str, confirmed: bool) -> Result<Done> {
    refuse_if_it_cannot_send(app, verb)?;
    // Being unable to ask and being told no are two different events.
    if !confirmed && !app.can_ask() {
        return Err(crate::sentences::refuse_unattended(
            format!("nothing was {}ed: it needs confirmation", verb.present()),
            "Pass -y to confirm in advance.".to_string(),
        ));
    }

    let known = crate::engine::target::known_pk(app, typed)?;
    let profile = app
        .client()
        .profile_named(crate::engine::target::clean(typed), known)
        .await?;

    // Refusing to follow yourself here rather than letting Instagram do it:
    // its answer is a generic 400 that `classify` cannot tell apart from a
    // real failure, and the user would be told the account is throttled.
    if Some(profile.id) == Some(app.client().session().ds_user_id) {
        return Err(anyhow!("you cannot {} your own account", verb.present()));
    }

    if let Some(settled) = already_done(verb, &profile) {
        return Ok(Done {
            profile,
            outcome: settled,
            status: None,
        });
    }

    let name = printable(&profile.username);
    let question = match verb {
        Verb::Follow if profile.is_private == Some(true) => {
            format!("Send @{name} a follow request?")
        }
        Verb::Follow => format!("Follow @{name}?"),
        Verb::Unfollow => format!("Unfollow @{name}?"),
    };
    if !confirmed && !app.confirm(question).await? {
        return Ok(Done {
            profile,
            outcome: Outcome::Declined,
            status: None,
        });
    }

    let ids = RememberedIds { db: app.db() };
    let client = app.client();
    let status = match verb {
        Verb::Follow => client.follow(profile.id, &profile.username, &ids).await?,
        Verb::Unfollow => client.unfollow(profile.id, &profile.username, &ids).await?,
    };
    let outcome = outcome(verb, &status);
    remember_own_write(app, &profile, outcome);
    Ok(Done {
        profile,
        outcome,
        status: Some(status),
    })
}

/// What would refuse this write before a profile is read: no CSRF token,
/// or a cooldown. Nothing is sent.
pub fn refuse_if_it_cannot_send(app: &App, verb: Verb) -> Result<()> {
    if !app.client().can_write() {
        return Err(cannot_write(verb));
    }
    if let Some(held) = app.held()? {
        return Err(ExitError::new(
            ExitCode::RateLimited,
            format!("{}, so {NOTHING_SENT}", crate::sentences::held_until(&held)),
        )
        .with_cooldown_until(held.until_ms)
        .into());
    }
    Ok(())
}

/// The refusal of a session that has no CSRF token to write with.
pub fn cannot_write(verb: Verb) -> anyhow::Error {
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

/// How recent a walk of the following list has to be for a follow to be
/// answered as already so from it, with nothing asked.
const FOLLOWED_AS_STORED_FOR: i64 = 3_600;

/// The account `typed` names, when the account's own following list, walked
/// within the last hour and with its own writes since applied
/// (`own_writes`), holds it; with when that list was read. **Only for a
/// follow**: a stored list cannot tell an unfollow that would change nothing
/// from one that would withdraw a pending request, which no list holds.
pub fn followed_as_stored(
    store: &snob_store::store::Store,
    viewer: snob_core::Pk,
    typed: &str,
    now: snob_core::Epoch,
) -> Result<Option<(snob_core::model::User, snob_core::Epoch)>> {
    use snob_store::store::{own_writes, users};
    let conn = store.conn();
    let Some(pk) = users::pk_named(conn, crate::engine::target::clean(typed))? else {
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
pub enum Outcome {
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
    pub fn token(self) -> &'static str {
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
    pub fn changed(self) -> bool {
        matches!(self, Self::Followed | Self::Requested | Self::Unfollowed)
    }

    /// The sentence a person is told.
    pub fn sentence(self, verb: Verb, name: &str) -> Option<String> {
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

/// What the profile already says, when it says the request would change
/// nothing.
fn already_done(verb: Verb, profile: &WebProfileInfo) -> Option<Outcome> {
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

/// Writes down a write that changed the account's following, so a stored
/// following list is handed out with it applied until the next walk reads it
/// (`own_writes`, `engine::serve_stored`).
///
/// **Not a reason to fail the run**: the write has happened, and saying it
/// failed would invite sending it again.
fn remember_own_write(app: &App, profile: &WebProfileInfo, outcome: Outcome) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Pk;

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
