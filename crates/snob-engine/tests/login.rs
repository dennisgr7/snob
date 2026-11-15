//! A login, from a session in hand to an account commands act as, against the
//! fake Instagram: what a program is handed back, and what a login that did
//! not happen leaves behind. And the sign-out that undoes one.
//!
//! No browser is started: the browser logins here are handed their cookies
//! by the test, in place of a window.

#![cfg(feature = "testing")]

use std::sync::OnceLock;

use snob_core::Pk;
use snob_engine::login::{self, Agent, ByBrowser, Check, Failure, Method, Opened};
use snob_engine::logout::{self, Who};
use snob_engine::sandbox::Sandbox;
use snob_engine::{browser, cdp};
use snob_fake_ig::world::{self, World};
use snob_ig::login::BrowserCookies;
use snob_ig::pace::CancelToken;
use snob_store::registry::Registry;
use snob_store::viewer::Card;
use tempfile::TempDir;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// The one fake Instagram of this process, which every client is pointed at
/// once: served from a thread of its own, since each test has its own runtime.
fn world() -> &'static World {
    &served().0
}

/// The fake Instagram and the server it answers on.
fn served() -> &'static (World, wiremock::MockServer) {
    static WORLD: OnceLock<(World, wiremock::MockServer)> = OnceLock::new();
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
                said.send((world, server)).expect("somebody waiting");
                std::future::pending::<()>().await;
            });
        });
        let (world, server) = heard.recv().expect("the fake Instagram started");
        snob_ig::client::point_every_client_at(server.uri().parse().expect("an address"))
            .expect("pointed once");
        (world, server)
    })
}

/// The paths of the requests sent with `sessionid`, in the order they came.
async fn asked_with(sessionid: &str) -> Vec<String> {
    served()
        .1
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| {
            request
                .headers
                .get("cookie")
                .and_then(|cookie| cookie.to_str().ok())
                .is_some_and(|cookie| cookie.contains(sessionid))
        })
        .map(|request| request.url.path().to_string())
        .collect()
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

/// Two accounts signed in, each with a browser profile of its own.
async fn two_signed_in(sandbox: &Sandbox, tag: &str) {
    for pk in [world::ME, world::SOMEONE] {
        paste(sandbox, &sessionid(pk, tag)).await.unwrap();
        std::fs::create_dir_all(sandbox.paths.browser_profile_for(Pk::new(pk))).unwrap();
    }
}

async fn signed_out(sandbox: &Sandbox, who: &Who) -> logout::Left {
    let (stopping, settled) = snob_engine::jobs::stop_every_job(who.accounts(&sandbox.paths))
        .await
        .unwrap();
    assert!(settled, "there is no job to wait for");
    logout::sign_out(&sandbox.secrets(), &sandbox.paths, who, &stopping).await
}

#[tokio::test]
async fn signing_one_account_out_takes_its_session_and_profile_and_leaves_the_other() {
    let (_root, sandbox) = sandbox();
    two_signed_in(&sandbox, "one-leaves").await;

    let who = Who::of(&sandbox.paths, Some(Pk::new(world::ME)), false);
    assert_eq!(who.leaving, [Pk::new(world::ME)]);
    assert!(!who.sweep, "the other account's profile is kept");
    let left = signed_out(&sandbox, &who).await;

    assert!(left.had_session);
    assert!(left.sessions.is_ok());
    assert!(left.profiles_deleted);
    assert!(left.profile_failure.is_none());
    assert!(stored_session(&sandbox, world::ME).is_none());
    assert!(
        !sandbox
            .paths
            .browser_profile_for(Pk::new(world::ME))
            .exists()
    );
    assert!(stored_session(&sandbox, world::SOMEONE).is_some());
    assert!(
        sandbox
            .paths
            .browser_profile_for(Pk::new(world::SOMEONE))
            .is_dir()
    );
}

#[tokio::test]
async fn signing_out_with_no_account_named_takes_every_one() {
    let (_root, sandbox) = sandbox();
    two_signed_in(&sandbox, "all-leave").await;

    let who = Who::of(&sandbox.paths, None, false);
    assert_eq!(who.leaving.len(), 2);
    assert!(who.sweep);
    let left = signed_out(&sandbox, &who).await;

    assert!(left.had_session && left.sessions.is_ok() && left.profiles_deleted);
    for pk in [world::ME, world::SOMEONE] {
        assert!(stored_session(&sandbox, pk).is_none());
        assert!(!sandbox.paths.browser_profile_for(Pk::new(pk)).exists());
    }
}

#[tokio::test]
async fn signing_out_where_nothing_is_stored_says_there_was_no_session() {
    let (_root, sandbox) = sandbox();

    let left = signed_out(&sandbox, &Who::of(&sandbox.paths, None, true)).await;

    assert!(!left.had_session);
    assert!(left.sessions.is_ok());
    assert!(!left.profiles_deleted);
}

#[tokio::test]
async fn a_login_keeps_what_its_check_showed_of_the_account_and_asks_nothing_more() {
    let (_root, sandbox) = sandbox();
    let session = sessionid(world::ME, "card");
    let account = sandbox.paths.account(Pk::new(world::ME));

    paste(&sandbox, &session).await.unwrap();

    let card = Card::load(&account).expect("a card is kept");
    assert_eq!(card.full_name.as_deref(), Some("Invented me"));
    assert_eq!(card.biography.as_deref(), Some("the invented bio of me"));
    assert!(card.picture.is_some());
    assert_eq!(card.kept_picture, None, "no picture is fetched by a login");
    assert!(!account.viewer_picture().exists());
    assert_eq!(
        asked_with(&session).await,
        [
            format!("/api/v1/friendships/{}/following/", world::ME),
            format!("/api/v1/users/{}/info/", world::ME),
        ],
        "the check and the name: what a login cost before it kept a card"
    );

    // Logging in again, the name known: the check alone, and the card stays.
    let again = sessionid(world::ME, "card-again");
    paste(&sandbox, &again).await.unwrap();
    assert_eq!(asked_with(&again).await.len(), 1);
    assert_eq!(Card::load(&account), Some(card));
}

#[tokio::test]
async fn the_picture_is_fetched_once_and_goes_with_the_session() {
    let (_root, sandbox) = sandbox();
    let session = sessionid(world::ME, "picture");
    let account = sandbox.paths.account(Pk::new(world::ME));
    paste(&sandbox, &session).await.unwrap();
    let client = || {
        snob_ig::client::IgClient::new(
            stored_session(&sandbox, world::ME).unwrap(),
            snob_engine::app::pacer_saying_a_line(&account, CancelToken::default()).unwrap(),
        )
        .unwrap()
    };

    let picture = snob_engine::viewer::picture(&client(), &account)
        .await
        .expect("the picture is kept");
    assert_eq!(picture, account.viewer_picture());
    assert!(
        std::fs::read(&picture)
            .unwrap()
            .starts_with(b"\xff\xd8\xff")
    );
    let fetches = || async {
        asked_with(&session)
            .await
            .iter()
            .filter(|path| path.starts_with("/media/"))
            .count()
    };
    let once = fetches().await;

    assert!(
        snob_engine::viewer::picture(&client(), &account)
            .await
            .is_some()
    );
    assert_eq!(
        fetches().await,
        once,
        "the picture kept is not fetched again"
    );

    let who = Who::of(&sandbox.paths, Some(Pk::new(world::ME)), false);
    signed_out(&sandbox, &who).await;
    assert_eq!(Card::load(&account), None);
    assert!(!account.viewer_picture().exists());
}
