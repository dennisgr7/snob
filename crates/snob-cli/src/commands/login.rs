//! `snob login`.
//!
//! Everything that can fail on its own — the store being writable, a browser
//! being installed, which browser — is settled **before** the user is asked for
//! anything, so nobody completes a two-factor login only to be told afterwards
//! that there was nowhere to put the result.

use anyhow::{Context, Result, bail};
use snob_engine::app::Viewer;
use snob_engine::exit::{ExitCode, ExitError};
use snob_engine::login::{self, Agent, ByBrowser, Check, Method, Opened, Stored};
use snob_engine::{browser, cdp, interrupt};
use snob_ig::login::BrowserCookies;
use snob_ig::pace::CancelToken;
use snob_store::paths::AppPaths;
use snob_store::secrets::{Backend, SecretStore};

use crate::cli::LoginArgs;
use crate::progress::Progress;
use crate::ui::{self, LoginMethod};

/// Logs in again as `renewing`, the account resolved for this run, or as
/// another account.
pub async fn run(
    args: LoginArgs,
    store: SecretStore,
    paths: &AppPaths,
    renewing: Option<Viewer>,
    named: bool,
) -> Result<ExitCode> {
    // Before anything is asked for: if the session cannot be stored anywhere,
    // better to find out now than after two-factor authentication.
    let wanted = store.backend();
    let usable = match store.probe_writable() {
        Ok(backend) => backend,
        Err(e) => bail!(
            "{e}\n\
             There is nowhere to put the session: neither the system keyring nor \
             a file in the data directory could be written."
        ),
    };

    // Falling back is fine.
    if wanted == Backend::Keyring && usable == Backend::File {
        ui::warn(
            "no system keyring is available here, so the session goes to a \
             protected file instead.\n\
             On a headless machine that is normal — Secret Service needs a \
             desktop session — and the file is readable only by you.",
        );
    }
    let store = store.using(usable);

    // Before anything is asked for, and this is the half the probe above does
    // not cover: `probe_writable` returns as soon as the keyring answers, so it
    // never touches the data directory at all on a machine that has one.
    writable(paths)?;

    let Some(method) = choose_method(&args)? else {
        ui::info("Login canceled.");
        return Ok(ExitCode::Interrupted);
    };

    match method {
        // A paste is whichever account the sessionid is: there is nothing to
        // renew or add, and `finish` says what it replaces once it knows.
        LoginMethod::Paste => by_paste(args, store, paths).await,
        method @ (LoginMethod::Browser | LoginMethod::Remote) => {
            // A question not answered is 130, as it is everywhere else —
            // `watch setup`'s menu, a confirmation — and as the exit codes say.
            let renewing = match renewing {
                Some(account) if !named && ui::can_show_a_menu() => match renew_or_add(&account)? {
                    Some(renew) => renew.then_some(account),
                    None => {
                        ui::info("Login canceled.");
                        return Ok(ExitCode::Interrupted);
                    }
                },
                other => other,
            };
            // Before the browser opens, which is when there is still time to
            // stop: the profile is this account's.
            if let Some(account) = &renewing
                && let Ok(Some(stored)) = store.session_of(&paths.account(account.pk)).load()
            {
                login::announce_replacement(&stored);
            }
            by_browser(args, store, paths, renewing, method).await
        }
    }
}

/// Adds an account through a browser login, as `snob login --add --browser`
/// does, for the interactive views' account picker, which has no command line
/// to take the flags from.
pub async fn add_by_browser(store: SecretStore, paths: &AppPaths) -> Result<ExitCode> {
    let remote = snob_engine::seat::Seat::here().prefers_remote();
    let args = LoginArgs {
        paste: false,
        browser: !remote,
        remote,
        user_agent: None,
        csrftoken: None,
        add: true,
        keep_profile: false,
    };
    // Renewing no account is what makes this an add; `add` mirrors the flag.
    run(args, store, paths, None, false).await
}

/// Asks whether this login is the account in use again, or another one:
/// `Some(true)` to log in again, `None` when the question was declined.
fn renew_or_add(account: &Viewer) -> Result<Option<bool>> {
    let again = format!("Log in again as {}", account.label());
    let chosen = ui::menu::choose(
        "Which account is this login for?",
        &[again.as_str(), "Add another account"],
    )?;
    Ok(chosen.map(|i| i == 0))
}

/// Whether the data directory takes a file: the account's database goes
/// under it once the login says whose it is.
fn writable(paths: &AppPaths) -> Result<()> {
    paths.ensure_dirs()?;
    let probe = paths.data_dir().join(".write-probe");
    std::fs::write(&probe, b"probe")
        .with_context(|| format!("{} cannot be written to", paths.data_dir().display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Works out the method: whatever the flags say, or whatever the user picks
/// from the menu.
fn choose_method(args: &LoginArgs) -> Result<Option<LoginMethod>> {
    choose_method_at(args, snob_engine::seat::Seat::here())
}

fn choose_method_at(
    args: &LoginArgs,
    seat: &snob_engine::seat::Seat,
) -> Result<Option<LoginMethod>> {
    if args.paste {
        return Ok(Some(LoginMethod::Paste));
    }
    if args.remote {
        return Ok(Some(LoginMethod::Remote));
    }
    if args.browser {
        // Said outright, and taken as said — except where a window cannot
        // open at all, which Chromium would only report as exiting with 1.
        if seat.window == snob_engine::seat::Window::Nowhere {
            bail!("{}", no_window_here(seat));
        }
        return Ok(Some(LoginMethod::Browser));
    }
    if !ui::can_show_a_menu() {
        let example = if seat.prefers_remote() {
            "\"snob login --remote\" or \"snob login --paste\""
        } else {
            "\"snob login --paste\""
        };
        bail!(
            "there is no interactive terminal to show the menu in.\n\
             Give the method explicitly, for example {example}."
        );
    }
    ui::choose_login_method(seat)
}

/// Why `--browser` cannot work at `seat`, and what does.
fn no_window_here(seat: &snob_engine::seat::Seat) -> String {
    let why = if seat.ssh.is_some() {
        "this is an SSH session to a machine with no display"
    } else {
        "there is no display here (neither DISPLAY nor WAYLAND_DISPLAY is set)"
    };
    format!(
        "a browser window cannot open: {why}.\n\
         \"snob login --remote\" logs in from the browser of the computer you are at, \
         through an SSH tunnel; \"snob login --paste\" takes a sessionid copied from a \
         browser where you are logged in."
    )
}

/// Which browser this login is about.
fn choose_browser(
    purpose: &str,
    installed: &[browser::Browser],
) -> Result<Option<browser::Browser>> {
    // Nothing to ask about: `first` is already `None` on an empty list and
    // already the only entry on a list of one, so the two cases that have no
    // question in them and the case where there is nobody to ask are one line.
    if installed.len() < 2 || !ui::can_show_a_menu() {
        return Ok(installed.first().cloned());
    }

    let labels: Vec<String> = installed
        .iter()
        .map(|b| format!("{} {}", b.name, b.major_version))
        .collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    match ui::menu::choose(purpose, &refs) {
        // `get`, which is total, rather than trusting the index to be in range.
        Ok(Some(i)) => Ok(installed.get(i).cloned()),
        Ok(None) => {
            Err(ExitError::new(ExitCode::Interrupted, "no browser was chosen".to_string()).into())
        }
        Err(e) if snob_engine::exit::from_chain(&e) == Some(ExitCode::Interrupted) => Err(e),
        // The menu failing to draw: the preferred one still beats refusing to
        // continue.
        Err(_) => Ok(installed.first().cloned()),
    }
}

/// Opens a browser, waits for the login, and takes the session from it.
async fn by_browser(
    args: LoginArgs,
    store: SecretStore,
    paths: &AppPaths,
    renewing: Option<Viewer>,
    method: LoginMethod,
) -> Result<ExitCode> {
    let remote = method == LoginMethod::Remote;
    let seat = snob_engine::seat::Seat::here();
    if !remote && seat.window == snob_engine::seat::Window::Elsewhere {
        ui::info(
            "This is an SSH session: the browser window opens on this machine's own screen, \
             not on the computer you are at. \"snob login --remote\" shows it there instead.",
        );
    }
    let installed = browser::detect_all();
    let purpose = if remote {
        "Which browser should snob log in with?"
    } else {
        "Which browser should snob open?"
    };
    let Some(found) = choose_browser(purpose, &installed)? else {
        return Err(login::NoBrowser.into());
    };
    let cancel = interrupt::install();
    let requested = args.user_agent.clone();

    let stored = login::by_browser(
        ByBrowser {
            browser: &found,
            user_agent_pinned: requested.is_some(),
            renewing,
            method: if remote {
                Method::Remote
            } else {
                Method::Browser
            },
        },
        store,
        paths,
        &cancel,
        async |opened: Opened<'_>| {
            if remote {
                ui::info(&format!(
                    "Starting {} here, without a window, on Instagram's login page.\n\
                     It uses a profile of its own, under {}, so it is not your everyday browser \
                     and logging in there changes nothing about it.",
                    found.name,
                    opened.kept_at.display(),
                ));
                capture_remotely(&found, requested, opened.profile, paths, &cancel).await
            } else {
                ui::info(&format!(
                    "Opening {} on Instagram's login page.\n\
                     It uses a profile of its own, under {}, so it is not your everyday browser \
                     and logging in there changes nothing about it.\n\
                     Log in as usual; snob will notice when you are done, and gives up after \
                     {} minutes. Ctrl+C to cancel.",
                    found.name,
                    opened.kept_at.display(),
                    cdp::LOGIN_TIMEOUT.as_secs() / 60
                ));
                let progress = Progress::new(true);
                login::in_a_window(&found, requested, opened.profile, &cancel, &progress).await
            }
        },
    )
    .await?;
    match stored {
        Some(stored) => Ok(say(&stored)),
        None => {
            ui::info("Login canceled.");
            Ok(ExitCode::Interrupted)
        }
    }
}

/// [`login::in_a_window`], for a login watched from another machine: the browser runs here
/// without a window, and its tab is relayed through a tunnel to a page in the
/// person's own browser (`snob_engine::relay`), until the session is in it.
async fn capture_remotely(
    found: &browser::Browser,
    requested_user_agent: Option<String>,
    profile: &std::path::Path,
    paths: &AppPaths,
    cancel: &CancelToken,
) -> Result<(BrowserCookies, String)> {
    let relay = snob_engine::relay::Relay::bind(paths)?;
    let snob_engine::headless::RemoteLogin {
        cdp,
        tab,
        user_agent,
    } = snob_engine::headless::for_a_remote_login(found, profile, paths, cancel).await?;
    // An explicit --user-agent still wins, as in a windowed login.
    let user_agent = requested_user_agent.unwrap_or(user_agent);
    let outcome = relayed(&cdp, &tab, relay, cancel).await;
    cdp.close().await;
    outcome.map(|cookies| (cookies, user_agent))
}

async fn relayed(
    cdp: &cdp::Cdp,
    tab: &str,
    relay: snob_engine::relay::Relay,
    cancel: &CancelToken,
) -> Result<BrowserCookies> {
    // As in a windowed login: a profile still signed in is taken as it is.
    if let Some(existing) = cdp.instagram_cookies().await? {
        ui::info(login::STILL_LOGGED_IN);
        return Ok(existing);
    }
    // Only now, with the tab dressed (`headless::for_a_remote_login`): the
    // login page's first request already carries the identity every request
    // after it will.
    cdp.page_call(
        tab,
        "Page.navigate",
        serde_json::json!({ "url": cdp::login_url() }),
        cdp::CALL_TIMEOUT,
    )
    .await?;
    ui::info(&remote_instructions(
        &relay,
        snob_engine::seat::Seat::here().ssh.as_ref(),
    ));
    // One bar, started once nothing else is printed.
    let progress = Progress::new(true);
    let captured = relay.serve(cdp, tab, cancel, &progress).await;
    progress.finish();
    captured
}

/// What to do on the computer the person is at, for `relay`.
fn remote_instructions(
    relay: &snob_engine::relay::Relay,
    ssh: Option<&snob_engine::seat::Ssh>,
) -> String {
    let named = ssh.and_then(|s| s.server.as_ref()).is_some();
    let host_note = if named {
        "(or with the name you usually give ssh for this machine)"
    } else {
        "(with the name you usually give ssh for this machine in place of \"this-machine\")"
    };
    format!(
        "\nOn the computer you are at, open a tunnel to this machine, in a terminal of its \
         own:\n\n\
        \x20   {}\n\n\
         {host_note}, and then open this link in a browser there:\n\n\
        \x20   {}\n\n\
         The link opens once, in one tab, and only through that tunnel. Log in on the page \
         it shows, which is snob's browser on this machine; snob notices when you are \
         done, and gives up after {} minutes. Ctrl+C to cancel.\n\
         If the tunnel drops, open it again and reload that tab. If ssh stops at once \
         because that port is taken on the computer you are at, press Ctrl+C here and \
         run \"snob login --remote\" again for another; if it refuses to forward at all, \
         use \"snob login --paste\" instead.\n",
        relay.tunnel(ssh),
        relay.link(),
        cdp::LOGIN_TIMEOUT.as_secs() / 60
    )
}

async fn by_paste(args: LoginArgs, store: SecretStore, paths: &AppPaths) -> Result<ExitCode> {
    // The User-Agent is settled first because it is the half that can still ask
    // questions.
    let chosen = match args.user_agent {
        // The flag is the user saying it outright.
        Some(ua) => Agent::pinned(ua),
        None => resolve_user_agent()?,
    };

    ui::paste_instructions();

    let sessionid = ui::prompt_secret("sessionid: ")?;
    if sessionid.trim().is_empty() {
        bail!("no sessionid was entered");
    }

    let cancel = interrupt::install();
    let stored = login::by_paste(
        &sessionid,
        chosen,
        args.csrftoken.as_deref(),
        store,
        paths,
        &cancel,
    )
    .await?;
    Ok(say(&stored))
}

/// Says how a login ended.
fn say(stored: &Stored) -> ExitCode {
    let now = stored.viewer.label();
    match &stored.check {
        Check::Confirmed => {
            crate::ui::say!("Session stored for {now} in the {}.", stored.kept_in);
        }
        Check::Unconfirmed => {
            crate::ui::say!("Session stored in the {}.", stored.kept_in);
            ui::warn(
                "it could not be confirmed with Instagram because it is throttling \
                 requests. The session is probably valid; check in a few minutes \
                 with \"snob whoami\".",
            );
        }
        Check::Skipped(held) => {
            crate::ui::say!("Session stored in the {}.", stored.kept_in);
            ui::warn(&format!(
                "it was not checked: {}. Nothing is spent until then — not even the \
                 single request this would cost. Check it with \"snob whoami\" once it \
                 lifts.",
                snob_engine::sentences::held_until(held)
            ));
        }
    }
    // Said when it moved, and only where there is another to have moved from.
    if stored.accounts > 1
        && stored
            .was_active
            .as_ref()
            .is_none_or(|was| was.pk != stored.viewer.pk)
    {
        match &stored.was_active {
            Some(was) => crate::ui::say!("Active account: {now} (was {})", was.label()),
            None => crate::ui::say!("Active account: {now}"),
        }
    }

    if let Some(other) = &stored.stays_signed_in {
        ui::info(&format!(
            "{} stays signed in; \"snob logout --account {}\" signs it out.",
            other.label(),
            other.pk
        ));
    }

    // Somebody who has just logged in has no idea what to type next, and the
    // whole-account summary is the answer to the question they came with.
    ui::info("Try \"snob scan\" for the whole picture, or \"snob unfollowers\".");

    ExitCode::Ok
}

/// Gets the User-Agent without sending the user to the browser console.
fn resolve_user_agent() -> Result<Agent> {
    let installed = browser::detect_all();

    if let Some(b) = choose_browser("Which browser is your Instagram session in?", &installed)? {
        ui::info(&format!(
            "Using the User-Agent of {} {}.",
            b.name, b.major_version
        ));

        // With one browser installed nothing was asked, so the confirmation is
        // the only chance to say it guessed wrong.
        if installed.len() == 1 && !ui::confirm("Is that the browser your session is in?", true)? {
            return prompt_user_agent();
        }
        return Ok(Agent::of(&b));
    }

    ui::warn("no Chromium-based browser was found installed");
    prompt_user_agent()
}

/// Asks for it outright, which is the last resort and also the most explicit
/// thing the user can do — so what comes back is pinned.
fn prompt_user_agent() -> Result<Agent> {
    // Guarded, or `echo "$SESSIONID" | snob login --paste` on a server with no
    // browser installed would feed the piped sessionid in as the User-Agent,
    // and then fail with a message about the User-Agent — never mentioning
    // that the thing it had eaten was the credential.
    if !ui::can_be_asked() {
        bail!(
            "there is no browser installed to take a User-Agent from, and no terminal \
             to ask for one at.\n\
             Give it explicitly, for example:\n\
            \x20   snob login --paste --user-agent \"Mozilla/5.0 (X11; Linux x86_64) \
             AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36\"\n\
             Copy the whole \"User Agent\" line from about:version in the browser your \
             Instagram session is in. The sessionid can still be piped in on standard \
             input."
        );
    }

    eprintln!(
        "\n\
         In the address bar of the browser your session is in, go to:\n\
        \n\
             about:version\n\
        \n\
         and copy the whole \"User Agent\" line. Your browser may show that\n\
         label translated.\n"
    );

    let text = ui::prompt_line("User-Agent: ")?;

    if ui::looks_like_console_dump(&text) {
        bail!(
            "that looks like a dump of the browser console, not a User-Agent.\n\
             Copying from the \"Console\" tab easily picks up dozens of lines, and \
             pasting them makes the terminal try to run each one.\n\
             Use \"about:version\", which shows the value on its own."
        );
    }

    Ok(Agent::pinned(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_engine::seat::{Seat, Ssh, Window};

    fn args(paste: bool, browser: bool, remote: bool) -> LoginArgs {
        LoginArgs {
            paste,
            browser,
            remote,
            user_agent: None,
            csrftoken: None,
            add: false,
            keep_profile: false,
        }
    }

    fn over_ssh(window: Window) -> Seat {
        Seat {
            ssh: Some(Ssh {
                server: Some(("192.168.1.50".to_string(), 22)),
                user: None,
            }),
            window,
        }
    }

    /// A method said outright is the method, wherever the person sits: the
    /// one refusal is a window where none can open.
    #[test]
    fn the_method_said_is_the_method_taken() {
        let seat = over_ssh(Window::Elsewhere);
        let chosen = |a| choose_method_at(&a, &seat).unwrap();
        assert_eq!(chosen(args(true, false, false)), Some(LoginMethod::Paste));
        assert_eq!(chosen(args(false, true, false)), Some(LoginMethod::Browser));
        assert_eq!(chosen(args(false, false, true)), Some(LoginMethod::Remote));

        let nowhere = over_ssh(Window::Nowhere);
        let refused = choose_method_at(&args(false, true, false), &nowhere).unwrap_err();
        let said = format!("{refused:#}");
        assert!(
            said.contains("--remote") && said.contains("--paste"),
            "{said}"
        );
        assert_eq!(
            choose_method_at(&args(false, false, true), &nowhere).unwrap(),
            Some(LoginMethod::Remote)
        );
    }

    /// The instructions give the tunnel, then the link, each on a line of its
    /// own to copy, and say how long the login waits.
    #[tokio::test]
    async fn the_remote_instructions_give_the_tunnel_and_the_link() {
        let temporary = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(temporary.path());
        paths.ensure_dirs().unwrap();
        let relay = snob_engine::relay::Relay::bind(&paths).unwrap();
        let ssh = over_ssh(Window::Nowhere).ssh;
        let said = remote_instructions(&relay, ssh.as_ref());
        let tunnel = format!("    {}", relay.tunnel(ssh.as_ref()));
        let link = format!("    {}", relay.link());
        let lines: Vec<&str> = said.lines().collect();
        let at = |wanted: &str| lines.iter().position(|line| *line == wanted);
        assert!(at(&tunnel).is_some(), "{said}");
        assert!(at(&tunnel) < at(&link), "{said}");
        assert!(said.contains("10 minutes"), "{said}");
        assert!(!said.contains("this-machine"), "{said}");
        assert!(remote_instructions(&relay, None).contains("in place of \"this-machine\""));
    }
}
