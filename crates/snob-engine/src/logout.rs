//! Signing out: the session and the browser profile of an account, or of
//! every one, removed from this computer.
//!
//! **The order is AGENTS.md's "Signing out"**: the jobs stopped and their lock
//! held to the end, every other process signed out, the browsers released,
//! and only then the session and the profiles removed. What went and what
//! did not comes back as data ([`Left`]); whoever runs it says it.

use std::path::PathBuf;

use snob_core::Pk;
use snob_store::paths::{self, AppPaths};
use snob_store::registry::Registry;
use snob_store::secrets::{SecretStore, SecretsError};

use crate::browser::owner;
use crate::jobs::Stopping;

/// Which accounts a sign-out is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    pub leaving: Vec<Pk>,
    /// Every profile there is goes, not only those of the accounts leaving:
    /// no other account's is to be kept.
    pub sweep: bool,
}

impl Who {
    /// `account` alone, or with `all` or no account every one found: a
    /// registry that is gone must not leave the sessions behind while the
    /// profiles are swept.
    pub fn of(paths: &AppPaths, account: Option<Pk>, all: bool) -> Self {
        let everyone = Registry::known_accounts(paths);
        let all = all || account.is_none();
        let sweep = all || everyone.len() <= 1;
        let leaving = match account {
            Some(pk) if !all => vec![pk],
            _ => everyone,
        };
        Self { leaving, sweep }
    }

    /// The accounts whose jobs are stopped first ([`crate::jobs::stop_every_job`]).
    pub fn accounts(&self, paths: &AppPaths) -> Vec<paths::AccountPaths> {
        self.leaving.iter().map(|pk| paths.account(*pk)).collect()
    }
}

/// What a sign-out removed, and what refused to go.
#[derive(Debug)]
pub struct Left {
    /// Whether any of the accounts had a session stored.
    pub had_session: bool,
    /// Whether every stored session went.
    pub sessions: Result<(), SecretsError>,
    /// Whether any profile was removed.
    pub profiles_deleted: bool,
    /// Profiles not removed for being too close to the root.
    pub too_close: Vec<PathBuf>,
    /// The first profile that could not be removed, and why.
    pub profile_failure: Option<(PathBuf, std::io::Error)>,
}

/// Signs `who` out for every process, then removes their sessions and
/// profiles. **`_held` is their jobs, stopped, and their jobs locks**, which
/// the caller keeps until this returns: a walk still running would go on with
/// the session being deleted, and start a browser again on a profile about to
/// go.
pub async fn sign_out(
    secrets: &SecretStore,
    paths: &AppPaths,
    who: &Who,
    _held: &[Stopping],
) -> Left {
    // Every other process still holding one of these sessions: a walk typed
    // in another terminal, the monitor's run, an agent's call.
    owner::sign_out(paths, &who.leaving);
    // A browser still running on a profile about to go would write into it as
    // it went, or keep Windows from removing it.
    let only = match who.leaving.as_slice() {
        [one] if !who.sweep => Some(*one),
        _ => None,
    };
    owner::release(paths, only).await;
    let stores: Vec<_> = who
        .leaving
        .iter()
        .map(|pk| secrets.session_of(&paths.account(*pk)))
        .collect();
    // Read through the store, which owns what counts as a stored session: a
    // corrupt credential is still a credential.
    let had_session = stores.iter().any(|s| s.something_is_stored());
    // Every one is attempted, and the first refusal kept.
    let mut sessions = Ok(());
    for store in &stores {
        let deleted = store.delete();
        if sessions.is_ok() {
            sessions = deleted;
        }
    }
    // What each showed of itself goes with its session, and what each was
    // shown of others with it.
    for pk in &who.leaving {
        let account = paths.account(*pk);
        snob_store::viewer::Card::forget(&account);
        if let Err(e) = snob_store::cache::forget(&account) {
            tracing::debug!(error = %e, "an account's cache could not be removed");
        }
    }

    // **Always.** The browser profile is where every request is sent from
    // (`headless/`), so it holds the live session, kept current by the
    // browser, and a sign-out that left it would leave the session on the
    // machine while saying it had gone.
    let profiles = if who.sweep {
        crate::browser::headless::profile::every_profile(paths)
    } else {
        who.leaving
            .iter()
            .map(|pk| paths.browser_profile_for(*pk))
            .collect()
    };
    // A walk running its own browser (`SNOB_NO_OWNER`) closes it once it sees
    // the account signed out; until then the browser would make a removed
    // profile again as it closed.
    owner::let_go_of(&profiles).await;
    let mut profiles_deleted = false;
    let mut too_close = Vec::new();
    let mut profile_failure = None;
    for profile in profiles {
        if !profile.exists() {
            continue;
        }
        if !paths::is_safe_to_remove(&profile) {
            too_close.push(profile);
            continue;
        }
        // Collected, as the sessions' refusal is: one must not decide the
        // other.
        match paths::remove_tree_patiently(&profile) {
            Ok(()) => profiles_deleted = true,
            Err(e) => {
                profile_failure.get_or_insert((profile, e));
            }
        }
    }

    Left {
        had_session,
        sessions,
        profiles_deleted,
        too_close,
        profile_failure,
    }
}
