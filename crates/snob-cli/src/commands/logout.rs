//! `snob logout`: the session and the browser profile of an account, or of
//! every one, removed from this computer.
//!
//! **The order is AGENTS.md's "Signing out"**: the jobs stopped and their lock
//! held to the end, every other process signed out, the browsers released,
//! and only then the session and the profiles removed.

use anyhow::Result;
use snob_core::Pk;
use snob_engine::exit::ExitCode;
use snob_store::paths::{self, AccountPaths, AppPaths};
use snob_store::registry::Registry;
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
    let everyone = Registry::known_accounts(paths);
    // With no account resolved every one found goes, sessions and profiles
    // alike: a registry that is gone must not leave the sessions behind while
    // the profiles are swept.
    let all = args.all || account.is_none();
    let leaving: Vec<Pk> = match &account {
        Some(account) if !all => vec![account.pk()],
        _ => everyone.clone(),
    };
    // Every profile there is when no other account's is to be kept: with
    // `--all` or no account resolved, and where there is no other account.
    let sweep = all || everyone.len() <= 1;
    // The jobs of the accounts leaving first: a walk still running would go on
    // with the session being deleted, and start a browser again on the profile
    // about to go.
    let _stopping = crate::jobs::dispatch::stop_before_leaving(
        leaving.iter().map(|pk| paths.account(*pk)).collect(),
    )
    .await?;
    // Then every other process still holding one of these sessions: a walk
    // typed in another terminal, the monitor's run, an agent's call.
    snob_engine::owner::sign_out(paths, &leaving);
    // A browser still running on a profile about to go would write into it as
    // it went, or keep Windows from removing it.
    snob_engine::owner::release(paths, if sweep { None } else { account.map(|a| a.pk()) }).await;
    let stores: Vec<_> = leaving
        .iter()
        .map(|pk| secrets.session_of(&paths.account(*pk)))
        .collect();
    // Read through the store, which owns what counts as a stored session and why
    // — a corrupt credential is still a credential.
    let had_session = stores.iter().any(|s| s.something_is_stored());
    // Not `?`.
    let mut removal = Ok(());
    for store in &stores {
        let deleted = store.delete();
        if removal.is_ok() {
            removal = deleted;
        }
    }

    match (&removal, had_session) {
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

    // **Always.** The browser profile is where every request is sent from
    // (`headless/`), so it holds the live session — kept current by the
    // browser, fresher than the stored copy — and a logout that left it would
    // leave the session on the machine while saying it had gone.
    let _ = args.purge_profile;
    let profiles = if sweep {
        snob_engine::headless::profile::every_profile(paths)
    } else {
        leaving
            .iter()
            .map(|pk| paths.browser_profile_for(*pk))
            .collect()
    };
    // A walk running its own browser (`SNOB_NO_OWNER`) closes it once it sees
    // the account signed out; until then the browser would make a removed
    // profile again as it closed.
    snob_engine::owner::let_go_of(&profiles).await;
    let mut deleted = false;
    let mut profile_failure = None;
    for profile in profiles {
        if !profile.exists() {
            continue;
        }
        // Guarded like every other recursive delete in the tool.
        if !paths::is_safe_to_remove(&profile) {
            ui::warn(&format!(
                "{} is too close to the root to remove; delete it by hand",
                profile.display()
            ));
            continue;
        }
        // Collected rather than `?`-ed, for the same reason `removal` above is:
        // one refusal must not decide the other, and a keyring that also
        // refused is still said at the end.
        match paths::remove_tree_patiently(&profile) {
            Ok(()) => deleted = true,
            Err(e) => {
                profile_failure.get_or_insert((profile, e));
            }
        }
    }
    if deleted {
        if sweep {
            crate::ui::say!("Browser profiles deleted.");
        } else {
            crate::ui::say!("Browser profile deleted.");
        }
    }

    // After the profiles, so that one refusal does not decide the other.
    if removal.is_err()
        && let Some((profile, io)) = &profile_failure
    {
        ui::warn(&format!(
            "{} could not be removed either: {io}",
            profile.display()
        ));
    }
    removal?;
    if let Some((profile, e)) = profile_failure {
        return Err(
            anyhow::Error::new(e).context(format!("{} could not be removed", profile.display()))
        );
    }
    Ok(ExitCode::Ok)
}
