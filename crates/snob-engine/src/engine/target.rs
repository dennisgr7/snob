//! Who the run is asking about.

use anyhow::Result;
use snob_core::Pk;
use snob_core::model::ListKind;
use snob_store::store::{Store, accounts, users};

use crate::app::{App, Viewer};
use crate::engine::ListQuery;

#[derive(Debug, Clone)]
pub struct Target {
    pub pk: Pk,
    /// As Instagram spells it, or as the store recorded it — never as it was
    /// typed.
    pub username: Option<String>,
    pub is_self: bool,
    /// The counters, when resolving already asked for them.
    ///
    /// Resolving a name and polling its counters are the **same request** to
    /// the same endpoint, so carrying the answer forward saves asking
    /// Instagram the identical question twice in a row — four times for a
    /// crossing.
    pub counters: Option<Counters>,
}

pub use snob_ig::model::Counters;

/// Strips the at sign people type out of habit.
pub fn clean(typed: &str) -> &str {
    typed.trim_start_matches('@')
}

/// How to name the account a run is about, before anything has resolved it.
pub fn label(app: &App, typed: Option<&str>) -> String {
    label_for(app.viewer(), typed)
}

/// [`label`] for a caller with no `App`, such as `--dry-run`.
pub fn label_for(viewer: &Viewer, typed: Option<&str>) -> String {
    match typed {
        Some(raw) => format!("@{}", snob_core::model::printable(clean(raw))),
        None => viewer.label(),
    }
}

/// The pk last seen under the name `typed`: the session's own when it is the
/// viewer's stored name, and otherwise from the local database.
pub fn known_pk(app: &App, typed: &str) -> Result<Option<Pk>> {
    known_pk_in(app.viewer(), app.db(), typed)
}

/// [`known_pk`] for a caller with no `App`: the viewer and the database it
/// reads, which a report opens without writing (`--dry-run`).
pub fn known_pk_in(viewer: &Viewer, db: &Store, typed: &str) -> Result<Option<Pk>> {
    let name = clean(typed);
    if viewer
        .username
        .as_deref()
        .is_some_and(|own| own.eq_ignore_ascii_case(name))
    {
        return Ok(Some(viewer.pk));
    }
    Ok(users::pk_named(db.conn(), name)?)
}

/// [`known_pk`] for a read of `target`, or with none of the viewer's own
/// account, whose pk is the session's whatever name `typed` holds.
pub fn known_pk_of(app: &App, target: Option<&str>, typed: &str) -> Result<Option<Pk>> {
    match target {
        None => Ok(Some(app.viewer().pk)),
        Some(_) => known_pk(app, typed),
    }
}

/// Whether `viewer` is shown what the account `profile` is keeps for its
/// followers once it is private: its grid and tabs, its highlights and its
/// stories. Its own, a public account's, and a private one's it follows; a
/// private account that does not say whether it is followed counts as not.
///
/// **A list is held to less** ([`resolve`]): only an account known to be
/// private and known not to be followed is refused there.
pub fn shown_to(profile: &snob_ig::model::WebProfileInfo, viewer: Pk) -> bool {
    profile.id == viewer
        || !profile.is_private.unwrap_or(false)
        || profile.followed_by_viewer.unwrap_or(false)
}

/// Resolves against Instagram when a name was given: from the browser, the
/// profile as the app opens one, its query and the three reads beside it
/// (`IgClient::profile_named`), four once the name's pk is known here and
/// five the first time; one without a browser.
///
/// A private account the viewer does not follow is refused **here**, before a
/// single page is walked: Instagram serves those lists to followers only, so
/// walking would buy nothing but empty pages.
pub async fn resolve(app: &mut App, args: &ListQuery) -> Result<Target> {
    let Some(typed) = args.target.as_deref() else {
        let viewer = app.viewer().clone();
        let resolved = match viewer.username {
            Some(u) => Some(u),
            None => app.client().resolve_username(viewer.pk).await?,
        };

        return Ok(Target {
            pk: viewer.pk,
            // `None` when it was never learned, rather than the numeric id
            // standing in for it.
            username: resolved.clone(),
            is_self: true,
            counters: match resolved {
                // Not asked for yet: your own account does not come through
                // the profile endpoint, and the poll reads its counters.
                Some(_) => None,
                // Without a real name, which only a run without a browser can
                // lack, there is nothing to poll **with** — the profile
                // endpoint takes a username — so saying "the counters are
                // unknown" costs nothing, while asking about a numeric id
                // would spend a request on a guaranteed 404 every single run.
                None => Some(Counters {
                    followers: None,
                    following: None,
                }),
            },
        });
    };

    let known = known_pk(app, typed)?;
    let profile = app.profile_named(clean(typed), known).await?;
    let is_self = app.viewer().pk == profile.id;

    // Only certainty blocks.
    if !is_self && profile.is_private == Some(true) && profile.followed_by_viewer == Some(false) {
        // Which of the two refusals it is, and nothing about how either reads.
        return Err(crate::sentences::refuse_private(
            &profile.username,
            profile.requested_by_viewer == Some(true),
        ));
    }

    // The profile endpoint answers 400 for certain business accounts, and the
    // client falls back to search to get an id at all.
    if !profile.counters_are_knowable() {
        app.progress()
            .warn(&crate::sentences::counters_unknowable(&profile.username));
    }

    Ok(Target {
        is_self,
        pk: profile.id,
        // The same answer that named the account also counted it.
        counters: Some(profile.counters()),
        username: Some(profile.username),
    })
}

/// Resolves from the local store alone, without touching the network.
pub fn from_store(app: &App, typed: Option<&str>, kind: ListKind) -> Result<Target> {
    let Some(typed) = typed else {
        let viewer = app.viewer();
        return Ok(Target {
            pk: viewer.pk,
            username: viewer.username.clone(),
            is_self: true,
            counters: None,
        });
    };

    let typed = clean(typed);
    let Some(pk) = accounts::find_pk_by_username(app.db().conn(), typed)? else {
        return Err(crate::sentences::refuse_nothing_stored(kind, Some(typed)));
    };

    Ok(Target {
        pk,
        // Always a name: the account was found by the one that was typed, so the
        // worst case is the typed spelling rather than nothing.
        username: Some(users::name(app.db().conn(), pk)?.unwrap_or_else(|| typed.to_string())),
        is_self: app.viewer().pk == pk,
        // Nothing was asked of Instagram, so there is nothing to carry.
        counters: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_at_sign_is_optional() {
        assert_eq!(clean("@someone"), "someone");
        assert_eq!(clean("someone"), "someone");
        // Only the leading one: it is not part of any username anyway.
        assert_eq!(clean("@@someone"), "someone");
    }

    /// One's own account, a public one and a private one followed are shown;
    /// a private one not followed, or that does not say, is not.
    #[test]
    fn a_private_account_shows_its_posts_to_its_followers_alone() {
        let viewer = Pk::new(1);
        let profile = |pk: u64, private: Option<bool>, followed: Option<bool>| {
            let mut profile: snob_ig::model::WebProfileInfo = serde_json::from_value(
                serde_json::json!({ "id": pk.to_string(), "username": "x" }),
            )
            .unwrap();
            profile.is_private = private;
            profile.followed_by_viewer = followed;
            profile
        };
        for (pk, private, followed, shown) in [
            (1, Some(true), Some(false), true),
            (2, Some(false), Some(false), true),
            (2, None, None, true),
            (2, Some(true), Some(true), true),
            (2, Some(true), Some(false), false),
            (2, Some(true), None, false),
        ] {
            assert_eq!(
                shown_to(&profile(pk, private, followed), viewer),
                shown,
                "account {pk}, private {private:?}, followed {followed:?}"
            );
        }
    }
}
