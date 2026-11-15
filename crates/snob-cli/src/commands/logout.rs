//! `snob logout`: the session and the browser profile of an account, or of
//! every one, removed from this computer.
//!
//! **The order is AGENTS.md's "Signing out"**: the jobs stopped and their lock
//! held to the end, every other process signed out, the browsers released,
//! and only then the session and the profiles removed.

use anyhow::Result;
use snob_engine::exit::ExitCode;
use snob_engine::logout::{self, Left, Who};
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::secrets::SecretStore;

use crate::cli::LogoutArgs;
use crate::ui;

/// Deletes the session and the browser profile of `account`, the account
/// resolved for this run, or of every account with `--all` or with none
/// resolved.
pub async fn run(
    args: LogoutArgs,
    secrets: SecretStore,
    paths: &AppPaths,
    account: Option<AccountPaths>,
) -> Result<ExitCode> {
    // The profile always goes, whatever the flag says: it holds the live
    // session (`snob_engine::logout`).
    let _ = args.purge_profile;
    let who = Who::of(paths, account.map(|account| account.pk()), args.all);
    let stopping = crate::jobs::dispatch::stop_before_leaving(who.accounts(paths)).await?;
    let Left {
        had_session,
        sessions,
        profiles_deleted,
        too_close,
        profile_failure,
    } = logout::sign_out(&secrets, paths, &who, &stopping).await;

    match (&sessions, had_session) {
        (Ok(()), true) => {
            crate::ui::say!("Session deleted.");
            // Worth saying: nothing was closed on Instagram's side, because
            // that would be a write and snob does not write.
            ui::info(
                "The session is still active on Instagram. To really close it, use\n\
                 \"Active sessions\" in the app's settings.",
            );
        }
        (Ok(()), false) => crate::ui::say!("There was no session stored."),
        // Nothing is claimed here.
        (Err(_), _) => {}
    }

    for profile in too_close {
        ui::warn(&format!(
            "{} is too close to the root to remove; delete it by hand",
            profile.display()
        ));
    }
    if profiles_deleted {
        if who.sweep {
            crate::ui::say!("Browser profiles deleted.");
        } else {
            crate::ui::say!("Browser profile deleted.");
        }
    }

    // After the profiles, so that one refusal does not decide the other.
    if sessions.is_err()
        && let Some((profile, io)) = &profile_failure
    {
        ui::warn(&format!(
            "{} could not be removed either: {io}",
            profile.display()
        ));
    }
    sessions?;
    if let Some((profile, e)) = profile_failure {
        return Err(
            anyhow::Error::new(e).context(format!("{} could not be removed", profile.display()))
        );
    }
    Ok(ExitCode::Ok)
}
