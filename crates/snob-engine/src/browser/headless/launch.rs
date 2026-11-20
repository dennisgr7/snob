//! Starting a browser without a window, dressed as the one installed is: for
//! an account's requests ([`for_requests`]), or for a login watched from
//! another machine ([`for_a_remote_login`]).
//!
//! **One place gives a browser its identity** ([`dressed`]): the flags, the
//! User-Agent of the build that runs and its client hints on every target,
//! and focus, so a login says of itself what the requests after it will.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use snob_core::Pk;
use snob_core::session::Session;
use snob_ig::client::page::PushedBack;
use snob_ig::pace::CancelToken;
use snob_store::paths::AppPaths;

use super::identity::{machine_hints, metadata};
use super::live::Live;
use super::profile::{self, ProfileMark};
use super::{Account, COMMAND_TIMEOUT, guard, listen};
use crate::browser::Browser;
use crate::browser::cdp::{Cdp, OnAttach};
use crate::system::sync::lock;

/// How much of the app's answers the browser keeps for the listener to read:
/// enough for a page's API calls, which are what it judges, and no more.
const LISTENING_BUFFER: u64 = 1024 * 1024;

/// The screen the browser says it is on: the commonest desktop size.
pub(crate) const SCREEN: (u32, u32) = (1920, 1080);

/// The strip at the bottom of that screen a taskbar keeps for itself.
///
/// A headless screen is all work area, so `screen.availHeight` equaled
/// `screen.height` — measured, 1080 of 1080 — which no desktop with a taskbar
/// or a dock reports. The window fills what is left, from the corner, rather
/// than sitting at (10,10) and running off the bottom edge.
const TASKBAR: u32 = 48;

/// Starts the browser `session`'s requests are sent from, on its account's
/// profile, guarded and listened to, and tells `heard` the first push-back
/// it hears.
pub(super) async fn for_requests(
    paths: &AppPaths,
    heard: &tokio::sync::broadcast::Sender<(Pk, PushedBack)>,
    session: &Session,
    account: &Account,
) -> Result<Live> {
    let (profile, _) = profile::for_account(paths, session.ds_user_id)?;
    let mut mark = ProfileMark::read(&profile);
    // The browser that made the profile, while it is still installed; then
    // the one the session names; then the first found.
    let made_it = mark.browser.as_deref().and_then(|path| {
        crate::browser::detect_all()
            .into_iter()
            .find(|b| b.path == path)
    });
    let browser = made_it
        .or_else(|| {
            session
                .browser
                .as_deref()
                .and_then(crate::browser::detect_named)
        })
        .or_else(crate::browser::detect)
        .ok_or_else(|| {
            anyhow!(
                "snob sends its requests from a browser, and no Chrome, Edge, Brave or \
                 Chromium was found on this machine.\n\
                 Install Chrome, Edge or Brave (or Chromium on Linux), or set \
                 SNOB_NO_BROWSER=1 to send them directly — which Instagram can tell \
                 apart from a browser."
            )
        })?;
    refuse_root()?;
    // A profile nothing has claimed yet, or whose browser is gone: from
    // here on it is this one's.
    let claim = mark.browser.as_deref() != Some(browser.path.as_path());
    mark.browser = Some(browser.path.clone());
    // The User-Agent of the binary being started, never the one stored with
    // the session, pinned at login or not.
    let user_agent = browser.user_agent();

    // What this browser says about the machine, asked of it once without
    // `--user-agent` (which blanks it) and kept. See `machine_hints`.
    let hints = machine_hints(&browser, paths).await;

    let flags = windowless_flags(&user_agent);
    let cancel = CancelToken::default();
    let launched = crate::browser::cdp::launch_headless(&browser, &profile, &flags)
        .with_context(|| format!("could not start {} without a window", browser.name))?;
    let cdp = Cdp::connect(launched, &cancel).await?;
    if claim {
        mark.write(&profile);
    }

    // Created here so the guard can read what the app is seen to send,
    // and which write of snob's to let through. The rule is set before any
    // target is told to pause what it sends (`dressed`).
    let listened = Arc::new(listen::Shared::default());
    let judged = Arc::clone(&listened);
    let armed = Arc::new(guard::Armed::default());
    let expected = Arc::clone(&armed);
    cdp.connection().set_pause_rule(Arc::new(move |params| {
        let documents = lock(&judged.documents);
        guard::judge(params, documents.rediscovered(), &expected)
    }));
    let (
        Tab {
            session: tab,
            target: tab_target,
        },
        _,
    ) = match dressed(&cdp, &browser, hints.as_ref(), true).await {
        Ok(dressed) => dressed,
        Err(e) => {
            // Closed and waited for, not only killed as a drop does, so the
            // start after this one (`start_when_free`) finds the profile
            // let go of.
            if e.is::<NoTab>() {
                cdp.close().await;
            }
            return Err(e);
        }
    };

    // So that a tab that crashes says so, and the request waiting on it
    // fails at once rather than at its timeout: `Inspector.targetCrashed`
    // is only sent to a session that asked for the domain.
    cdp.page_call(&tab, "Inspector.enable", json!({}), COMMAND_TIMEOUT)
        .await?;

    // The calls Instagram's own app makes on this tab are listened to for
    // push-back (`listen`): their answers, with room for a page's worth and
    // no more, and the tab's address as it changes, which is where an
    // in-app route to the challenge shows without a request.
    cdp.page_call(
        &tab,
        "Network.enable",
        json!({
            "maxTotalBufferSize": LISTENING_BUFFER,
            "maxResourceBufferSize": LISTENING_BUFFER / 4,
            "maxPostDataSize": 65_536,
        }),
        COMMAND_TIMEOUT,
    )
    .await?;
    cdp.browser_call(
        "Target.setDiscoverTargets",
        json!({ "discover": true, "filter": [{ "type": "page" }] }),
    )
    .await?;
    *lock(&account.listened) = Some(Arc::clone(&listened));
    // Told on to whoever asked to hear, the moment it happens; ends with
    // the browser, when the latch goes.
    let mut latched = listened.latch.heard();
    let tell = heard.clone();
    let pk = session.ds_user_id;
    tokio::spawn(async move {
        let cause = match latched.wait_for(Option::is_some).await {
            Ok(cause) => cause.clone(),
            Err(_) => return,
        };
        if let Some(cause) = cause {
            let _ = tell.send((pk, cause));
        }
    });
    let account_paths = paths.account(pk);
    tokio::spawn(listen::listen(
        cdp.connection().clone(),
        tab.clone(),
        tab_target,
        Arc::clone(&listened),
        account_paths.clone(),
    ));

    Ok(Live {
        cdp,
        profile,
        tab,
        origin: "about:blank".to_string(),
        world: None,
        page: snob_ig::page_values::PageValues::default(),
        checked: false,
        mark,
        listened,
        armed,
        account: account_paths,
        token_named: None,
        asked_document: None,
        mode: None,
    })
}

/// The tab of the browser behind `cdp`, once the browser is dressed as the
/// one installed is, and the User-Agent it was given: the running build's
/// and its client hints on every target, and focus on the tab. With
/// `guarded`, every target also pauses what it sends for the guard to judge
/// (`guard::patterns`), by the rule its caller set before this.
async fn dressed(
    cdp: &Cdp,
    browser: &Browser,
    hints: Option<&Value>,
    guarded: bool,
) -> Result<(Tab, String)> {
    let (full_version, user_agent) = running_build(cdp, browser).await?;

    // Attached before the plan below is set, so the dispatcher only
    // registers the tab: the calls after the plan are the one place the
    // tab is given its identity.
    let tab = attach_to_a_tab(cdp).await?;

    // **Every target, not only the tab.** A worker, a frame and the service
    // worker the site registers are targets of their own, and
    // `setUserAgentOverride` holds for the one it was sent to; measured, a
    // service worker's requests carried no client hints at all.
    let metadata = metadata(&user_agent, &full_version, hints);
    let identity = json!({ "userAgent": user_agent, "userAgentMetadata": metadata });
    let mut page_commands = vec![
        ("Emulation.setUserAgentOverride", identity.clone()),
        // A headless tab never has focus, and `document.hasFocus()` said
        // so — measured false — where a page somebody is looking at says
        // true.
        (
            "Emulation.setFocusEmulationEnabled",
            json!({ "enabled": true }),
        ),
    ];
    let mut worker_commands = vec![("Network.setUserAgentOverride", identity)];
    if guarded {
        // The guard's pauses, judged by the rule the caller set.
        page_commands.push(("Fetch.enable", guard::patterns()));
        worker_commands.push(("Fetch.enable", guard::patterns()));
    }
    cdp.connection().set_on_attach(OnAttach {
        page: page_commands.clone(),
        worker: worker_commands,
    });
    cdp.browser_call(
        "Target.setAutoAttach",
        json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": true,
            "flatten": true,
            // The service and shared workers, which belong to the browser
            // rather than to a tab. The tab's own are asked for on it.
            "filter": [
                { "type": "service_worker", "exclude": false },
                { "type": "shared_worker", "exclude": false },
                { "exclude": true },
            ],
        }),
    )
    .await?;

    for (method, params) in page_commands {
        cdp.page_call(&tab.session, method, params, COMMAND_TIMEOUT)
            .await?;
    }
    cdp.page_call(
        &tab.session,
        "Target.setAutoAttach",
        json!({ "autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true }),
        COMMAND_TIMEOUT,
    )
    .await?;
    Ok((tab, user_agent))
}

/// Chromium will not run as root with its sandbox on, and snob does not turn
/// the sandbox off for anybody: this browser loads pages and media a server
/// chose, which is what the sandbox is for. Measured: it exits 1 at once, and
/// "exited with code 1" says nothing about why.
pub(crate) fn refuse_root() -> Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: `geteuid` takes nothing, touches no memory and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            anyhow::bail!(
                "snob sends its requests from a browser, and Chromium will not run as root \
                 with its sandbox on.\n\
                 Run snob as an ordinary user, or set SNOB_NO_BROWSER=1 to send the requests \
                 directly — which Instagram can tell apart from a browser."
            );
        }
    }
    Ok(())
}

/// The flags a browser without a window starts with, saying `user_agent`.
fn windowless_flags(user_agent: &str) -> Vec<String> {
    vec![
        "--headless=new".to_string(),
        // Kept although every target is overridden once it attaches: a few requests
        // belong to no target — the script of a service worker is one — and
        // without it they would name `HeadlessChrome`.
        format!("--user-agent={user_agent}"),
        format!(
            "--screen-info={{0,0 {}x{} workAreaBottom={TASKBAR}}}",
            SCREEN.0, SCREEN.1
        ),
        "--window-position=0,0".to_string(),
        format!("--window-size={},{}", SCREEN.0, SCREEN.1 - TASKBAR),
        // A headless browser has no pointer: `(pointer: fine)` and `(hover:
        // hover)` were both false, which is a phone's answer on a desktop's
        // screen.
        "--blink-settings=primaryPointerType=4,availablePointerTypes=4,\
         primaryHoverType=2,availableHoverTypes=2"
            .to_string(),
    ]
}

/// The whole version of the build that runs, and the User-Agent that states
/// it.
async fn running_build(cdp: &Cdp, browser: &Browser) -> Result<(String, String)> {
    let version = cdp.browser_call("Browser.getVersion", json!({})).await?;
    let full_version = version
        .get("product")
        .and_then(Value::as_str)
        .and_then(|product| product.split('/').nth(1))
        .unwrap_or_default()
        .to_string();
    // The build that runs, which is not always the one detection read: an update
    // waiting for the browser to close puts the new version beside the old, and the old
    // one is what starts — and what answered the probe.
    let running = full_version
        .split('.')
        .next()
        .and_then(|major| major.parse::<u32>().ok())
        .unwrap_or(browser.major_version);
    let user_agent = browser.user_agent_at(running);
    Ok((full_version, user_agent))
}

/// A browser without a window, for a login watched from another machine
/// (`crate::relay`), on the tab it opened.
pub struct RemoteLogin {
    pub cdp: Cdp,
    /// The DevTools session of its tab.
    pub tab: String,
    /// The User-Agent its tab sends, which the session is tied to: the
    /// running build's, never the `HeadlessChrome` the browser would report.
    pub user_agent: String,
}

/// Starts the browser a login watched from another machine happens in, on
/// `profile`, dressed as every request's browser is, and on `about:blank`.
///
/// **The same identity as the requests that follow**, from the first request
/// the login page sends: the flags, the client hints, the override on every
/// target, the focus ([`dressed`]).
///
/// **Without the guard** (`guard`): what the person does here is a login, with
/// its own posts — the form, a two-factor code, a challenge — and none of them
/// is a call of snob's to judge.
pub async fn for_a_remote_login(
    browser: &Browser,
    profile: &std::path::Path,
    paths: &AppPaths,
    cancel: &CancelToken,
) -> Result<RemoteLogin> {
    refuse_root()?;
    let hints = machine_hints(browser, paths).await;
    let flags = windowless_flags(&browser.user_agent());
    let launched = crate::browser::cdp::launch_headless(browser, profile, &flags)
        .with_context(|| format!("could not start {} without a window", browser.name))?;
    let cdp = Cdp::connect(launched, cancel).await?;
    let (Tab { session: tab, .. }, user_agent) =
        dressed(&cdp, browser, hints.as_ref(), false).await?;
    Ok(RemoteLogin {
        cdp,
        tab,
        user_agent,
    })
}

/// How long a browser that answers is given to show the tab it opens with.
/// **Assumed, not measured**: nothing orders a windowless browser's answer
/// on its pipe before its tab becomes a target, so the first look may find
/// none. Five seconds is a guess well past that and short beside a start's
/// thirty (`cdp::STARTUP_TIMEOUT`); the owner's log, which names what the
/// browser showed instead ([`NoTab`]), is what will settle it.
pub(super) const TAB_WAIT: Duration = Duration::from_secs(5);

/// How often the browser is asked for its targets during [`TAB_WAIT`], the
/// pace at which `Cdp::connect` asks whether it answers.
const TAB_ASKED_EVERY: Duration = Duration::from_millis(100);

/// A browser that answered but showed no tab within [`TAB_WAIT`], and the
/// kinds of target it showed instead, so a log says what it opened.
#[derive(Debug)]
pub(super) struct NoTab {
    seen: Vec<String>,
    waited: Duration,
}

impl std::fmt::Display for NoTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let seen = if self.seen.is_empty() {
            "none".to_string()
        } else {
            self.seen.join(", ")
        };
        write!(
            f,
            "the browser opened no tab in {} s (its targets: {seen})",
            self.waited.as_secs()
        )
    }
}

impl std::error::Error for NoTab {}

/// The id of the first tab among the browser's `targets` (its answer to
/// `Target.getTargets`), or else every kind of target it named, once each.
fn first_page(targets: &Value) -> Result<String, Vec<String>> {
    let all = targets
        .get("targetInfos")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let page = all
        .iter()
        .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
        .find_map(|t| t.get("targetId").and_then(Value::as_str));
    if let Some(page) = page {
        return Ok(page.to_string());
    }
    let mut seen: Vec<String> = Vec::new();
    for kind in all
        .iter()
        .filter_map(|t| t.get("type").and_then(Value::as_str))
    {
        if !seen.iter().any(|s| s == kind) {
            seen.push(kind.to_string());
        }
    }
    Err(seen)
}

/// The first tab `ask` names, asked again every `every` until `wait` is
/// over, then [`NoTab`]. A call that fails is the answer, not asked again.
async fn wait_for_a_page<F, Fut>(mut ask: F, wait: Duration, every: Duration) -> Result<String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Value>>,
{
    let started = tokio::time::Instant::now();
    let mut said: Option<Vec<String>> = None;
    loop {
        let seen = match first_page(&ask().await?) {
            Ok(page) => return Ok(page),
            Err(seen) => seen,
        };
        let waited = started.elapsed();
        if waited >= wait {
            return Err(NoTab { seen, waited }.into());
        }
        if said.as_ref() != Some(&seen) {
            tracing::debug!(?seen, "no tab yet");
            said = Some(seen);
        }
        tokio::time::sleep(every).await;
    }
}

/// Whether a start that failed with `error` is made once more: one whose
/// browser showed no tab is, once (`relaunched` says it already was).
pub(super) fn start_again(error: &anyhow::Error, relaunched: bool) -> bool {
    !relaunched && error.is::<NoTab>()
}

/// Attaches to the tab the browser opened with, in flat mode so its commands
/// travel on the same pipe, once it shows one ([`TAB_WAIT`]).
pub(super) async fn attach_to_a_tab(cdp: &Cdp) -> Result<Tab> {
    let page = wait_for_a_page(
        || cdp.browser_call("Target.getTargets", json!({})),
        TAB_WAIT,
        TAB_ASKED_EVERY,
    )
    .await?;
    let attached = cdp
        .browser_call(
            "Target.attachToTarget",
            json!({ "targetId": page, "flatten": true }),
        )
        .await?;
    let session = attached
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("the browser would not attach to its tab"))?;
    Ok(Tab {
        session,
        target: page,
    })
}

/// The tab a browser opened with, as this connection knows it.
pub(super) struct Tab {
    /// The DevTools session its commands travel on.
    pub(super) session: String,
    /// The target it is, whose address the browser reports as it changes.
    pub(super) target: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(kinds: &[(&str, &str)]) -> Value {
        let infos: Vec<Value> = kinds
            .iter()
            .map(|(kind, id)| json!({ "type": kind, "targetId": id }))
            .collect();
        json!({ "targetInfos": infos })
    }

    #[test]
    fn the_first_tab_is_picked_and_without_one_every_target_type_is_named() {
        let found = first_page(&targets(&[
            ("service_worker", "w"),
            ("page", "first"),
            ("page", "second"),
        ]));
        assert_eq!(found, Ok("first".to_string()));

        let none = first_page(&targets(&[
            ("service_worker", "w1"),
            ("browser", "b"),
            ("service_worker", "w2"),
        ]));
        assert_eq!(
            none,
            Err(vec!["service_worker".to_string(), "browser".to_string()])
        );

        assert_eq!(first_page(&json!({})), Err(Vec::new()));
        assert_eq!(
            first_page(&json!({ "targetInfos": "not a list" })),
            Err(Vec::new())
        );
        // A page without an id is no tab to attach to.
        assert_eq!(
            first_page(&json!({ "targetInfos": [{ "type": "page" }] })),
            Err(vec!["page".to_string()])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_tab_that_comes_late_is_waited_for() {
        let started = tokio::time::Instant::now();
        let mut asked = 0;
        let page = wait_for_a_page(
            || {
                asked += 1;
                let shown = if asked < 4 {
                    targets(&[("service_worker", "w")])
                } else {
                    targets(&[("page", "late")])
                };
                async move { Ok(shown) }
            },
            TAB_WAIT,
            TAB_ASKED_EVERY,
        )
        .await
        .unwrap();
        assert_eq!(page, "late");
        assert_eq!(asked, 4);
        assert_eq!(started.elapsed(), TAB_ASKED_EVERY * 3);
    }

    #[tokio::test(start_paused = true)]
    async fn a_browser_that_never_opens_a_tab_is_given_up_on_naming_what_it_opened() {
        let started = tokio::time::Instant::now();
        let mut asked = 0u32;
        let error = wait_for_a_page(
            || {
                asked += 1;
                async { Ok(targets(&[("service_worker", "w"), ("browser", "b")])) }
            },
            TAB_WAIT,
            TAB_ASKED_EVERY,
        )
        .await
        .unwrap_err();
        assert!(error.is::<NoTab>(), "{error:#}");
        assert_eq!(
            error.to_string(),
            "the browser opened no tab in 5 s (its targets: service_worker, browser)"
        );
        assert_eq!(started.elapsed(), TAB_WAIT);
        // Asked at once, then every tenth of a second up to the wait's end.
        assert_eq!(asked, 51);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_call_for_the_targets_is_not_asked_again() {
        let started = tokio::time::Instant::now();
        let mut asked = 0;
        let error = wait_for_a_page(
            || {
                asked += 1;
                async { Err(anyhow!("the browser stopped answering (Target.getTargets)")) }
            },
            TAB_WAIT,
            TAB_ASKED_EVERY,
        )
        .await
        .unwrap_err();
        assert!(!error.is::<NoTab>(), "{error:#}");
        assert_eq!(asked, 1);
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[test]
    fn a_browser_that_shows_no_tab_is_started_once_more_and_no_more() {
        let no_tab = || {
            anyhow::Error::from(NoTab {
                seen: vec!["service_worker".to_string()],
                waited: TAB_WAIT,
            })
            .context("could not dress the tab")
        };
        assert!(start_again(&no_tab(), false));
        assert!(!start_again(&no_tab(), true));
        let other = anyhow!("could not start Chrome without a window");
        assert!(!start_again(&other, false));
    }
}
