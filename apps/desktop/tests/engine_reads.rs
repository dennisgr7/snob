//! What a screen reads past what a start shows, on the real engine against
//! the fake Instagram: a tab read on a page at a time from the cursor the
//! engine handed out, a list served from what was stored, the names the
//! account already knows, and a list's row pictures from the CDN.
//!
//! **Every request leaves from a tab with no browser behind it**
//! (`snob_fake_ig::tab`), as a profile's posts are only read from a page:
//! a process of its own, since the page every client sends from is the
//! process's. A browser has to be installed all the same, since a pasted
//! session is sent as one. A page made to fail answers 500 once, from the
//! fake's own server (`fail_once`).

#![cfg(feature = "testing")]

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use snob_core::Pk;
use snob_core::secret::Secret;
use snob_core::session::Session;
use snob_desktop::backend::engine::Engine;
use snob_desktop::backend::{Backend, Listed, SignIn, Tiles, Unread, Walked};
use snob_desktop::runtime::Runtime;
use snob_desktop::state::flow::Named;
use snob_desktop::state::profile::{Side, Tab};
use snob_engine::sandbox::Sandbox;
use snob_engine::{browser, log};
use snob_fake_ig::tab::TestTab;
use snob_fake_ig::world::{self, World};
use snob_ig::client::page::Page;
use snob_ig::pace::CancelToken;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot, watch};

/// The fake Instagram of this process, and what its server is asked.
struct Fake {
    world: World,
    asking: mpsc::UnboundedSender<Ask>,
}

/// What the fake's server is asked from a test.
enum Ask {
    /// How many requests it received so far.
    Count(oneshot::Sender<usize>),
    /// The next query of this name answers 500, once.
    FailOnce(&'static str, oneshot::Sender<()>),
}

/// The one fake Instagram of this process, served from a thread of its
/// own, which every client is pointed at and sends to from a tab of its
/// own, once.
fn fake() -> &'static Fake {
    static FAKE: OnceLock<Fake> = OnceLock::new();
    FAKE.get_or_init(|| {
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime for the fake Instagram");
            runtime.block_on(async {
                let world = World::new();
                let server = world.serve_web().await;
                let (asking, mut asked) = mpsc::unbounded_channel::<Ask>();
                said.send((world, server.uri(), asking))
                    .expect("somebody waiting");
                while let Some(ask) = asked.recv().await {
                    match ask {
                        Ask::Count(answer) => {
                            let received = server.received_requests().await.unwrap_or_default();
                            let _ = answer.send(received.len());
                        }
                        Ask::FailOnce(query, done) => {
                            use wiremock::matchers::{body_string_contains, method, path_regex};
                            wiremock::Mock::given(method("POST"))
                                .and(path_regex(r"^/(api/graphql|graphql/query)$"))
                                .and(body_string_contains(query))
                                .respond_with(wiremock::ResponseTemplate::new(500))
                                .with_priority(1)
                                .up_to_n_times(1)
                                .mount(&server)
                                .await;
                            let _ = done.send(());
                        }
                    }
                }
            });
        });
        let (world, address, asking) = heard.recv().expect("the fake Instagram started");
        snob_ig::client::point_every_client_at(address.parse().expect("an address"))
            .expect("pointed once");
        snob_ig::client::page::use_the_page_off_instagram();
        let tabs_of = world.clone();
        let factory: snob_ig::client::page::PageFactory = Arc::new(move |session: &Session| {
            Arc::new(TestTab::new(&tabs_of, session)) as Arc<dyn Page>
        });
        assert!(
            snob_ig::client::page::send_every_request_from(factory).is_ok(),
            "the first page set"
        );
        Fake { world, asking }
    })
}

/// How many requests the fake Instagram received so far.
fn requests() -> usize {
    let (answer, answered) = oneshot::channel();
    fake()
        .asking
        .send(Ask::Count(answer))
        .expect("the fake is serving");
    answered.blocking_recv().expect("the fake counted")
}

/// The next query named `query` the fake receives answers 500, once.
fn fail_once(query: &'static str) {
    let (done, mounted) = oneshot::channel();
    fake()
        .asking
        .send(Ask::FailOnce(query, done))
        .expect("the fake is serving");
    mounted.blocking_recv().expect("the failure mounted");
}

/// The log of this process: one for every test, since a subscriber is the
/// process's.
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

/// An app's backend in a sandbox of its own, signed in as `me` with a
/// session told apart by `tag`, or none, said, where no browser is
/// installed. **One test at a time holds it**, so the requests the fake
/// received while it is held are that test's alone.
fn signed_in_as_me(tag: &str) -> Option<(MutexGuard<'static, ()>, TempDir, Sandbox, Engine)> {
    static ALONE: Mutex<()> = Mutex::new(());
    let alone = ALONE.lock().unwrap_or_else(PoisonError::into_inner);
    fake();
    if browser::detect().is_none() {
        let required = snob_engine::system::env::flag("SNOB_TEST_REQUIRE_BROWSER");
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
    sign_in(&engine, &session_id(tag));
    Some((alone, root, sandbox, engine))
}

/// A `sessionid` of `me`, told apart from every other test's by `tag`.
fn session_id(tag: &str) -> String {
    format!("{}%3A{tag}AbCdEfGh%3A20", world::ME)
}

fn sign_in(engine: &Engine, session_id: &str) {
    let outcome = engine
        .sign_in_with(Secret::new(session_id.to_string()), None)
        .blocking_recv()
        .expect("the engine answers");
    assert!(matches!(outcome, SignIn::Done(_)), "{outcome:?}");
}

fn named(pk: u64, username: &str) -> Named {
    Named {
        pk: Pk::new(pk),
        username: Some(username.to_string()),
    }
}

fn read_on(engine: &Engine, who: &Named, tiles: &Tiles) -> Result<Tiles, Unread> {
    read_on_in(engine, who, Tab::Posts, tiles)
}

fn read_on_in(engine: &Engine, who: &Named, tab: Tab, tiles: &Tiles) -> Result<Tiles, Unread> {
    let (resting, _) = watch::channel(None);
    engine
        .more_tiles(
            who.clone(),
            tab,
            tiles.next.expect("a cursor handed out"),
            resting,
            CancelToken::default(),
        )
        .blocking_recv()
        .expect("the engine answers")
}

fn walked(engine: &Engine, owner: &Named, side: Side) -> Listed {
    let (walked, _) = watch::channel(Walked::default());
    engine
        .list(owner.clone(), side, walked, CancelToken::default())
        .blocking_recv()
        .expect("the engine answers")
        .expect("the list is read")
}

#[test]
fn a_tabs_next_page_comes_from_the_cursor_the_page_before_handed_out() {
    let Some((_alone, _root, _sandbox, engine)) = signed_in_as_me("pages") else {
        return;
    };
    let who = named(4_001, "many_posts");
    fake().world.person(4_001, |person| {
        person.username = "many_posts".into();
        person.posts = 30;
    });

    let first = engine
        .tiles(who.clone(), Tab::Posts)
        .blocking_recv()
        .unwrap()
        .expect("the first page is read");
    assert_eq!(first.tiles.len(), 12);
    assert!(first.more && first.next.is_some());

    let second = read_on(&engine, &who, &first).expect("the second page");
    assert_eq!(second.tiles.len(), 12);
    let codes = |tiles: &Tiles| -> Vec<String> {
        tiles.tiles.iter().map(|tile| tile.code.clone()).collect()
    };
    assert!(
        codes(&second)
            .iter()
            .all(|code| !codes(&first).contains(code)),
        "the posts after the first page's"
    );

    let third = read_on(&engine, &who, &second).expect("the third page");
    assert_eq!(third.tiles.len(), 6);
    assert_eq!(third.next, None, "the grid has ended");
    assert!(
        read_on(&engine, &who, &first).is_err(),
        "a cursor is spent by the page it led to"
    );
}

#[test]
fn a_page_that_failed_is_asked_again_from_the_same_cursor_without_opening_the_tab_again() {
    let Some((_alone, _root, _sandbox, engine)) = signed_in_as_me("failed") else {
        return;
    };
    let who = named(4_002, "tagged_again");
    fake().world.person(4_002, |person| {
        person.username = "tagged_again".into();
        person.tab_posts = 20;
    });
    let first = engine
        .tiles(who.clone(), Tab::Tagged)
        .blocking_recv()
        .unwrap()
        .expect("the first page is read");
    let opened_before = fake().world.navigations().len();

    // The app is navigated to the tab, and the page after it fails.
    fail_once("PolarisProfileTaggedTabContentQuery_connection");
    assert!(read_on_in(&engine, &who, Tab::Tagged, &first).is_err());
    let second = read_on_in(&engine, &who, Tab::Tagged, &first)
        .expect("read from where the first page ended");
    assert!(!second.tiles.is_empty());

    let opened: Vec<String> = fake().world.navigations()[opened_before..]
        .iter()
        .filter(|route| route.ends_with("/tagged/"))
        .cloned()
        .collect();
    assert_eq!(opened.len(), 1, "the tab is opened once: {opened:?}");
}

#[test]
fn a_cursor_handed_out_before_signing_in_again_reads_nothing() {
    let Some((_alone, _root, _sandbox, engine)) = signed_in_as_me("again") else {
        return;
    };
    let who = named(4_003, "posts_again");
    fake().world.person(4_003, |person| {
        person.username = "posts_again".into();
        person.posts = 20;
    });
    let first = engine
        .tiles(who.clone(), Tab::Posts)
        .blocking_recv()
        .unwrap()
        .expect("the first page is read");

    // The session dies between the two pages, and a new one signs in: its
    // browser was never navigated where the old one was.
    fake().world.refuse(&session_id("again"));
    assert!(read_on(&engine, &who, &first).is_err());
    sign_in(&engine, &session_id("again-renewed"));

    let before = requests();
    assert_eq!(
        read_on(&engine, &who, &first),
        Err(Unread::Failed(String::new())),
        "let go of by the sign-in"
    );
    assert_eq!(requests(), before, "nothing was sent");
    let read_again = engine
        .tiles(who.clone(), Tab::Posts)
        .blocking_recv()
        .unwrap()
        .expect("the first page again");
    let second = read_on(&engine, &who, &read_again).expect("the second page");
    assert_eq!(second.tiles.len(), 8);
    assert_eq!(second.next, None);
}

#[test]
fn a_list_read_before_is_served_from_storage_with_nothing_sent() {
    let Some((_alone, _root, _sandbox, engine)) = signed_in_as_me("kept") else {
        return;
    };
    let me = named(world::ME, "me");
    let someone = named(world::SOMEONE, "someone");
    assert_eq!(
        engine
            .list_kept(me.clone(), Side::Followers)
            .blocking_recv()
            .unwrap(),
        None,
        "nothing is stored before a walk"
    );
    let read = walked(&engine, &me, Side::Followers);
    let theirs = walked(&engine, &someone, Side::Following);

    let before = requests();
    let kept = engine
        .list_kept(me.clone(), Side::Followers)
        .blocking_recv()
        .unwrap()
        .expect("the walk was stored");
    let kept_theirs = engine
        .list_kept(someone.clone(), Side::Following)
        .blocking_recv()
        .unwrap()
        .expect("another account's walk was stored");
    assert_eq!(requests(), before, "nothing was sent");

    let pks = |listed: &Listed| -> Vec<Pk> {
        let mut pks: Vec<Pk> = listed.people.iter().map(|p| p.named.pk).collect();
        pks.sort();
        pks
    };
    assert_eq!(pks(&kept), pks(&read));
    assert_eq!(pks(&kept_theirs), pks(&theirs));
    assert!(kept.whole);
    assert_eq!(kept.taken_at, read.taken_at, "when the walk ended");
    assert!(
        kept.people
            .iter()
            .all(|person| person.picture_from.is_some()),
        "each row's address on the CDN, as its list answered it"
    );
    assert_eq!(
        engine
            .list_kept(someone, Side::Followers)
            .blocking_recv()
            .unwrap(),
        None,
        "a list never walked"
    );
}

#[test]
fn the_accounts_already_known_come_back_for_their_first_letters_with_nothing_sent() {
    let Some((_alone, _root, _sandbox, engine)) = signed_in_as_me("known") else {
        return;
    };
    walked(&engine, &named(world::ME, "me"), Side::Following);

    let before = requests();
    let known = |typed: &str, limit: usize| -> Vec<String> {
        engine
            .known_accounts(typed.to_string(), limit)
            .blocking_recv()
            .unwrap()
            .into_iter()
            .filter_map(|account| account.named.username)
            .collect()
    };
    let users = known("USER", 50);
    assert_eq!(users.len(), 10, "{users:?}");
    assert!(users.iter().all(|name| name.starts_with("user")));
    assert_eq!(known("@user", 3).len(), 3);
    assert!(known("nobody_here", 6).is_empty());
    assert!(known("", 6).is_empty());
    assert_eq!(requests(), before, "nothing was sent");
}

#[test]
fn a_rows_picture_comes_from_the_cdn_into_the_cache_once() {
    let Some((_alone, _root, sandbox, engine)) = signed_in_as_me("pictures") else {
        return;
    };
    let listed = walked(&engine, &named(world::ME, "me"), Side::Following);
    let wanted: Vec<(Pk, String)> = listed
        .people
        .iter()
        .filter(|person| person.picture.is_none())
        .filter_map(|person| Some((person.named.pk, person.picture_from.clone()?)))
        .take(4)
        .collect();
    assert_eq!(wanted.len(), 4, "rows with an address and no picture kept");

    let fetch = || {
        let (came, mut heard) = mpsc::unbounded_channel();
        engine
            .pictures(wanted.clone(), came, CancelToken::default())
            .blocking_recv()
            .expect("the engine answers");
        std::iter::from_fn(|| heard.try_recv().ok()).collect::<Vec<_>>()
    };
    let first = fetch();
    assert_eq!(first.len(), 4);
    let cache = sandbox.paths.account(Pk::new(world::ME)).cache_dir();
    for (_, picture, downloaded) in &first {
        assert!(downloaded);
        assert!(picture.starts_with(&cache), "{}", picture.display());
    }

    let before = requests();
    let again = fetch();
    assert_eq!(again.len(), 4);
    assert!(again.iter().all(|(_, _, downloaded)| !downloaded));
    assert_eq!(requests(), before, "a picture kept is not downloaded again");
}
