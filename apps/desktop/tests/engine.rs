//! The app's backend on the real engine, against the fake Instagram: what a
//! screen is answered when a person signs in, changes account and signs out,
//! and what is written down while it happens.
//!
//! No window, and no browser started: the engine sends from `reqwest` in a
//! sandbox, as the command line's own sandboxed tests do. A browser has to
//! be installed all the same, since a pasted session is sent as one.

#![cfg(feature = "testing")]

use std::sync::{Arc, OnceLock};

use snob_core::Pk;
use snob_core::secret::Secret;
use snob_desktop::backend::engine::Engine;
use snob_desktop::backend::{Account, Backend, SignIn};
use snob_desktop::runtime::Runtime;
use snob_desktop::state::flow::{Found, Update};
use snob_desktop::state::prefs::Prefs;
use snob_engine::login::Failure;
use snob_engine::sandbox::Sandbox;
use snob_engine::{browser, log};
use snob_fake_ig::world::{self, World};
use tempfile::TempDir;

/// The one fake Instagram of this process, which every client is pointed at
/// once: served from a thread of its own, since the app's runtime is not
/// the tests'.
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

/// The log of this process, written as the app writes it: one for every
/// test, since a subscriber is the process's.
fn logged() -> &'static (TempDir, log::Detail, Arc<log::Files>) {
    static LOG: OnceLock<(TempDir, log::Detail, Arc<log::Files>)> = OnceLock::new();
    LOG.get_or_init(|| {
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::util::SubscriberInitExt as _;

        let root = tempfile::tempdir().unwrap();
        let paths = snob_store::paths::AppPaths::rooted_at(root.path());
        let started = log::start(&paths, None).unwrap();
        let files = started.layer.files();
        tracing_subscriber::registry().with(started.layer).init();
        (root, started.detail, files)
    })
}

/// An app's backend in a sandbox of its own, or none, said, where no
/// browser is installed.
fn backend() -> Option<(TempDir, Sandbox, Engine)> {
    world();
    if browser::detect().is_none() {
        let required = snob_engine::headless::env_flag("SNOB_TEST_REQUIRE_BROWSER");
        assert!(!required, "no browser is installed, and one is required");
        eprintln!("no browser is installed; skipping");
        return None;
    }
    let root = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::under(root.path(), None, false).unwrap();
    sandbox.paths.ensure_dirs().unwrap();
    let (_, detail, files) = logged();
    let engine = Engine::new(
        Arc::new(Runtime::start().unwrap()),
        sandbox.paths.clone(),
        sandbox.secrets(),
        (detail.clone(), Arc::clone(files)),
        "snob desktop, in a test".to_string(),
    );
    Some((root, sandbox, engine))
}

/// A `sessionid` of `pk`, told apart from every other test's by `tag`.
fn session_id(pk: u64, tag: &str) -> String {
    format!("{pk}%3A{tag}AbCdEfGh%3A20")
}

fn sign_in(engine: &Engine, session_id: &str) -> SignIn {
    engine
        .sign_in_with(Secret::new(session_id.to_string()), None)
        .blocking_recv()
        .expect("the engine answers")
}

fn signed_in(engine: &Engine, session_id: &str) -> Account {
    match sign_in(engine, session_id) {
        SignIn::Done(account) => account,
        other => panic!("the sign-in ended as {other:?}"),
    }
}

#[test]
fn a_new_computer_has_nobody_and_a_pasted_id_signs_its_account_in() {
    let Some((_root, _sandbox, engine)) = backend() else {
        return;
    };
    let loaded = engine.load().blocking_recv().unwrap();
    assert_eq!(loaded.found, Found::Nobody);
    assert!(loaded.accounts.is_empty());
    assert_eq!(loaded.crash, None);

    let account = signed_in(&engine, &session_id(world::ME, "first"));

    assert_eq!(account.named.pk, Pk::new(world::ME));
    assert_eq!(account.named.username.as_deref(), Some("me"));
    assert_eq!(account.full_name.as_deref(), Some("Invented me"));
    assert_eq!(account.biography.as_deref(), Some("the invented bio of me"));
    assert!(account.active);
    let picture = account.picture.clone().expect("its picture was fetched");
    assert!(std::fs::read(picture).unwrap().starts_with(b"\xff\xd8\xff"));

    // What the next start finds, from files alone.
    let loaded = engine.load().blocking_recv().unwrap();
    assert_eq!(loaded.found, Found::SignedIn(account.named.clone()));
    assert_eq!(loaded.accounts, [account]);
}

#[test]
fn a_session_instagram_rejects_is_a_failure_with_a_code_the_log_holds() {
    let Some((_root, _sandbox, engine)) = backend() else {
        return;
    };
    let dead = session_id(world::ME, "rejected");
    world().refuse(&dead);

    let SignIn::Failed { why, id, until } = sign_in(&engine, &dead) else {
        panic!("a rejected session signs nobody in");
    };

    assert_eq!(why, Failure::Rejected);
    assert_eq!(until, None);
    let (_, _, files) = logged();
    let lines = files.recent(500);
    assert!(
        lines.iter().any(|line| line.contains(&id)),
        "the code {id} is in no line of the log"
    );
    assert_eq!(engine.load().blocking_recv().unwrap().found, Found::Nobody);
}

#[test]
fn what_is_pasted_wrong_is_refused_before_anything_is_sent() {
    let Some((_root, _sandbox, engine)) = backend() else {
        return;
    };
    let SignIn::Failed { why, .. } = sign_in(&engine, "not a session id") else {
        panic!("that is no session");
    };
    assert_eq!(why, Failure::Malformed);
}

#[test]
fn accounts_are_changed_between_and_signed_out_one_at_a_time() {
    let Some((_root, sandbox, engine)) = backend() else {
        return;
    };
    signed_in(&engine, &session_id(world::ME, "two"));
    let someone = signed_in(&engine, &session_id(world::SOMEONE, "two"));
    assert!(someone.active, "the account that signed in last is in use");

    let names = |accounts: &[Account]| -> Vec<(u64, bool)> {
        accounts
            .iter()
            .map(|a| (a.named.pk.get(), a.active))
            .collect()
    };
    let loaded = engine.load().blocking_recv().unwrap();
    assert_eq!(
        names(&loaded.accounts),
        [(world::SOMEONE, true), (world::ME, false)],
        "the one in use first"
    );

    let switched = engine
        .switch_to(Pk::new(world::ME))
        .blocking_recv()
        .unwrap();
    assert_eq!(
        names(&switched),
        [(world::ME, true), (world::SOMEONE, false)]
    );

    // Signing the one in use out leaves the other, and in use.
    let left = engine.sign_out(Pk::new(world::ME)).blocking_recv().unwrap();
    assert_eq!(names(&left), [(world::SOMEONE, true)]);
    let me = sandbox.paths.account(Pk::new(world::ME));
    assert!(!me.viewer_card().exists(), "its card went with its session");
    assert_eq!(
        engine.load().blocking_recv().unwrap().found,
        Found::SignedIn(someone.named.clone())
    );

    let nobody = engine
        .sign_out(Pk::new(world::SOMEONE))
        .blocking_recv()
        .unwrap();
    assert!(nobody.is_empty());
    assert_eq!(engine.load().blocking_recv().unwrap().found, Found::Nobody);
}

/// What happened on a real machine: Instagram pushed back on the login's
/// second request, the one that reads the name, so the session came back
/// with none, and the welcome and the menu named the account by its number
/// until the files were read again. A sign-in in a pause, whose check is
/// skipped, comes back the same way, and is how it is made here.
#[test]
fn a_sign_in_whose_check_read_no_name_is_named_as_the_list_of_accounts_names_it() {
    use snob_core::budget::RateBudget as _;

    let Some((_root, sandbox, engine)) = backend() else {
        return;
    };
    signed_in(&engine, &session_id(world::ME, "named"));
    engine.sign_out(Pk::new(world::ME)).blocking_recv().unwrap();

    let account = sandbox.paths.account(Pk::new(world::ME));
    let _schema = snob_store::store::Store::open(&account).unwrap();
    snob_store::store::rate_budget::SqliteRateBudget::open(&account)
        .unwrap()
        .start_cooldown("rate_limit", std::time::Duration::from_secs(3600))
        .unwrap();

    let again = signed_in(&engine, &session_id(world::ME, "named-again"));

    assert_eq!(again.named.username.as_deref(), Some("me"));
    let loaded = engine.load().blocking_recv().unwrap();
    assert_eq!(
        loaded.accounts.first().map(|account| &account.named),
        Some(&again.named),
        "the same name a start reads from the files"
    );
}

#[test]
fn an_account_whose_session_was_found_dead_is_asked_back_by_name() {
    let Some((_root, sandbox, engine)) = backend() else {
        return;
    };
    let account = signed_in(&engine, &session_id(world::ME, "expires"));
    Prefs::change(&sandbox.paths, |prefs| {
        prefs.expired = Some(Pk::new(world::ME))
    });

    assert_eq!(
        engine.load().blocking_recv().unwrap().found,
        Found::Expired(account.named.clone())
    );

    // Signing in again as it is what clears it.
    signed_in(&engine, &session_id(world::ME, "renewed"));
    assert_eq!(
        engine.load().blocking_recv().unwrap().found,
        Found::SignedIn(account.named)
    );
}

#[test]
fn no_session_id_reaches_the_log_or_the_diagnostic_report() {
    let Some((_root, _sandbox, engine)) = backend() else {
        return;
    };
    let secret = session_id(world::ME, "PrivateTokenZq");
    signed_in(&engine, &secret);
    let dead = session_id(world::SOMEONE, "PrivateTokenZq");
    world().refuse(&dead);
    sign_in(&engine, &dead);

    let (_, _, files) = logged();
    let lines = files.recent(1000);
    assert!(!lines.is_empty(), "signing in is logged");
    let report = engine.diagnostic_report().blocking_recv().unwrap();
    for written in lines.iter().chain([&report]) {
        assert!(!written.contains("PrivateTokenZq"), "{written}");
    }
    // And the report names nobody.
    assert!(
        !report.contains("@me") && !report.contains("@someone"),
        "{report}"
    );
}

#[test]
fn nothing_newer_is_offered_until_the_app_has_a_release() {
    let Some((_root, _sandbox, engine)) = backend() else {
        return;
    };
    assert_eq!(
        engine.check_for_update().blocking_recv().unwrap(),
        Update::None
    );
    assert_eq!(engine.paused_until().blocking_recv().unwrap(), None);
}

#[test]
fn a_session_is_asked_about_once_a_day_and_one_found_dead_is_asked_back() {
    let Some((_root, sandbox, engine)) = backend() else {
        return;
    };
    let id = session_id(world::ME, "dies");
    let account = signed_in(&engine, &id);
    let asked = || engine.expired_session().blocking_recv().unwrap();
    assert_eq!(asked(), None, "it just signed in");

    // Dead from now on, which nobody finds out within the day: nothing is
    // sent to ask.
    world().refuse(&id);
    assert_eq!(asked(), None);
    assert_eq!(
        engine.load().blocking_recv().unwrap().found,
        Found::SignedIn(account.named.clone())
    );

    // A day later it is asked about, and found dead.
    Prefs::change(&sandbox.paths, |prefs| {
        for checked in &mut prefs.checked {
            checked.at = snob_core::Epoch::new(checked.at.get() - 25 * 60 * 60);
        }
    });
    assert_eq!(asked(), Some(account.named.clone()));
    assert_eq!(
        engine.load().blocking_recv().unwrap().found,
        Found::Expired(account.named)
    );
}

#[test]
fn a_pasted_session_is_kept_as_the_browser_it_was_said_to_come_from() {
    let Some((_root, sandbox, engine)) = backend() else {
        return;
    };
    let browsers = engine.browsers().blocking_recv().unwrap();
    assert!(!browsers.found.is_empty(), "one is installed");
    assert_eq!(browsers.last, None, "none was chosen yet");

    // The last one found: with several installed, not the one preferred.
    let chosen = browsers.found.last().unwrap().clone();
    let outcome = engine
        .sign_in_with(
            Secret::new(session_id(world::ME, "browser")),
            Some(chosen.clone()),
        )
        .blocking_recv()
        .unwrap();
    assert!(matches!(outcome, SignIn::Done(_)), "{outcome:?}");

    // What the engine follows as that browser updates is its name, kept
    // with the session.
    let account = sandbox.paths.account(Pk::new(world::ME));
    let session = sandbox
        .secrets()
        .session_of(&account)
        .load()
        .unwrap()
        .expect("a session is stored");
    assert_eq!(session.browser.as_deref(), Some(chosen.as_str()));
    assert!(!session.user_agent_pinned);
    assert_eq!(
        engine.browsers().blocking_recv().unwrap().last,
        Some(chosen),
        "offered first the next time"
    );

    let outcome = engine
        .sign_in_with(
            Secret::new(session_id(world::SOMEONE, "browser")),
            Some("Netscape".to_string()),
        )
        .blocking_recv()
        .unwrap();
    assert!(
        matches!(
            outcome,
            SignIn::Failed {
                why: Failure::NoBrowser,
                ..
            }
        ),
        "a browser that is not installed is none: {outcome:?}"
    );
}
