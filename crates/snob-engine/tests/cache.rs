//! The cache a view shows at once (`snob_engine::cache`), against the fake
//! Instagram: it adds no request and takes none away, it is kept by the
//! account that saw it, and a sign-out takes it.

#![cfg(feature = "testing")]

use std::sync::OnceLock;

use snob_core::Pk;
use snob_core::model::ListKind;
use snob_engine::app::App;
use snob_engine::cache;
use snob_engine::engine::{self, ListQuery};
use snob_engine::host::Front;
use snob_engine::login::{self, Agent};
use snob_engine::logout::{self, Who};
use snob_engine::sandbox::Sandbox;
use snob_fake_ig::world::{self, World};
use snob_ig::pace::CancelToken;
use tempfile::TempDir;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// The fake Instagram of this process and the server it answers on, served
/// from a thread of its own, since each test has its own runtime.
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

/// Every request sent with `sessionid`, path and query, in the order they
/// came.
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
        .map(|request| match request.url.query() {
            Some(query) => format!("{}?{query}", request.url.path()),
            None => request.url.path().to_string(),
        })
        .collect()
}

fn sandbox() -> (TempDir, Sandbox) {
    served();
    let root = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::under(root.path(), None, false).unwrap();
    sandbox.paths.ensure_dirs().unwrap();
    (root, sandbox)
}

/// Signs `pk` in with a `sessionid` of its own, told apart by `tag`, and
/// opens a run as it.
async fn signed_in(sandbox: &Sandbox, pk: u64, tag: &str) -> (App, String) {
    let sessionid = format!("{pk}%3A{tag}%3A20");
    login::by_paste(
        &sessionid,
        Agent::pinned(UA.to_string()),
        None,
        sandbox.secrets(),
        &sandbox.paths,
        &CancelToken::default(),
    )
    .await
    .unwrap();
    let account = sandbox.paths.account(Pk::new(pk));
    let app = App::open(
        &sandbox.secrets().session_of(&account),
        &account,
        Front::quiet,
    )
    .unwrap()
    .expect("a session is stored");
    (app, sessionid)
}

/// What `read` sent.
async fn sent_by<F: std::future::Future>(sessionid: &str, read: F) -> Vec<String> {
    let before = asked_with(sessionid).await.len();
    read.await;
    asked_with(sessionid).await.split_off(before)
}

#[tokio::test]
async fn a_profile_read_sends_the_same_requests_with_the_cache_as_without() {
    let (_root, sandbox) = sandbox();
    let (app, session) = signed_in(&sandbox, world::ME, "same-requests").await;
    let me = Pk::new(world::ME);
    let someone = Pk::new(world::SOMEONE);

    let without = sent_by(&session, async {
        app.client().profile_named("someone", None).await.unwrap();
    })
    .await;
    assert_eq!(cache::shown(&sandbox.paths, me, someone), None);
    assert!(!without.is_empty());

    let filling = sent_by(&session, async {
        app.profile_named("someone", None).await.unwrap();
    })
    .await;
    let shown = cache::shown(&sandbox.paths, me, someone).expect("the answer is kept");
    assert_eq!(shown.username.as_deref(), Some("someone"));
    assert_eq!(shown.followers, Some(25));
    assert_eq!(shown.following, Some(11));
    assert!(shown.posts.is_some());

    let filled = sent_by(&session, async {
        // What a view shows first, then the read it makes anyway.
        assert!(cache::shown(&sandbox.paths, me, someone).is_some());
        app.profile_named("someone", None).await.unwrap();
    })
    .await;

    assert_eq!(filling, without, "an empty cache adds and takes no request");
    assert_eq!(filled, without, "a full cache adds and takes no request");
}

#[tokio::test]
async fn a_private_profile_one_account_saw_is_never_shown_to_another() {
    let (_root, sandbox) = sandbox();
    let (app, _) = signed_in(&sandbox, world::ME, "private-me").await;
    let (_, _) = signed_in(&sandbox, world::SOMEONE, "private-someone").await;
    let ghost = Pk::new(world::GHOST);

    app.profile_named("ghost", None).await.unwrap();

    let seen = cache::shown(&sandbox.paths, Pk::new(world::ME), ghost).expect("kept for me");
    assert_eq!(seen.is_private, Some(true));
    assert_eq!(
        cache::shown(&sandbox.paths, Pk::new(world::SOMEONE), ghost),
        None
    );
}

#[tokio::test]
async fn the_counter_poll_keeps_the_accounts_own_counts() {
    let (_root, sandbox) = sandbox();
    let (mut app, _) = signed_in(&sandbox, world::ME, "own-counts").await;
    let me = Pk::new(world::ME);
    assert_eq!(
        cache::shown(&sandbox.paths, me, me).and_then(|own| own.followers),
        None,
        "a login's check counts nothing"
    );

    let query = ListQuery {
        target: None,
        yes: false,
        refresh: false,
        cache: false,
        max_age: std::time::Duration::from_secs(3600),
        no_resume: false,
        max_pages: None,
        over_budget: None,
        walk_at_most_every: None,
    };
    engine::list(&mut app, &query, ListKind::Followers)
        .await
        .unwrap();

    let own = cache::shown(&sandbox.paths, me, me).expect("the poll's answer is kept");
    assert_eq!((own.followers, own.following), (Some(11), Some(10)));
    assert_eq!(
        own.full_name.as_deref(),
        Some("Invented me"),
        "from the card"
    );
}

#[tokio::test]
async fn a_sign_out_takes_the_accounts_cache() {
    let (_root, sandbox) = sandbox();
    let (app, _) = signed_in(&sandbox, world::ME, "sign-out").await;
    app.profile_named("someone", None).await.unwrap();
    let account = sandbox.paths.account(Pk::new(world::ME));
    assert!(account.cache_dir().exists());
    drop(app);

    let who = Who::of(&sandbox.paths, Some(Pk::new(world::ME)), false);
    let (stopping, _) = snob_engine::jobs::stop_every_job(who.accounts(&sandbox.paths))
        .await
        .unwrap();
    logout::sign_out(&sandbox.secrets(), &sandbox.paths, &who, &stopping).await;

    assert!(!account.cache_dir().exists());
    assert_eq!(
        cache::shown(&sandbox.paths, Pk::new(world::ME), Pk::new(world::SOMEONE)),
        None
    );
}
