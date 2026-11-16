//! Signing in: a session taken from a browser of snob's own or pasted, checked
//! against Instagram, stored, and made the account commands act as.
//!
//! **Everything here is the same for a command line and an app.** What it
//! says while it works goes out as notices (`host`); how it ended comes back
//! as data ([`Stored`], and [`failure_of`] for an error), never as a line.
//! Which method, which browser and which account are asked by whoever runs
//! it, before any of this starts.

use std::path::Path;

use anyhow::{Context, Result};
use snob_core::session::Session;
use snob_ig::error::IgError;
use snob_ig::login::{self, BrowserCookies, LoginError, ValidationOutcome};
use snob_ig::pace::CancelToken;
use snob_store::paths::{self, AccountPaths, AppPaths};
use snob_store::registry::Registry;
use snob_store::secrets::SecretStore;

use crate::app::{Held, Viewer};
use crate::browser::Browser;
use crate::headless::ProfileMark;
use crate::headless::profile::{self, ProfileSwap};
use crate::host::{self, Progress, Wait};
use crate::{cdp, owner};

/// Where a session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// A `sessionid` copied from a browser the person is logged in with.
    Paste,
    /// A window of snob's own browser.
    Browser,
    /// That browser without a window, relayed to another computer.
    Remote,
}

/// A User-Agent and what is known about where it came from.
#[derive(Debug, Clone)]
pub struct Agent {
    pub user_agent: String,
    /// The person gave it outright, so nothing may rewrite it later.
    pub pinned: bool,
    /// Which browser it describes, when one was picked.
    pub browser: Option<String>,
}

impl Agent {
    /// One the person gave outright.
    pub fn pinned(user_agent: String) -> Self {
        Self {
            user_agent,
            pinned: true,
            browser: None,
        }
    }

    /// The one `browser` sends.
    pub fn of(browser: &Browser) -> Self {
        Self {
            user_agent: browser.user_agent(),
            pinned: false,
            browser: Some(browser.name.to_string()),
        }
    }
}

/// No browser snob can open is installed.
#[derive(Debug)]
pub struct NoBrowser;

impl std::fmt::Display for NoBrowser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "no Chromium-based browser was found installed, and this needs one to open.\n\
             Use \"snob login --paste\" instead.",
        )
    }
}

impl std::error::Error for NoBrowser {}

/// Whether Instagram was asked about the session, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Confirmed,
    /// Not confirmed, because Instagram is throttling: probably valid.
    Unconfirmed,
    /// Not asked: the account may send nothing until then.
    Skipped(Held),
}

/// A login that ended with a session stored.
#[derive(Debug, Clone)]
pub struct Stored {
    /// The account that signed in, which commands act as from now on.
    pub viewer: Viewer,
    pub check: Check,
    /// Where the session is kept, in words ("system keyring").
    pub kept_in: String,
    /// How many accounts are signed in here now.
    pub accounts: usize,
    /// The account commands acted as before, when there was one.
    pub was_active: Option<Viewer>,
    /// The account this login was meant to renew, when another signed in
    /// and that one's session is still stored.
    pub stays_signed_in: Option<Viewer>,
}

/// What a browser login is told before it starts.
pub struct ByBrowser<'a> {
    pub browser: &'a Browser,
    /// The person gave the User-Agent outright.
    pub user_agent_pinned: bool,
    /// The account logging in again, on its own profile; none for a new one.
    pub renewing: Option<Viewer>,
    pub method: Method,
}

/// The profile a login is about to happen in.
pub struct Opened<'a> {
    pub profile: &'a Path,
    /// Where the profile is kept once the login is done, to name to the
    /// person: a new login's has a name of its own until its account is known.
    pub kept_at: &'a Path,
}

/// Logs in with snob's own browser: `capture` opens it on the profile it is
/// handed and comes back with the session's cookies and the User-Agent they
/// belong to ([`in_a_window`], or a relay of the caller's).
///
/// `None` when the login was canceled.
pub async fn by_browser(
    login: ByBrowser<'_>,
    store: SecretStore,
    paths: &AppPaths,
    cancel: &CancelToken,
    capture: impl AsyncFnOnce(Opened<'_>) -> Result<(BrowserCookies, String)>,
) -> Result<Option<Stored>> {
    let ByBrowser {
        browser,
        user_agent_pinned,
        renewing,
        method,
    } = login;
    let (profile, existed) = match &renewing {
        Some(previous) => {
            owner::release(paths, Some(previous.pk)).await;
            profile::for_account(paths, previous.pk)?
        }
        None => (profile::for_a_login(paths)?, false),
    };
    let kept_at = match &renewing {
        Some(_) => profile.clone(),
        None => paths.browser_profile(),
    };

    // Held rather than unwrapped until the browser has been shut down: a
    // cancellation has to read as one, not as a failure.
    let captured = capture(Opened {
        profile: &profile,
        kept_at: &kept_at,
    })
    .await;
    // Whatever `capture` came back with: the profile holds a live session
    // from the moment the form was submitted.
    //
    // **Never when it was already here**: the commonest way for this launch to
    // fail is another snob holding that very profile, and it is not deleted
    // under that snob's browser.
    if !existed && (captured.is_err() || cancel.is_canceled()) {
        // Off the worker: the removal retries with sleeps adding up to five
        // seconds, and a worker has to stay free for what stops the run.
        let profile = profile.clone();
        tokio::task::spawn_blocking(move || discard_profile(&profile))
            .await
            .context("the profile could not be removed")?;
    }
    if cancel.is_canceled() {
        return Ok(None);
    }
    let (cookies, user_agent) = captured?;

    let mut session = login::session_from_cookies(&cookies, &user_agent)?;
    session.user_agent_pinned = user_agent_pinned;
    session.browser = Some(browser.name.to_string());

    if let Some(previous) = &renewing
        && existed
        && previous.pk != session.ds_user_id
    {
        let previous = previous.label();
        host::warn(&format!(
            "account {} signed in on the browser profile of {previous}, which is now \
             {}'s: Instagram has seen both sign in from it. {previous} starts from a \
             fresh profile when it logs in again.",
            session.ds_user_id, session.ds_user_id
        ));
    }

    // A browser the owner started on the account's profile since this login
    // began, for another run, is closed before the profile is replaced.
    owner::release(paths, Some(session.ds_user_id)).await;
    if let Some(previous) = &renewing
        && previous.pk != session.ds_user_id
    {
        owner::release(paths, Some(previous.pk)).await;
    }

    // The login happened in this profile, so it is the device Instagram just
    // saw sign in, and it becomes the account's before anything is sent: the
    // check below goes out from the account's profile.
    let swap = match ProfileSwap::replace(paths, &profile, session.ds_user_id) {
        Ok(swap) => Some(swap),
        Err(e) => {
            host::warn(&format!(
                "{e:#}\n\
                 The profile the login happened in stays at {}, holding the session, \
                 and the account's requests go out from another; \"snob logout\" \
                 removes it.",
                profile.display()
            ));
            None
        }
    };
    let made_it = swap
        .as_ref()
        .map_or(profile.as_path(), |s| s.profile.as_path());
    // The profile made this session, and is the browser's to keep current from
    // now on: the next run must not write the stored copy back over it.
    ProfileMark::after_login(made_it, browser, &session);
    let finished = finish(session, store, method, paths, renewing.as_ref(), cancel).await;
    if let Some(swap) = swap {
        match &finished {
            Ok(_) => swap.keep(),
            Err(_) => swap.undo(),
        }
    }
    finished.map(Some)
}

/// Removes a profile a login created and did not finish.
///
/// **About 90 MB, and possibly a half-finished session.**
fn discard_profile(profile: &Path) {
    if !profile.exists() {
        return;
    }
    if !paths::is_safe_to_remove(profile) {
        host::warn(&format!(
            "{} is too close to the root to remove; delete it by hand",
            profile.display()
        ));
        return;
    }

    if let Err(e) = paths::remove_tree_patiently(profile) {
        host::warn(&format!(
            "the browser profile at {} could not be removed ({e}). It holds a \
             logged-in session; delete it by hand, or run \
             \"snob logout\" later.",
            profile.display()
        ));
    }
}

/// Opens `browser` in a window on `profile`, waits for the person to log in
/// there, and always closes it: the session's cookies, and the User-Agent
/// they belong to (`requested_user_agent` when the person gave one).
pub async fn in_a_window(
    browser: &Browser,
    requested_user_agent: Option<String>,
    profile: &Path,
    cancel: &CancelToken,
    progress: &dyn Progress,
) -> Result<(BrowserCookies, String)> {
    let cdp = cdp::Cdp::connect(cdp::launch(browser, profile)?, cancel).await?;
    let outcome = collect(&cdp, browser, requested_user_agent, cancel, progress).await;
    cdp.close().await;
    outcome
}

async fn collect(
    cdp: &cdp::Cdp,
    browser: &Browser,
    requested_user_agent: Option<String>,
    cancel: &CancelToken,
    progress: &dyn Progress,
) -> Result<(BrowserCookies, String)> {
    // Asked for rather than reconstructed: this is the browser the cookie will
    // belong to, and Instagram checks that the two agree.
    let user_agent = match requested_user_agent {
        Some(ua) => ua,
        None => cdp.user_agent().await.unwrap_or_else(|e| {
            tracing::debug!(error = %e, "falling back to the rebuilt User-Agent");
            browser.user_agent()
        }),
    };

    // The profile keeps whatever was logged in last time, and Instagram sends a
    // browser that still has a session straight past the login page.
    let cookies = match cdp.instagram_cookies().await? {
        Some(existing) => {
            host::info(STILL_LOGGED_IN);
            existing
        }
        None => {
            // Ten minutes with nothing on screen reads as a hang, and this is
            // the one stretch where the person is in another window typing a
            // password and a code and comes back to check.
            progress.waiting(Wait::LogIn, cdp::LOGIN_TIMEOUT);
            let captured = cdp::wait_for_login(cdp, cancel).await;
            // Before the `?`, so a cancellation and a broken pipe both leave
            // nothing drawn behind.
            progress.finish();
            captured?
        }
    };

    Ok((cookies, user_agent))
}

/// Said when the profile a login opens is still signed in.
pub const STILL_LOGGED_IN: &str = "This browser profile was still logged in, so that session is the one \
     being stored.\n\
     To sign in as somebody else, run \"snob login --add\".";

/// Logs in with a pasted `sessionid`, sent as `agent`.
pub async fn by_paste(
    sessionid: &str,
    agent: Agent,
    csrftoken: Option<&str>,
    store: SecretStore,
    paths: &AppPaths,
    cancel: &CancelToken,
) -> Result<Stored> {
    let mut session = login::session_from_paste(sessionid, &agent.user_agent)?;
    session.user_agent_pinned = agent.pinned;
    session.browser = agent.browser;
    // The requests go out from the browser that made the account's profile,
    // while it is installed (`ProfileMark`), whichever one was named here.
    if let Some(named) = &session.browser
        && !crate::headless::env_flag("SNOB_NO_BROWSER")
        && let Some(maker) =
            ProfileMark::read(&paths.browser_profile_for(session.ds_user_id)).browser
        && maker.is_file()
        && crate::browser::detect_named(named).is_some_and(|b| b.path != maker)
    {
        host::info(&format!(
            "Requests for this account go out from {}, the browser that made its profile, \
             rather than from {named}: a profile opens only in the browser that made it.",
            maker.display()
        ));
    }
    // Taken as given rather than asked for: it is the CSRF double-submit
    // token, worth nothing without the cookie.
    session.csrftoken = csrftoken
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(Into::into);
    // The check goes out from a browser that takes this session, whatever the
    // clock said when the one it held was made (`headless::newer_login`).
    owner::release(paths, Some(session.ds_user_id)).await;
    ProfileMark::forget_session(&paths.browser_profile_for(session.ds_user_id));
    finish(session, store, Method::Paste, paths, None, cancel).await
}

/// Says whose session is about to be replaced.
pub fn announce_replacement(stored: &Session) {
    let who = who(stored);
    host::info(&format!(
        "There is already a session for {who}. Logging in as {who} again replaces it."
    ));
}

/// The account a session belongs to.
fn viewer_of(session: &Session) -> Viewer {
    Viewer {
        pk: session.ds_user_id,
        username: session.username.clone(),
    }
}

/// How to name the account a session belongs to.
fn who(session: &Session) -> String {
    viewer_of(session).label()
}

/// Checks `session` against Instagram, stores it, and makes its account the
/// one commands act as.
///
/// **A login that fails leaves what worked before working**: the session
/// stored before stays, and an account directory made for a session Instagram
/// rejected goes.
pub async fn finish(
    mut session: Session,
    secrets: SecretStore,
    method: Method,
    paths: &AppPaths,
    renewing: Option<&Viewer>,
    cancel: &CancelToken,
) -> Result<Stored> {
    let pk = session.ds_user_id;
    let account = paths.account(pk);
    let existed = account.dir().exists();
    // Before anything is written to the account's own database, which would
    // then be the one it keeps.
    let adopted = match snob_store::layout::adopt(paths, pk) {
        Ok(adopted) => adopted,
        Err(e) => {
            host::warn(&format!(
                "{e}\nThe data kept from before several accounts could be signed in stays at {}.",
                paths.unclaimed_dir().display()
            ));
            false
        }
    };
    let store = secrets.session_of(&account);
    let previous = store
        .load()
        .ok()
        .flatten()
        .filter(|previous| previous.ds_user_id == pk);
    if let Some(previous) = &previous
        && renewing.is_none_or(|renewing| renewing.pk != pk)
    {
        announce_replacement(previous);
    }
    // Neither method hands over a username.
    let kept = previous.as_ref().map(|previous| previous.created_at);
    if session.username.is_none() {
        session.username = previous.and_then(|previous| previous.username);
    }

    // Checking a session is a request like any other, charged to the budget
    // of the account that signed in: repeated logins spend.
    let pacer = crate::app::pacer_saying_a_line(&account, cancel.clone())?;
    // Let in before the check, which goes out as any request does: a logout or
    // purge of the account signed out every session it had then, and this one
    // is a login's own (`owner::signed_out`).
    account.admit_login(session.created_at, kept)?;
    host::info("Checking the session against Instagram...");
    // The check goes out from the browsers, as every request does.
    crate::owner::ensure(paths);
    let validated = login::validate_seeing(&mut session, pacer).await;
    let rejected = matches!(
        &validated,
        Err(LoginError::Instagram(ig)) if ig.invalidates_session()
    );
    let stored = match validated {
        Ok((outcome, face)) => store
            .save(&session)
            .map(|()| {
                // Beside the session it came with, and gone with it.
                if let Some(face) = face {
                    crate::viewer::keep(&account, face);
                }
                outcome
            })
            .map_err(Into::into),
        // A rejection during login means different things depending on where
        // the session came from.
        Err(e) => Err(match &e {
            LoginError::Instagram(IgError::SignedOutHere) => anyhow::anyhow!(e),
            LoginError::Instagram(ig) if ig.invalidates_session() => {
                anyhow::anyhow!(e).context(match method {
                    Method::Paste => {
                        "Instagram rejected the session. Check that the sessionid was \
                         copied whole and that the User-Agent belongs to the same browser"
                    }
                    Method::Browser | Method::Remote => {
                        "Instagram rejected the session the browser handed over. It may \
                         have been signed out in the meantime; run \"snob logout\" and \
                         log in again"
                    }
                })
            }
            _ => anyhow::anyhow!(e),
        }),
    };
    let outcome = match stored {
        Ok(outcome) => outcome,
        Err(e) => {
            // The browser the check went out from holds this session now, as
            // the account's newest, and would go on sending it in place of the
            // one still stored; and a browser login's profile is about to be
            // put back under it.
            owner::release(paths, Some(pk)).await;
            ProfileMark::forget_session(&paths.browser_profile_for(pk));
            forget_the_attempt(&account, adopted, !existed && rejected);
            return Err(e);
        }
    };
    if adopted {
        host::info(&format!(
            "The data kept from before several accounts could be signed in is {}'s now.",
            who(&session)
        ));
    }

    // The account that signed in is the one commands act as from now on.
    let username = session.username.clone();
    let mut was_active = None;
    let registry = Registry::update(paths, |registry| {
        was_active = registry.active_account().map(Viewer::from);
        let name = username
            .or_else(|| registry.get(pk).map(|a| a.username.clone()))
            .unwrap_or_default();
        registry.upsert(pk, &name, snob_core::clock::now());
        registry.active = Some(pk);
    })
    .context("the session was stored, but the list of accounts could not be updated")?;

    let check = match outcome {
        ValidationOutcome::Confirmed => Check::Confirmed,
        ValidationOutcome::Unconfirmed => Check::Unconfirmed,
        // Asked again, as the check's pacer is spent: what held it may be the
        // brake on every account rather than a cooldown of its own.
        ValidationOutcome::Skipped { until_ms } => Check::Skipped(
            crate::app::pacer_saying_a_line(&account, cancel.clone())
                .and_then(|pacer| crate::app::held(&pacer, Some(paths)))
                .ok()
                .flatten()
                .unwrap_or(Held {
                    until_ms,
                    braked: Vec::new(),
                }),
        ),
    };
    // A browser login meant for another account that signed in as this one
    // leaves that account's session where it was.
    let stays_signed_in = renewing
        .filter(|renewing| renewing.pk != pk)
        .filter(|renewing| {
            secrets
                .session_of(&paths.account(renewing.pk))
                .something_is_stored()
        })
        .cloned();

    Ok(Stored {
        viewer: viewer_of(&session),
        check,
        kept_in: store.describe().to_string(),
        accounts: registry.accounts.len(),
        was_active,
        stays_signed_in,
    })
}

/// Undoes what a login that did not happen left for an account nothing signs in
/// as, so the id stays nobody's: the data it adopted goes back to waiting, and
/// a directory it made for a session Instagram rejected goes.
fn forget_the_attempt(account: &AccountPaths, adopted: bool, made_for_nothing: bool) {
    let listed =
        Registry::load(account).map_or(true, |registry| registry.get(account.pk()).is_some());
    if listed {
        return;
    }
    if adopted {
        if let Err(e) = snob_store::layout::unadopt(account, account.pk()) {
            host::warn(&format!(
                "{e}\nThe data kept from before several accounts could be signed in stays at {}.",
                account.dir().display()
            ));
        }
    } else if made_for_nothing
        && paths::is_safe_to_remove(&account.dir())
        && let Err(e) = paths::remove_tree_patiently(&account.dir())
    {
        host::warn(&format!(
            "{} could not be removed: {e}",
            account.dir().display()
        ));
    }
}

/// Why a login did not end with a session stored, for a program to act on:
/// a view says each in its own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// No browser snob can open is installed.
    NoBrowser,
    /// The person closed the login window before logging in.
    WindowClosed,
    /// Nobody logged in within [`cdp::LOGIN_TIMEOUT`].
    TimedOut,
    /// What was pasted cannot be a `sessionid`, or the User-Agent a browser's.
    Malformed,
    /// Instagram does not take the session: expired, or never valid.
    Rejected,
    /// Instagram wants the account verified first.
    Challenge,
    /// Instagram is throttling, or told the account to wait.
    PushedBack,
    /// Instagram could not be reached.
    Network,
    /// Anything else.
    Other,
}

/// What kind of failure `error` is, from what it carries.
pub fn failure_of(error: &anyhow::Error) -> Failure {
    for cause in error.chain() {
        if cause.is::<NoBrowser>() {
            return Failure::NoBrowser;
        }
        if cause.is::<cdp::WindowClosed>() {
            return Failure::WindowClosed;
        }
        if cause.is::<cdp::NobodyLoggedIn>() {
            return Failure::TimedOut;
        }
        if cause.is::<snob_core::session::SessionError>() {
            return Failure::Malformed;
        }
        let ig = match cause.downcast_ref::<LoginError>() {
            Some(LoginError::Instagram(ig)) => Some(ig),
            Some(_) => return Failure::Malformed,
            None => cause.downcast_ref::<IgError>(),
        };
        if let Some(ig) = ig {
            return match ig {
                IgError::Challenge { .. } | IgError::Checkpoint { .. } => Failure::Challenge,
                ig if ig.invalidates_session() => Failure::Rejected,
                IgError::RateLimited | IgError::FeedbackRequired => Failure::PushedBack,
                IgError::Network(_) | IgError::Unreachable(_) => Failure::Network,
                _ => Failure::Other,
            };
        }
    }
    Failure::Other
}
