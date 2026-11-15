//! A login, from a session in hand to an account commands act as, against the
//! fake Instagram: what a program is handed back, and what a login that did
//! not happen leaves behind.
//!
//! No browser is started: the browser logins here are handed their cookies
//! by the test, in place of a window.

#![cfg(feature = "testing")]

use std::sync::OnceLock;

use snob_core::Pk;
use snob_engine::login::{self, Agent, ByBrowser, Check, Failure, Method, Opened};
use snob_engine::sandbox::Sandbox;
use snob_engine::{browser, cdp};
use snob_fake_ig::world::{self, World};
use snob_ig::login::BrowserCookies;
use snob_ig::pace::CancelToken;
use snob_store::registry::Registry;
use tempfile::TempDir;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// The one fake Instagram of this process, which every client is pointed at
/// once: served from a thread of its own, since each test has its own runtime.
fn world() -> &'static World {
    static WORLD: OnceLock<World> = OnceLock::new();
    WORLD.get_or_init(|| {
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the fake Instagram");
            runtime.block_on(async {
                let world = World::new();
                let server = world.serve_rest().await;
                said.send((world, server.uri())).expect("somebody waiting");
                std::future::pending::<()>().await;
            });
        });
        let (world, address) = heard.recv().expect("the fake Instagram started");
        snob_ig::client::point_every_client_at(address.parse().expect("an address"))
            .expect("pointed once");
        world
    })
}

/// A sandbox of a test's own, on the fake Instagram.
fn sandbox() -> (TempDir, Sandbox) {
    world();
    let root = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::under(root.path(), None, false).unwrap();
    sandbox.paths.ensure_dirs().unwrap();
    (root, sandbox)
}

/// A `sessionid` of `pk`, told apart from every other test's by `tag`.
fn sessionid(pk: u64, tag: &str) -> String {
    format!("{pk}%3A{tag}%3A20")
}

async fn paste(sandbox: &Sandbox, sessionid: &str) -> anyhow::Result<login::Stored> {
    login::by_paste(
        sessionid,
        Agent::pinned(UA.to_string()),
        None,
        sandbox.secrets(),
        &sandbox.paths,
        &CancelToken::default(),
    )
    .await
}

fn stored_session(sandbox: &Sandbox, pk: u64) -> Option<snob_core::session::Session> {
    sandbox
        .secrets()
        .session_of(&sandbox.paths.account(Pk::new(pk)))
        .load()
        .unwrap()
}

#[tokio::test]
async fn a_pasted_session_is_checked_stored_and_made_the_account_in_use() {
    let (_root, sandbox) = sandbox();

    let stored = paste(&sandbox, &sessionid(world::ME, "first"))
        .await
        .unwrap();

    assert_eq!(stored.viewer.pk, Pk::new(world::ME));
    assert_eq!(stored.viewer.username.as_deref(), Some("me"));
    assert_eq!(stored.check, Check::Confirmed);
    assert_eq!(stored.accounts, 1);
    assert_eq!(stored.was_active, None);
    assert_eq!(stored.stays_signed_in, None);

    let registry = Registry::load(&sandbox.paths).unwrap();
    assert_eq!(registry.active, Some(Pk::new(world::ME)));
    let kept = stored_session(&sandbox, world::ME).expect("the session is stored");
    assert_eq!(kept.username.as_deref(), Some("me"));
    assert!(kept.validated_at.is_some());
}

#[tokio::test]
async fn a_second_account_says_which_one_commands_acted_as_before() {
    let (_root, sandbox) = sandbox();
    paste(&sandbox, &sessionid(world::ME, "second"))
        .await
        .unwrap();

    let stored = paste(&sandbox, &sessionid(world::SOMEONE, "second"))
        .await
        .unwrap();

    assert_eq!(stored.accounts, 2);
    assert_eq!(
        stored.was_active.map(|was| was.pk),
        Some(Pk::new(world::ME))
    );
    assert_eq!(
        Registry::load(&sandbox.paths).unwrap().active,
        Some(Pk::new(world::SOMEONE))
    );
    assert!(stored_session(&sandbox, world::ME).is_some());
}

#[tokio::test]
async fn a_session_instagram_rejects_is_told_apart_and_leaves_nothing_behind() {
    let (_root, sandbox) = sandbox();
    let dead = sessionid(world::ME, "rejected");
    world().refuse(&dead);

    let error = paste(&sandbox, &dead).await.unwrap_err();

    assert_eq!(login::failure_of(&error), Failure::Rejected, "{error:#}");
    assert!(stored_session(&sandbox, world::ME).is_none());
    assert!(Registry::load(&sandbox.paths).unwrap().accounts.is_empty());
    assert!(
        !sandbox.paths.account(Pk::new(world::ME)).dir().exists(),
        "an account directory made for a rejected session goes"
    );
}

#[tokio::test]
async fn a_rejected_session_leaves_the_one_stored_before_working() {
    let (_root, sandbox) = sandbox();
    let good = sessionid(world::ME, "kept");
    paste(&sandbox, &good).await.unwrap();
    let dead = sessionid(world::ME, "replacement");
    world().refuse(&dead);

    let error = paste(&sandbox, &dead).await.unwrap_err();

    assert_eq!(login::failure_of(&error), Failure::Rejected);
    let kept = stored_session(&sandbox, world::ME).expect("still stored");
    assert_eq!(kept.sessionid.expose(), good);
    assert_eq!(
        Registry::load(&sandbox.paths).unwrap().active,
        Some(Pk::new(world::ME))
    );
}

#[tokio::test]
async fn what_cannot_be_a_sessionid_is_refused_before_anything_is_stored() {
    let (_root, sandbox) = sandbox();

    let error = paste(&sandbox, "not a sessionid").await.unwrap_err();

    assert_eq!(login::failure_of(&error), Failure::Malformed, "{error:#}");
    assert!(Registry::load(&sandbox.paths).unwrap().accounts.is_empty());
}

#[test]
fn every_way_a_browser_login_ends_without_a_session_is_told_apart() {
    let kind = |error: anyhow::Error| login::failure_of(&error);
    assert_eq!(kind(login::NoBrowser.into()), Failure::NoBrowser);
    assert_eq!(kind(cdp::WindowClosed.into()), Failure::WindowClosed);
    assert_eq!(kind(cdp::NobodyLoggedIn.into()), Failure::TimedOut);
    assert_eq!(
        kind(anyhow::Error::new(cdp::WindowClosed).context("while logging in")),
        Failure::WindowClosed,
        "found under what a caller wrapped it in"
    );
    assert_eq!(kind(anyhow::anyhow!("anything else")), Failure::Other);
}

/// The browser a login is told it happened in, or none, said, where no
/// browser is installed.
fn a_browser() -> Option<browser::Browser> {
    let found = browser::detect();
    if found.is_none() {
        let required = snob_engine::headless::env_flag("SNOB_TEST_REQUIRE_BROWSER");
        assert!(!required, "no browser is installed, and one is required");
        eprintln!("no browser is installed; skipping");
    }
    found
}

fn cookies(sessionid: &str) -> BrowserCookies {
    BrowserCookies {
        sessionid: sessionid.into(),
        csrftoken: Some("tok".into()),
        ..BrowserCookies::default()
    }
}

#[tokio::test]
async fn the_profile_a_browser_login_happened_in_becomes_the_accounts() {
    let Some(browser) = a_browser() else { return };
    let (_root, sandbox) = sandbox();
    let session = sessionid(world::ME, "window");

    let stored = login::by_browser(
        ByBrowser {
            browser: &browser,
            user_agent_pinned: false,
            renewing: None,
            method: Method::Browser,
        },
        sandbox.secrets(),
        &sandbox.paths,
        &CancelToken::default(),
        async |opened: Opened<'_>| {
            assert!(
                opened.profile.is_dir(),
                "made before the browser opens on it"
            );
            assert_eq!(opened.kept_at, sandbox.paths.browser_profile());
            std::fs::write(opened.profile.join("made-here"), b"").unwrap();
            Ok((cookies(&session), UA.to_string()))
        },
    )
    .await
    .unwrap()
    .expect("nobody canceled");

    assert_eq!(stored.viewer.pk, Pk::new(world::ME));
    assert_eq!(stored.check, Check::Confirmed);
    let profile = sandbox.paths.browser_profile_for(Pk::new(world::ME));
    assert!(profile.join("made-here").is_file());
    let kept = stored_session(&sandbox, world::ME).expect("the session is stored");
    assert_eq!(kept.browser.as_deref(), Some(browser.name));
    assert!(!kept.user_agent_pinned);
}

#[tokio::test]
async fn a_browser_login_canceled_stores_nothing_and_removes_its_profile() {
    let Some(browser) = a_browser() else { return };
    let (_root, sandbox) = sandbox();
    let cancel = CancelToken::default();
    let mut opened_at = None;

    let stored = login::by_browser(
        ByBrowser {
            browser: &browser,
            user_agent_pinned: false,
            renewing: None,
            method: Method::Browser,
        },
        sandbox.secrets(),
        &sandbox.paths,
        &cancel,
        async |opened: Opened<'_>| {
            opened_at = Some(opened.profile.to_path_buf());
            cancel.cancel();
            Err(anyhow::anyhow!("canceled"))
        },
    )
    .await
    .unwrap();

    assert!(stored.is_none(), "a cancellation is not a failure");
    assert!(!opened_at.expect("the browser was opened").exists());
    assert!(Registry::load(&sandbox.paths).unwrap().accounts.is_empty());
}

#[tokio::test]
async fn a_window_closed_before_a_login_is_an_error_a_program_can_tell() {
    let Some(browser) = a_browser() else { return };
    let (_root, sandbox) = sandbox();
    let mut opened_at = None;

    let error = login::by_browser(
        ByBrowser {
            browser: &browser,
            user_agent_pinned: false,
            renewing: None,
            method: Method::Browser,
        },
        sandbox.secrets(),
        &sandbox.paths,
        &CancelToken::default(),
        async |opened: Opened<'_>| {
            opened_at = Some(opened.profile.to_path_buf());
            Err(cdp::WindowClosed.into())
        },
    )
    .await
    .unwrap_err();

    assert_eq!(login::failure_of(&error), Failure::WindowClosed);
    assert!(!opened_at.expect("the browser was opened").exists());
}
