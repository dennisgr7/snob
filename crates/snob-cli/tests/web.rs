//! The browser path's commands, driven through a tab with no browser behind
//! it (`common::tab`) on the fake Instagram of `common::ig`, and the fake
//! itself held to what it claims to check.
//!
//! The tab builds every call with the same `snob_ig::web::build_call` the
//! headless tab runs, so what reaches the fake here is what a browser would
//! send, headers the browser adds aside. `tests/headless.rs` sends through
//! the browser itself.
#![cfg(feature = "testing")]

use std::sync::Arc;

use snob_core::Pk;
use snob_core::session::{Session, SessionOrigin};
use snob_ig::allowlist::Operation;
use snob_ig::client::page::{Page, PageRequest};
use snob_ig::page_values::PageValues;
use snob_ig::web::{Ask, Call, Told};
use wiremock::MockServer;

mod common;

use common::ig::{self, World};
use common::tab::TestTab;

/// The session of [`common::SID`], account 42.
fn session() -> Session {
    Session::from_sessionid(common::SID, common::UA, SessionOrigin::Paste).unwrap()
}

/// An intent for the tab, as a client of `server` would hand it over.
fn call(server: &MockServer, ask: Ask, referrer: &str) -> Call {
    Call {
        ask,
        origin: server.uri(),
        referrer: referrer.into(),
        claim: "0".into(),
        cap: 1 << 20,
        timeout_ms: 20_000,
    }
}

fn hover_card(pk: u64) -> Ask {
    Ask::Query {
        operation: Operation::HoverCard,
        variables: format!(r#"{{"userID":"{pk}"}}"#),
    }
}

/// `request` with its form's `name` made `value`.
fn with_field(request: &PageRequest, name: &str, value: &str) -> PageRequest {
    let form: Vec<(String, String)> =
        url::form_urlencoded::parse(request.body.as_deref().unwrap_or_default().as_bytes())
            .into_owned()
            .map(|(n, v)| {
                if n == name {
                    (n, value.to_string())
                } else {
                    (n, v)
                }
            })
            .collect();
    PageRequest {
        body: Some(
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(form)
                .finish(),
        ),
        ..request.clone()
    }
}

/// The oracle takes a form built from the app's calls, and refuses one
/// with a bitmap the app never sent, a `__req` already used on the
/// document, or the pk where the fbid goes.
#[tokio::test]
async fn the_oracle_refuses_a_form_the_app_would_not_send() {
    let world = World::new();
    let server = world.serve_web().await;
    let tab = TestTab::new(&world, &session());

    let built = tab
        .build(&call(&server, hover_card(ig::SOMEONE), "/"))
        .unwrap();
    let sent = tab.send(built.clone()).await.unwrap();
    assert_eq!(sent.status, 200, "{}", sent.body);

    for (request, reason) in [
        (built.clone(), "__req"),
        (with_field(&built, "__dyn", "madeUp"), "__dyn"),
        (with_field(&built, "av", "42"), "av"),
    ] {
        let refused = tab.send(request).await.unwrap();
        assert_eq!(refused.status, 400, "{reason}: {}", refused.body);
        assert!(refused.body.contains(reason), "{reason}: {}", refused.body);
    }
}

/// Every value a call is built from is read off the world's documents, for
/// any account, and a session the world refuses gets a logged-out one.
#[test]
fn the_worlds_documents_carry_every_page_value() {
    let world = World::new();
    for pk in [42, 43] {
        let values = PageValues::parse(&world.document(Some(pk), ""));
        let viewer = values.viewer.expect("the viewer");
        assert_eq!(viewer.pk, Pk::new(pk));
        assert_eq!(viewer.fbid, ig::fbid(pk));
        assert_eq!(viewer.username, if pk == 42 { "me" } else { "user43" });
        assert!(values.site.is_some(), "{pk}");
        assert_eq!(values.lsd.unwrap().expose(), ig::lsd(pk));
        assert_eq!(values.relay_dtsg.unwrap().expose(), ig::relay_token(pk));
        assert_eq!(values.session_dtsg.unwrap().expose(), ig::session_token(pk));
        assert_eq!(values.bloks_version, Some(ig::bloks()));
        assert!(!values.signed_out);
    }
    world.refuse("42%3Agone%3A1");
    let viewer = world.viewer_of("42%3Agone%3A1");
    let values = PageValues::parse(&world.document(viewer, ""));
    assert!(values.signed_out);
    assert!(values.viewer.is_none());
}

/// The audit holds snob's requests to the allowlist and to the paths a test
/// retired, and leaves the app's alone.
#[tokio::test]
async fn the_audit_flags_a_retired_read_and_a_call_the_allowlist_refuses() {
    let world = World::new();
    let server = world.serve_web().await;
    let tab = TestTab::new(&world, &session());
    let get = |path: &str| PageRequest {
        method: snob_ig::client::page::Method::Get,
        url: format!("{}{path}", server.uri()),
        headers: Vec::new(),
        referrer: format!("{}/", server.uri()),
        body: None,
        navigate: false,
        cap: 1 << 20,
        timeout_ms: 20_000,
    };
    tab.send(get("/api/v1/users/42/info/")).await.unwrap();
    tab.send(get("/api/v1/friendships/42/following/?count=12"))
        .await
        .unwrap();
    let hover = tab.build(&call(&server, hover_card(42), "/")).unwrap();
    tab.send(with_field(
        &hover,
        "fb_api_req_friendly_name",
        ig::NOT_IN_THE_REGISTRY,
    ))
    .await
    .unwrap();

    let found = world.audit(&server, &["/api/v1/users/*/info/"]).await;
    assert_eq!(
        found,
        [
            "refused: GET /api/v1/users/42/info/",
            "retired: GET /api/v1/users/42/info/",
            "refused: POST /api/graphql"
        ]
    );
}

/// A `no-cors` GET, as a browser loads a document's icon by itself, is left
/// out of the audit; a `no-cors` POST, the shape of a beacon, is judged.
#[tokio::test]
async fn the_audit_leaves_out_a_no_cors_get_and_judges_a_no_cors_post() {
    let world = World::new();
    let server = world.serve_web().await;
    let http = reqwest::Client::new();
    http.get(format!("{}/favicon.ico", server.uri()))
        .header("sec-fetch-mode", "no-cors")
        .send()
        .await
        .unwrap();
    http.post(format!("{}/ajax/bz", server.uri()))
        .header("sec-fetch-mode", "no-cors")
        .header("content-type", "text/plain")
        .body("invented")
        .send()
        .await
        .unwrap();
    http.post(format!("{}/media/invented.jpg", server.uri()))
        .header("sec-fetch-mode", "no-cors")
        .send()
        .await
        .unwrap();

    let found = world.audit(&server, &[]).await;
    assert_eq!(
        found,
        [
            "refused: POST /ajax/bz",
            "refused: POST /media/invented.jpg"
        ]
    );
}

/// A query built from the document's boot calls is answered.
#[tokio::test]
async fn the_test_tab_answers_a_query_built_from_the_apps_calls() {
    let world = World::new();
    let server = world.serve_web().await;
    let tab = TestTab::new(&world, &session());
    let told = tab
        .ask(call(&server, hover_card(ig::SOMEONE), "/"))
        .await
        .unwrap();
    let Told::Answer(answer) = told else {
        panic!("a query is answered: {told:?}");
    };
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(
        answer.body.contains(r#""username":"someone""#),
        "{}",
        answer.body
    );
    assert!(world.audit(&server, &[]).await.is_empty());
}

/// The claim the first page of a list hands out is sent back with the
/// next one.
#[tokio::test]
async fn the_second_list_page_carries_the_claim_the_first_handed_out() {
    let world = World::new();
    let server = world.serve_web().await;
    let client = common::client(&server).with_page(Arc::new(TestTab::new(&world, &session())));
    let first = client
        .friendships_page(
            Pk::new(ig::SOMEONE),
            "someone",
            snob_ig::client::Direction::Followers,
            12,
            None,
        )
        .await
        .unwrap();
    assert_eq!(first.users.len(), 12);
    let cursor = first.next_cursor().expect("a second page").to_string();
    client
        .friendships_page(
            Pk::new(ig::SOMEONE),
            "someone",
            snob_ig::client::Direction::Followers,
            12,
            Some(&cursor),
        )
        .await
        .unwrap();
    assert_eq!(world.later_claims(), [ig::CLAIM]);
}

/// A client asking and sending through a tab on `world`, as the session of
/// [`common::SID`] stored under `stored`.
fn client_as(
    server: &MockServer,
    world: &World,
    stored: Option<&str>,
) -> snob_ig::client::IgClient {
    let mut session = session();
    session.username = stored.map(str::to_string);
    let tab = Arc::new(TestTab::new(world, &session));
    snob_ig::client::IgClient::new(session, snob_ig::pace::Pacer::unlimited())
        .unwrap()
        .with_base_url(url::Url::parse(&server.uri()).unwrap())
        .with_page(tab)
}

/// `whoami` reads the account off the tab's document, and spends nothing.
#[tokio::test]
async fn whoami_spends_nothing_and_says_the_worlds_name() {
    let world = World::new();
    let server = world.serve_web().await;
    let client = client_as(&server, &world, None);
    let identity = client.whoami().await.unwrap();
    assert_eq!(identity.pk, Pk::new(42));
    assert_eq!(identity.username.as_deref(), Some("me"));
    client.validate().await.unwrap();
    assert_eq!(client.pacer().spent(), 0);
    assert!(
        common::requests(&server).await == 0,
        "nothing reached the fake"
    );
}

/// A renamed account is told by its new name, whatever the session stored.
#[tokio::test]
async fn a_renamed_viewer_is_told_by_its_new_name() {
    let world = World::new();
    let server = world.serve_web().await;
    world.person(ig::ME, |me| me.username = "me.renamed".into());
    let client = client_as(&server, &world, Some("me"));
    let identity = client.whoami().await.unwrap();
    assert_eq!(identity.username.as_deref(), Some("me.renamed"));
}

/// A session Instagram serves a logged-out document to has expired, and
/// says so with the code of one.
#[tokio::test]
async fn a_logged_out_document_is_an_expired_session() {
    let world = World::new();
    let server = world.serve_web().await;
    world.refuse(common::SID);
    let client = client_as(&server, &world, Some("me"));
    let error = client.validate().await.unwrap_err();
    assert!(
        matches!(error, snob_ig::error::IgError::SessionExpired),
        "{error:?}"
    );
    let code = snob_engine::exit::from_ig_error(&error);
    assert_eq!(snob_engine::exit::code(code), 3);
    assert_eq!(common::requests(&server).await, 0, "nothing was sent");
}

/// A name is not looked up on a logged-out document, by the route
/// definitions or by its profile: the session has expired, and nothing is
/// sent.
#[tokio::test]
async fn a_logged_out_document_looks_up_no_name() {
    let world = World::new();
    let server = world.serve_web().await;
    world.refuse(common::SID);
    let client = client_as(&server, &world, Some("me"));
    let error = client.profile_named("someone", None).await.unwrap_err();
    assert!(
        matches!(error, snob_ig::error::IgError::SessionExpired),
        "{error:?}"
    );
    assert_eq!(common::requests(&server).await, 0, "nothing was sent");
}

/// A read with no account named, on a session that never stored its own
/// name, takes the name off the tab's document for nothing.
#[tokio::test]
async fn the_own_account_is_named_from_the_document_when_the_session_is_not() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = common::app_web(
        &server,
        &world,
        common::open_db(tmp.path()),
        Arc::new(snob_core::budget::UnlimitedRateBudget),
        snob_engine::app::Viewer {
            pk: common::ME,
            username: None,
        },
    );
    let own = snob_cli::commands::common::target_or_own(&app, None)
        .await
        .unwrap();
    assert_eq!(own, "me");
    assert_eq!(app.client().pacer().spent(), 0);
}

/// An `App` asking through a new tab on `world`, over the database under
/// `root`: what a second run on the same machine is.
fn app_on(server: &MockServer, world: &World, root: &std::path::Path) -> snob_engine::app::App {
    common::app_web(
        server,
        world,
        common::open_db(root),
        Arc::new(snob_core::budget::UnlimitedRateBudget),
        snob_engine::app::Viewer {
            pk: common::ME,
            username: Some("me".into()),
        },
    )
}

/// The paths snob sent the fake since the first `since` of its requests.
async fn sent_since(server: &MockServer, since: usize) -> Vec<String> {
    server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r))
        .map(|r| r.url.path().to_string())
        .collect()
}

fn query_for(target: &str) -> snob_engine::engine::ListQuery {
    snob_engine::engine::ListQuery::from(&common::args_for(target))
}

/// A private account the viewer does not follow is refused before a page
/// is walked, and one the viewer asked to follow says the request is
/// pending.
#[tokio::test]
async fn a_private_account_is_refused_before_its_lists_are_walked() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let error = snob_engine::engine::target::resolve(&mut app, &query_for("ghost"))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("private account you do not follow"),
        "{error}"
    );

    world.person(ig::ME, |me| me.requested.push(ig::GHOST));
    let error = snob_engine::engine::target::resolve(&mut app, &query_for("ghost"))
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("has not been accepted yet"),
        "{error}"
    );
    let sent = sent_since(&server, 0).await;
    assert!(
        !sent.iter().any(|p| p.contains("/friendships/")),
        "{sent:?}"
    );
    assert!(world.audit(&server, &[]).await.is_empty());
}

/// The own account's pk is the session's, whether it is named or not: its
/// profile is read by pk, with no lookup of the name.
#[tokio::test]
async fn the_own_profile_is_read_by_the_sessions_pk() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    for (target, typed) in [(None, "me"), (Some("ME"), "ME"), (Some("@me"), "@me")] {
        let known = snob_engine::engine::target::known_pk_of(&app, target, typed).unwrap();
        assert_eq!(known, Some(common::ME), "{target:?}");
    }
    assert_eq!(
        snob_engine::engine::target::known_pk(&app, "someone").unwrap(),
        None
    );

    let profile = app
        .client()
        .profile_named("me", Some(common::ME))
        .await
        .unwrap();
    assert_eq!(profile.id, common::ME);
    assert_eq!(
        app.client().pacer().spent(),
        4,
        "the profile, opened as the app opens one"
    );
    assert_eq!(
        sent_since(&server, 0).await,
        ["/api/graphql"; 4],
        "no route definitions and no profile document"
    );
    assert_eq!(relay_since(&server, 0).await, OPENED);
}

/// The first run on a name finds its pk through the route definitions and
/// then reads the profile; a later run on the same database knows the pk,
/// and reads the profile alone.
#[tokio::test]
async fn a_name_is_looked_up_once_and_its_profile_read_by_pk_after() {
    use snob_core::model::ListKind;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();

    let mut app = app_on(&server, &world, tmp.path());
    snob_engine::engine::list(&mut app, &query_for("someone"), ListKind::Followers)
        .await
        .unwrap();
    assert_eq!(
        sent_since(&server, 0).await,
        [
            "/ajax/bulk-route-definitions/",
            "/api/graphql",
            "/api/graphql",
            "/api/graphql",
            "/api/graphql",
            "/ajax/navigation/",
            "/api/v1/friendships/9001/followers/",
            "/api/v1/friendships/9001/followers/",
            "/api/v1/friendships/9001/followers/"
        ]
    );
    assert_eq!(world.navigations(), ["/someone/"]);

    let since = common::requests(&server).await;
    let mut app = app_on(&server, &world, tmp.path());
    snob_engine::engine::list(&mut app, &query_for("someone"), ListKind::Followers)
        .await
        .unwrap();
    assert_eq!(sent_since(&server, since).await, ["/api/graphql"; 4]);
    let found = world
        .audit(&server, &["/api/v1/users/web_profile_info/"])
        .await;
    assert!(found.is_empty(), "{found:?}");
}

/// `watch check` reads each account through its profile, by pk, with its
/// counters; the session is checked for nothing.
#[tokio::test]
async fn watch_check_reads_each_account_by_its_profile() {
    use snob_engine::watch::check::{CheckReport, Verdict, What, with_a_session};
    use snob_engine::watch::{Consent, Watched};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let secrets = common::secrets(&common::account_at(tmp.path()), "web-check");
    let mut report = CheckReport::default();
    let watched = [
        Watched::own(),
        Watched::consented("someone".into(), Consent),
    ];
    with_a_session(&app, &secrets, &watched, &mut report).await;
    // The first run is still to lay the baselines, which is a warning.
    assert_eq!(report.verdict(), Verdict::Warned, "{:?}", report.checked);
    let someone = report
        .checked
        .iter()
        .find_map(|c| match &c.what {
            What::Account {
                target: Some(t),
                pk,
                followers,
                ..
            } if t == "someone" => Some((*pk, *followers)),
            _ => None,
        })
        .expect("someone was checked");
    assert_eq!(someone, (Some(Pk::new(ig::SOMEONE)), Some(25)));
    assert!(
        !sent_since(&server, 0)
            .await
            .iter()
            .any(|p| p.starts_with("/api/v1/users/") || p.contains("following/")),
        "the session was checked for nothing"
    );
}

/// `watch check` on the own account reads its profile by the session's
/// pk, opened as the app opens one (four reads, no route definitions), on a
/// database that has never walked it.
#[tokio::test]
async fn watch_check_reads_the_own_account_as_its_profile_opens() {
    use snob_engine::watch::Watched;
    use snob_engine::watch::check::{CheckReport, What, with_a_session};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let secrets = common::secrets(&common::account_at(tmp.path()), "web-check-own");
    let mut report = CheckReport::default();
    with_a_session(&app, &secrets, &[Watched::own()], &mut report).await;
    let own = report
        .checked
        .iter()
        .find_map(|c| match &c.what {
            What::Account {
                target: None, pk, ..
            } => Some(*pk),
            _ => None,
        })
        .expect("the own account was checked");
    assert_eq!(own, Some(Pk::new(ig::ME)), "{:?}", report.checked);
    assert_eq!(relay_since(&server, 0).await, OPENED);
}

/// The profile query carries the app's own variables with only the id
/// replaced once the tab has seen the app send one, and before that the
/// ones the capture of 2026-11-13 shows it sending: the integrity filter,
/// the id and seven flags of its own.
#[tokio::test]
async fn the_profile_query_copies_the_apps_variables_once_it_has_seen_them() {
    let world = World::new();
    let server = world.serve_web().await;
    let tab = Arc::new(TestTab::new(&world, &session()));
    let client = common::client(&server).with_page(tab.clone());

    client.profile_named("someone", None).await.unwrap();
    tab.ask(call(
        &server,
        Ask::Document {
            path: "/someone/".into(),
        },
        "/",
    ))
    .await
    .unwrap();
    client.profile_named("user0", None).await.unwrap();
    assert_eq!(
        world.profile_queries(),
        [
            concat!(
                r#"{"enable_integrity_filters":true,"id":"9001","#,
                r#""__relay_internal__pv__PolarisCannesGuardianExperienceEnabledrelayprovider":true,"#,
                r#""__relay_internal__pv__PolarisCASB976ProfileEnabledrelayprovider":false,"#,
                r#""__relay_internal__pv__PolarisProfileInformTreatmentBannerEnabledrelayprovider":false,"#,
                r#""__relay_internal__pv__PolarisWebSchoolsEnabledrelayprovider":false,"#,
                r#""__relay_internal__pv__PolarisProfileRingsGradientEnabledrelayprovider":true,"#,
                r#""__relay_internal__pv__PolarisRepostsConsumptionEnabledrelayprovider":true,"#,
                r#""__relay_internal__pv__PolarisShortDramaEnabledrelayprovider":true}"#
            )
            .to_string(),
            r#"{"id":"1000","a_flag_the_app_sends":true}"#.to_string(),
        ]
    );
}

/// A run of the monitor on somebody else's account that finds nothing moved
/// costs its profile, opened as the app opens one (four reads), by the pk
/// the first run learned, which cost the route definitions besides.
#[tokio::test]
async fn watch_once_on_a_known_third_party_costs_its_profile() {
    use snob_engine::watch::{self, Consent, Watched};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = Watched::consented("someone".into(), Consent);

    let mut app = app_on(&server, &world, tmp.path());
    let first = watch::tick(&mut app, &watched).await.unwrap();
    watch::commit(&mut app, &first, None).unwrap();
    // The walks that lay the baseline come after.
    let sent = sent_since(&server, 0).await;
    assert_eq!(
        sent[..2],
        ["/ajax/bulk-route-definitions/", "/api/graphql"],
        "{sent:?}"
    );

    let mut app = app_on(&server, &world, tmp.path());
    let second = watch::tick(&mut app, &watched).await.unwrap();
    assert_eq!(
        second.requests, 4,
        "its profile, opened as the app opens one"
    );
}

/// What opening a profile sends: the profile query and the three reads the
/// app sends beside it, in order.
const OPENED: [&str; 4] = [
    "PolarisProfilePageContentQuery",
    "PolarisProfileStoryHighlightsTrayContentQuery",
    "PolarisProfileNoteBubbleQuery",
    "PolarisSchoolPartnerProfileBadgeQuery",
];

/// The Relay operations snob sent the fake since the first `since` of its
/// requests, by the name each form carries.
async fn relay_since(server: &MockServer, since: usize) -> Vec<String> {
    server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r) && r.url.path() == "/api/graphql")
        .filter_map(|r| {
            url::form_urlencoded::parse(&r.body)
                .find(|(n, _)| n == "fb_api_req_friendly_name")
                .map(|(_, v)| v.into_owned())
        })
        .collect()
}

/// The own account's followers are polled with its hover card, by pk: one
/// query, whose count is what the store records as declared.
#[tokio::test]
async fn the_own_followers_poll_the_hover_card_and_record_its_count() {
    use snob_core::model::ListKind;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let (users, _) = snob_engine::engine::list(
        &mut app,
        &snob_engine::engine::ListQuery::from(&common::args()),
        ListKind::Followers,
    )
    .await
    .unwrap();
    assert_eq!(users.len(), 11);
    assert_eq!(
        sent_since(&server, 0).await,
        [
            "/api/graphql",
            "/ajax/navigation/",
            "/api/v1/friendships/42/followers/"
        ]
    );
    assert_eq!(world.navigations(), ["/me/"]);
    assert_eq!(
        relay_since(&server, 0).await,
        [Operation::HoverCard.friendly_name()]
    );
    let own = snob_store::store::accounts::find(app.db().conn(), common::ME)
        .unwrap()
        .expect("the own account is stored");
    assert_eq!(own.counter(ListKind::Followers), Some(11));
    assert!(world.audit(&server, &[]).await.is_empty());
}

/// A crossing of the own lists, as `unfollowers` makes it, polls the
/// counters once per list: the hover card that opens the first list is
/// good until its walk begins, and the second list, read after that walk,
/// asks again.
#[tokio::test]
async fn unfollowers_polls_the_counters_once_per_list() {
    use snob_core::model::ListKind;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let query = snob_engine::engine::ListQuery::from(&common::args());
    for kind in [ListKind::Followers, ListKind::Following] {
        snob_engine::engine::list(&mut app, &query, kind)
            .await
            .unwrap();
    }
    assert_eq!(
        relay_since(&server, 0).await,
        [
            Operation::HoverCard.friendly_name(),
            Operation::HoverCard.friendly_name()
        ]
    );
    assert_eq!(app.client().pacer().actions(), 2, "one walk per list");
}

/// Somebody else's counters come with the profile that named the account:
/// no hover card, and no request besides.
#[tokio::test]
async fn a_third_partys_counters_come_with_its_profile() {
    use snob_core::model::ListKind;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    snob_engine::engine::list(&mut app, &query_for("someone"), ListKind::Following)
        .await
        .unwrap();
    assert_eq!(relay_since(&server, 0).await, OPENED);
    let someone = snob_store::store::accounts::find(app.db().conn(), Pk::new(ig::SOMEONE))
        .unwrap()
        .expect("someone is stored");
    assert_eq!(someone.counter(ListKind::Following), Some(11));
}

/// A run of the monitor on the own account that finds nothing moved costs
/// one request: the hover card.
#[tokio::test]
async fn watch_once_on_the_own_account_costs_one_request() {
    use snob_engine::watch::{self, Watched};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = Watched::own();

    let mut app = app_on(&server, &world, tmp.path());
    let first = watch::tick(&mut app, &watched).await.unwrap();
    watch::commit(&mut app, &first, None).unwrap();

    let since = common::requests(&server).await;
    let mut app = app_on(&server, &world, tmp.path());
    let second = watch::tick(&mut app, &watched).await.unwrap();
    assert_eq!(second.requests, 1);
    assert_eq!(
        relay_since(&server, since).await,
        [Operation::HoverCard.friendly_name()]
    );
}

/// Each tick on one App asks for the counters again, even when the tick
/// before walked nothing: a tick is an action, and counters read in an
/// earlier one are not reused (`Pacer::begin_action`, `App::resolved_target`).
#[tokio::test]
async fn every_tick_on_one_app_reads_the_counters_anew() {
    use snob_engine::watch::{self, Watched};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = Watched::own();

    let mut app = app_on(&server, &world, tmp.path());
    let baseline = watch::tick(&mut app, &watched).await.unwrap();
    watch::commit(&mut app, &baseline, None).unwrap();

    for tick in ["second", "third"] {
        let since = common::requests(&server).await;
        let report = watch::tick(&mut app, &watched).await.unwrap();
        watch::commit(&mut app, &report, None).unwrap();
        assert_eq!(report.requests, 1, "the {tick} tick");
        assert_eq!(
            relay_since(&server, since).await,
            [Operation::HoverCard.friendly_name()],
            "the {tick} tick polls the hover card"
        );
    }
}

/// A tab whose app has made none of the calls a query is built from ends
/// the run with exit 1: nothing is sent, no cooldown is written and no mark
/// moves, and the next run tries again.
#[tokio::test]
async fn a_page_not_ready_ends_the_tick_and_the_next_one_tries_again() {
    use snob_core::budget::RateBudget;
    use snob_engine::watch::{self, Watched};
    use snob_store::store::watch::all_marks;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let (paths, budget) = common::budget_at(tmp.path());
    let app_of = |world: &World| {
        common::app_web(
            &server,
            world,
            snob_store::store::Store::open(&paths).unwrap(),
            budget.clone(),
            snob_engine::app::Viewer {
                pk: common::ME,
                username: Some("me".into()),
            },
        )
    };
    let watched = Watched::own();
    let mut app = app_of(&world);
    let first = watch::tick(&mut app, &watched).await.unwrap();
    watch::commit(&mut app, &first, None).unwrap();
    let marks = all_marks(app.db().conn()).unwrap();
    assert_eq!(marks.len(), 2, "{marks:?}");

    world.app_calls_missing(true);
    let since = common::requests(&server).await;
    let mut app = app_of(&world);
    let error = watch::tick(&mut app, &watched).await.unwrap_err();
    assert_eq!(
        snob_engine::exit::code(snob_engine::exit::exit_code_for(&error)),
        1,
        "{error:#}"
    );
    assert!(error.to_string().contains("nothing was sent"), "{error:#}");
    assert!(sent_since(&server, since).await.is_empty());
    assert_eq!(budget.cooldown().unwrap(), None);
    assert_eq!(all_marks(app.db().conn()).unwrap(), marks);

    world.app_calls_missing(false);
    let mut app = app_of(&world);
    let next = watch::tick(&mut app, &watched).await.unwrap();
    assert_eq!(next.requests, 1);
}

/// A tab served a logged-out document ends the run as an expired session.
#[tokio::test]
async fn a_logged_out_document_ends_the_tick_with_exit_3() {
    use snob_engine::watch::{self, Watched};

    let world = World::new();
    let server = world.serve_web().await;
    world.refuse(common::SID);
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let error = watch::tick(&mut app, &Watched::own()).await.unwrap_err();
    assert_eq!(
        snob_engine::exit::code(snob_engine::exit::exit_code_for(&error)),
        3,
        "{error:#}"
    );
    assert_eq!(common::requests(&server).await, 0, "nothing was sent");
}

/// A list of thirty is walked in three pages of twelve, each asked as the
/// app asks it from the account's page; what a walk of it again is said to
/// cost counts the same twelve a request.
#[tokio::test]
async fn a_list_of_thirty_walks_in_three_pages_of_twelve() {
    use snob_core::model::ListKind;

    let world = World::new();
    let server = world.serve_web().await;
    for n in 25..30 {
        world.person(ig::FIRST_USER + n, |p| p.following.push(ig::SOMEONE));
    }
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let (users, _) =
        snob_engine::engine::list(&mut app, &query_for("someone"), ListKind::Followers)
            .await
            .unwrap();
    assert_eq!(users.len(), 30);

    let pages: Vec<(String, String)> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| !ig::is_the_apps(r) && r.url.path().ends_with("/followers/"))
        .map(|r| {
            (
                r.url.query().unwrap_or_default().to_string(),
                ig::header(r, "referer").unwrap_or_default().to_string(),
            )
        })
        .collect();
    let referer = format!("{}/someone/", server.uri());
    assert_eq!(
        pages,
        [
            ("count=12&search_surface=follow_list_page", &referer),
            (
                "count=12&max_id=12&search_surface=follow_list_page",
                &referer
            ),
            (
                "count=12&max_id=24&search_surface=follow_list_page",
                &referer
            ),
        ]
        .map(|(q, r)| (q.to_string(), r.clone()))
    );
    assert!(world.audit(&server, &[]).await.is_empty());

    // Three pages, as the walk above asked for them: the navigation, and
    // each page with its `show_many`.
    assert_eq!(snob_engine::engine::walk::pages_to_walk(30), 3);
    assert_eq!(
        snob_engine::engine::walk::requests_to_walk(30, true),
        1 + 2 * 3
    );
    let cost = snob_cli::commands::common::rewalk_cost(
        &app,
        Pk::new(ig::SOMEONE),
        &[(ListKind::Followers, 30)],
    )
    .unwrap();
    assert!(app.client().has_page());
    assert_eq!(
        cost,
        4 + 7,
        "the account found by its profile, then the list"
    );
}

/// An account that follows `me`, on top of the ten the world starts with:
/// `user15` to `user29`, twenty-six followers in all, three pages.
fn more_followers_of_me(world: &World) {
    for n in 15..30 {
        world.person(ig::FIRST_USER + n, |p| p.following.push(ig::ME));
    }
}

/// A list Instagram serves with more than one row in twenty twice is not
/// believed complete: the command that prints it exits 1.
#[tokio::test]
async fn a_list_served_with_repeats_is_truncated_and_exits_1() {
    use snob_core::model::{ListKind, StopReason};

    let world = World::new();
    let server = world.serve_web().await;
    more_followers_of_me(&world);
    world.repeat_every(Some(4));
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let (_, outcome) = snob_engine::engine::list(
        &mut app,
        &snob_engine::engine::ListQuery::from(&common::args()),
        ListKind::Followers,
    )
    .await
    .unwrap();
    assert_eq!(outcome.reason, StopReason::Truncated);
    assert_eq!(
        snob_engine::exit::code(outcome.exit_code_for_a_printed_result()),
        1
    );
}

/// The same list refuses a crossing, on the path without a browser too:
/// `unfollowers` walks the followers it crosses against first, and says
/// the answer would be wrong rather than give it.
#[tokio::test]
async fn a_crossing_refuses_a_list_served_with_repeats() {
    let world = World::new();
    let server = world.serve_web().await;
    more_followers_of_me(&world);
    world.repeat_every(Some(4));
    let tmp = tempfile::tempdir().unwrap();
    let snob = |args: &[&str], typed: Option<&str>| {
        use std::io::Write;

        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_snob"))
            .arg("--sandbox-root")
            .arg(tmp.path())
            .arg("--ig-base-url")
            .arg(server.uri())
            .args(args)
            .env("NO_COLOR", "1")
            .env_remove("SNOB_ACCOUNT")
            .env_remove("SNOB_IGNORE_COOLDOWN")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("the binary runs");
        if let Some(typed) = typed {
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(typed.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        child.wait_with_output().expect("the binary finishes")
    };
    let said = |out: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    };

    let out = snob(
        &["login", "--paste", "--user-agent", common::UA],
        Some(&format!("{}\n", common::SID)),
    );
    assert!(out.status.success(), "{}", said(&out));
    let out = snob(&["unfollowers", "--format", "json"], None);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert!(
        said(&out).contains("followers list could not be read in full"),
        "{}",
        said(&out)
    );
    assert!(
        said(&out).contains("more than one in twenty"),
        "{}",
        said(&out)
    );
    let following = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| r.url.path() == "/api/v1/friendships/42/following/")
        .filter(|r| r.url.query() != Some("count=1"))
        .count();
    assert_eq!(following, 0, "the second list is never walked");
}

/// The Relay operations snob sent the fake since the first `since` of its
/// requests, on either Relay endpoint, by the name each form carries.
async fn queries_since(server: &MockServer, since: usize) -> Vec<String> {
    server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r))
        .filter(|r| r.url.path() == "/api/graphql" || r.url.path() == "/graphql/query")
        .filter_map(|r| {
            url::form_urlencoded::parse(&r.body)
                .find(|(n, _)| n == "fb_api_req_friendly_name")
                .map(|(_, v)| v.into_owned())
        })
        .collect()
}

/// The highlights tray is the web client's tray query, whose entries know
/// no count and no dates; a highlight's items are its highlights query,
/// over the whole tray, and `reels_media` is never asked.
#[tokio::test]
async fn highlights_are_read_through_the_tray_and_highlights_queries() {
    use snob_engine::highlights::{self, Fetched};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let Fetched::Tray(tray) = highlights::tray(app.client(), "someone", common::ME, None)
        .await
        .unwrap()
    else {
        panic!("someone's tray is shown");
    };
    let titles: Vec<&str> = tray.entries.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Trips", "Food"]);
    for entry in &tray.entries {
        assert_eq!(entry.declared_items, None, "{entry:?}");
        assert_eq!(entry.created_at, None, "{entry:?}");
        assert_eq!(entry.updated_at, None, "{entry:?}");
    }
    assert_eq!(
        queries_since(&server, 0).await,
        OPENED,
        "the tray the profile opened with, and no second one"
    );

    let since = common::requests(&server).await;
    let items = highlights::items(app.client(), &tray, &tray.entries[0])
        .await
        .unwrap();
    let urls: Vec<String> = items.iter().filter_map(|s| s.url.clone()).collect();
    assert_eq!(
        urls,
        ["3100000000000000001", "3100000000000000002"]
            .map(|pk| format!("{}/media/{pk}.jpg", server.uri()))
    );
    assert!(items.iter().all(|s| s.expiring_at.is_none()));
    assert_eq!(
        queries_since(&server, since).await,
        [Operation::HighlightsPage.friendly_name()]
    );
    let window = server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .find(|r| r.url.path() == "/graphql/query")
        .map(|r| {
            url::form_urlencoded::parse(&r.body)
                .find(|(n, _)| n == "variables")
                .map(|(_, v)| v.into_owned())
                .unwrap_or_default()
        })
        .expect("the highlights query was sent");
    assert_eq!(
        window,
        r#"{"initial_reel_id":"highlight:17900000000000001","reel_ids":["highlight:17900000000000001","highlight:17900000000000002"],"first":3,"last":2,"__relay_internal__pv__PolarisCommunityNoteStoriesLabelEnabledrelayprovider":true}"#
    );

    let found = world
        .audit(
            &server,
            &[
                "/api/v1/highlights/*/highlights_tray/",
                "/api/v1/feed/reels_media/",
                "/api/v1/users/web_profile_info/",
            ],
        )
        .await;
    assert!(found.is_empty(), "{found:?}");
}

/// An account's stories are the web client's gallery, opened over the
/// stories tray the home page shows: the tray's reels in its order when the
/// account is in it, the account alone when it is not. The tray is read
/// for nothing, and `reels_media` is never asked.
#[tokio::test]
async fn stories_are_read_through_the_gallery_in_the_trays_order() {
    let world = World::new();
    let server = world.serve_web().await;
    let third = ig::FIRST_USER + 3;
    world.person(third, |p| {
        p.stories = vec![ig::Story {
            pk: "3000000000000000103".into(),
            taken_at: 1_790_000_300,
            video: false,
        }];
    });
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());

    let gallery = |since: usize| {
        let server = &server;
        async move {
            server.received_requests().await.unwrap_or_default()[since..]
                .iter()
                .filter(|r| r.url.path() == "/graphql/query")
                .map(|r| {
                    url::form_urlencoded::parse(&r.body)
                        .find(|(n, _)| n == "variables")
                        .map(|(_, v)| v.into_owned())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
        }
    };

    let stories = snob_engine::stories::named(app.client(), "user3", None)
        .await
        .unwrap();
    assert_eq!(stories.username, "user3");
    assert_eq!(stories.items.len(), 1);
    let (first, third) = (ig::FIRST_USER, third);
    assert_eq!(
        gallery(0).await,
        [format!(
            r#"{{"initial_reel_id":"{third}","reel_ids":["{first}","{third}"],"first":3,"last":2,"__relay_internal__pv__PolarisCommunityNoteStoriesLabelEnabledrelayprovider":true}}"#
        )]
    );

    let since = common::requests(&server).await;
    let stories = snob_engine::stories::named(app.client(), "someone", None)
        .await
        .unwrap();
    assert_eq!(stories.items.len(), 2);
    assert!(stories.items.iter().all(|s| s.expiring_at.is_some()));
    assert_eq!(
        gallery(since).await,
        [
            r#"{"initial_reel_id":"9001","reel_ids":["9001"],"first":3,"last":2,"__relay_internal__pv__PolarisCommunityNoteStoriesLabelEnabledrelayprovider":true}"#
        ]
    );
    let sent = queries_since(&server, since).await;
    assert_eq!(sent[..4], OPENED);
    assert_eq!(sent[4..], [Operation::ReelGallery.friendly_name()]);

    let found = world
        .audit(
            &server,
            &[
                "/api/v1/feed/reels_media/",
                "/api/v1/users/web_profile_info/",
            ],
        )
        .await;
    assert!(found.is_empty(), "{found:?}");
}

/// A profile whose newest story is none asks for no reel; one with a story
/// up asks the gallery once.
#[tokio::test]
async fn a_profile_with_no_story_up_sends_no_gallery_query() {
    use snob_cli::commands::profile::{MutualPolicy, Reads, fetch};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());

    let profile = fetch(
        app.client(),
        "user1",
        common::ME,
        Reads::all(MutualPolicy::Defer),
        None,
    )
    .await
    .unwrap();
    assert_eq!(profile.username, "user1");
    let sent = queries_since(&server, 0).await;
    assert_eq!(
        sent, OPENED,
        "the tray the profile opened with, and no second one"
    );

    let since = common::requests(&server).await;
    fetch(
        app.client(),
        "someone",
        common::ME,
        Reads::all(MutualPolicy::Defer),
        None,
    )
    .await
    .unwrap();
    let sent = queries_since(&server, since).await;
    assert_eq!(sent[..4], OPENED);
    assert_eq!(sent[4..], [Operation::ReelGallery.friendly_name()]);
    assert!(world.audit(&server, &[]).await.is_empty());
}

/// The profile picture is the full size the profile query carries, fetched
/// from its address with no lookup by id; an account that keeps the default
/// avatar has none, and nothing is downloaded.
#[tokio::test]
async fn pfp_downloads_the_full_size_the_profile_query_carries() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());

    snob_cli::commands::pfp::fetch(app.client(), "someone", None)
        .await
        .unwrap();
    let media: Vec<String> = sent_since(&server, 0)
        .await
        .into_iter()
        .filter(|p| p.starts_with("/media/"))
        .collect();
    assert_eq!(media, ["/media/9001-full.jpg"]);
    assert_eq!(
        app.client().pacer().spent(),
        5,
        "the name, then the profile, opened as the app opens one"
    );

    let since = common::requests(&server).await;
    let error = snob_cli::commands::pfp::fetch(app.client(), "user29", None)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("@user29 has no profile picture"),
        "{error}"
    );
    let sent = sent_since(&server, since).await;
    assert!(!sent.iter().any(|p| p.starts_with("/media/")), "{sent:?}");

    let found = world
        .audit(
            &server,
            &["/api/v1/users/*/info/", "/api/v1/users/web_profile_info/"],
        )
        .await;
    assert!(found.is_empty(), "{found:?}");
}

/// A profile query that names no full-size picture leaves the smaller one,
/// with no lookup by id: the page does not make one, and no request is
/// paid for it.
#[tokio::test]
async fn pfp_without_a_full_size_address_takes_the_smaller_picture() {
    let world = World::new();
    let server = world.serve_web().await;
    world.person(ig::SOMEONE, |someone| someone.full_size_picture = false);
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());

    snob_cli::commands::pfp::fetch(app.client(), "someone", None)
        .await
        .unwrap();
    let media: Vec<String> = sent_since(&server, 0)
        .await
        .into_iter()
        .filter(|p| p.starts_with("/media/"))
        .collect();
    assert_eq!(media, ["/media/9001-small.jpg"]);
    assert_eq!(
        app.client().pacer().spent(),
        5,
        "the name, then the profile, opened as the app opens one"
    );
    let found = world.audit(&server, &["/api/v1/users/*/info/"]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// The `doc_id`s a run knows: none, so each write goes under its seed.
struct Seeds;

impl snob_ig::graphql::DocIds for Seeds {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn put(&self, _: &str, _: &str) {}
}

/// A follow and an unfollow through the tab: each loads the profile, one
/// read, and sends the write the tab builds beside it in the app's form,
/// which the oracle takes; the relationship is the write's answer, and
/// nothing is read after it.
#[tokio::test]
async fn a_follow_and_an_unfollow_go_out_in_the_apps_form() {
    let world = World::new();
    let server = world.serve_web().await;
    let client = client_as(&server, &world, Some("me"));
    let someone = Pk::new(ig::SOMEONE);

    let followed = client.follow(someone, "someone", &Seeds).await.unwrap();
    assert!(followed.following, "{followed:?}");
    assert!(world.account(ig::ME).following.contains(&ig::SOMEONE));
    let unfollowed = client.unfollow(someone, "someone", &Seeds).await.unwrap();
    assert!(!unfollowed.following, "{unfollowed:?}");
    assert!(!world.account(ig::ME).following.contains(&ig::SOMEONE));
    assert_eq!(client.pacer().spent(), 4, "a profile and a write each");

    let received = server.received_requests().await.unwrap_or_default();
    let sent: Vec<(String, Option<String>)> = received
        .iter()
        .map(|r| {
            (
                r.url.path().to_string(),
                ig::header(r, "x-fb-friendly-name").map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        sent,
        [
            (
                "/api/graphql".to_string(),
                Some("usePolarisFollowMutation".to_string())
            ),
            (
                "/api/graphql".to_string(),
                Some("usePolarisUnfollowMutation".to_string())
            ),
        ],
        "a write each, and nothing read after"
    );
    let doc_ids: Vec<Option<String>> = received
        .iter()
        .map(|r| {
            url::form_urlencoded::parse(&r.body)
                .find(|(n, _)| n == "doc_id")
                .map(|(_, v)| v.into_owned())
        })
        .collect();
    assert_eq!(
        doc_ids,
        [
            Some("26508036048874888".to_string()),
            Some("27789106940691111".to_string())
        ],
        "each under its seed, as the run knows no other"
    );
    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// A write is built only on the profile its command loaded, while the tab
/// is still on it. Asked before any profile, or once another page was
/// loaded in between, as a second command on the account loads one, it is
/// not built and nothing is sent; with the profile loaded again, it goes.
#[tokio::test]
async fn a_write_is_built_only_on_the_profile_it_is_made_from() {
    use snob_ig::client::page::PageError;
    use snob_ig::graphql::{self, Mutation};

    let world = World::new();
    let server = world.serve_web().await;
    let tab = TestTab::new(&world, &session());
    let follow = || {
        call(
            &server,
            Ask::Write {
                mutation: Mutation::Follow,
                variables: format!(
                    r#"{{"target_user_id":"{}","container_module":"profile"}}"#,
                    ig::SOMEONE
                ),
                doc_id: Mutation::Follow.seed_doc_id().into(),
                route: graphql::PROFILE_ROUTE.into(),
            },
            "/someone/",
        )
    };
    let load = |path: &str| call(&server, Ask::Document { path: path.into() }, "/");

    let error = tab.ask(follow()).await.unwrap_err();
    assert!(matches!(error, PageError::NotReady(_)), "{error:?}");
    tab.ask(load("/someone/")).await.unwrap();
    tab.ask(load("/user0/")).await.unwrap();
    let error = tab.ask(follow()).await.unwrap_err();
    assert!(matches!(error, PageError::NotReady(_)), "{error:?}");
    assert!(sent_since(&server, 0).await.is_empty(), "nothing was sent");
    assert!(!world.account(ig::ME).following.contains(&ig::SOMEONE));

    tab.ask(load("/someone/")).await.unwrap();
    let told = tab.ask(follow()).await.unwrap();
    assert!(
        matches!(&told, Told::Answer(answer) if answer.status == 200),
        "{told:?}"
    );
    assert_eq!(sent_since(&server, 0).await, ["/api/graphql"]);
    assert!(world.account(ig::ME).following.contains(&ig::SOMEONE));
}

/// A session Instagram serves a logged-out profile to writes nothing: the
/// document is loaded, and the write is not asked.
#[tokio::test]
async fn a_write_from_a_logged_out_document_is_not_asked() {
    let world = World::new();
    let server = world.serve_web().await;
    world.refuse(common::SID);
    let client = client_as(&server, &world, Some("me"));
    let error = client
        .follow(Pk::new(ig::SOMEONE), "someone", &Seeds)
        .await
        .unwrap_err();
    assert!(
        matches!(error, snob_ig::error::IgError::SessionExpired),
        "{error:?}"
    );
    assert_eq!(common::requests(&server).await, 0, "nothing was sent");
    assert!(!world.account(ig::ME).following.contains(&ig::SOMEONE));
}

/// A walk follows each page with the app's `show_many` about its accounts,
/// before the next page, once the app has sent the token it goes with; on
/// documents whose app sends none, `show_many` is never sent, and the walk
/// is the same.
#[tokio::test]
async fn each_list_page_is_followed_by_show_many_once_the_app_sent_its_token() {
    use snob_core::model::ListKind;

    let world = World::new();
    world.app_rest_posts(true);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let (users, _) =
        snob_engine::engine::list(&mut app, &query_for("someone"), ListKind::Followers)
            .await
            .unwrap();
    assert_eq!(users.len(), 25);
    let sent = sent_since(&server, 0).await;
    let lists: Vec<&str> = sent
        .iter()
        .map(String::as_str)
        .filter(|p| p.starts_with("/api/v1/friendships/"))
        .collect();
    assert_eq!(
        lists,
        [
            "/api/v1/friendships/9001/followers/",
            "/api/v1/friendships/show_many/",
            "/api/v1/friendships/9001/followers/",
            "/api/v1/friendships/show_many/",
            "/api/v1/friendships/9001/followers/",
            "/api/v1/friendships/show_many/",
        ]
    );
    let statuses = world.statuses();
    assert_eq!(statuses.len(), 3);
    assert_eq!(statuses[0].split(',').count(), 12);
    assert_eq!(statuses[2].split(',').count(), 1);
    assert_eq!(world.navigations(), ["/someone/"]);
    assert!(world.audit(&server, &[]).await.is_empty());

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    snob_engine::engine::list(&mut app, &query_for("someone"), ListKind::Followers)
        .await
        .unwrap();
    assert!(world.statuses().is_empty());
}

/// A profile's grid is its posts query and then the query's connection from
/// the cursor the first page ended at, both made from the profile's page;
/// each post carries its items, and nothing it reads is a call the
/// allowlist refuses or the retired REST reads.
#[tokio::test]
async fn a_grid_is_the_posts_query_and_then_its_next_page() {
    use snob_engine::posts::{Fetched, PostKind, fetch_grid, more};

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.posts = 15);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let Fetched::Grid(mut grid) = fetch_grid(app.client(), "someone", common::ME, None, 1, 0)
        .await
        .unwrap()
    else {
        panic!("someone's grid is shown");
    };
    assert_eq!(grid.posts.len(), 12);
    assert!(grid.next.is_some());
    let mut opened = OPENED.to_vec();
    opened.push("PolarisProfilePostsQuery");
    assert_eq!(queries_since(&server, 0).await, opened);

    let carousel = &grid.posts[0];
    assert_eq!(carousel.kind, PostKind::Carousel(2));
    assert_eq!(carousel.tagged, ["user1"]);
    assert_eq!(carousel.mentions, ["user3"]);
    assert!(carousel.items[1].url.as_deref().unwrap().ends_with(".mp4"));
    assert_eq!(grid.posts[1].kind, PostKind::Reel);
    assert_eq!(grid.posts[1].plays, Some(1234));
    // The faces under a post name the likers the viewer knows; a face with
    // no name is skipped, and with no faces the app's one name stands in.
    let known = |n: usize| -> Vec<String> {
        grid.posts[n]
            .likers_known
            .iter()
            .map(|a| a.username.clone())
            .collect()
    };
    assert_eq!(known(0), ["user2"]);
    assert_eq!(grid.posts[0].likers_known[0].pk.as_deref(), Some("1002"));
    assert_eq!(known(1), ["user2"]);
    assert_eq!(grid.posts[1].likers_known[0].pk, None);
    // Hidden counts are never said, the placeholder least of all.
    assert!(grid.posts[2].likes_hidden);
    assert_eq!(
        grid.posts[2].liked_line().as_deref(),
        Some("Liked by user2 and others")
    );

    let since = common::requests(&server).await;
    assert!(more(app.client(), &mut grid).await.unwrap());
    assert_eq!(grid.posts.len(), 15);
    assert_eq!(grid.next, None);
    assert!(
        !more(app.client(), &mut grid).await.unwrap(),
        "the grid has ended"
    );
    assert_eq!(
        queries_since(&server, since).await,
        ["PolarisProfilePostsTabContentQuery_connection"]
    );
    let page = server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .find(|r| r.url.path() == "/api/graphql")
        .map(|r| {
            (
                ig::header(r, "referer").unwrap_or_default().to_string(),
                r.body.clone(),
            )
        })
        .expect("the next page was asked");
    assert!(page.0.ends_with("/someone/"), "{}", page.0);
    let variables = url::form_urlencoded::parse(&page.1)
        .find(|(n, _)| n == "variables")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    assert!(
        variables.starts_with(r#"{"after":"posts:12""#),
        "{variables}"
    );

    let found = world
        .audit(
            &server,
            &["/api/v1/users/web_profile_info/", "/api/v1/feed/user/*/"],
        )
        .await;
    assert!(found.is_empty(), "{found:?}");
}

/// A reader that names the posts it has pays for the new ones: no page of
/// the grid is asked for past the first that holds one of them, a post the
/// owner pinned never stops the read, and one that is nowhere costs the
/// pages allowed and is said not to have been reached.
#[tokio::test]
async fn a_grid_read_stops_at_the_page_that_holds_a_known_post() {
    use snob_engine::posts::{Fetched, read_grid};
    use snob_ig::shortcode::MediaPk;

    let world = World::new();
    // Thirty posts, of which the owner pinned an old one, the twentieth.
    world.person(ig::SOMEONE, |p| {
        p.posts = 30;
        p.pinned = vec![20];
    });
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let post = |n: u64| MediaPk::new(ig::post_pk(ig::SOMEONE, n));
    let pages_asked = |queries: Vec<String>| {
        queries
            .iter()
            .filter(|q| q.contains("ProfilePosts"))
            .count()
    };
    let read = |until: Vec<MediaPk>| {
        let (app, server) = (&app, &server);
        async move {
            let since = common::requests(server).await;
            let (fetched, how) = read_grid(app.client(), "someone", common::ME, None, 3, 0, &until)
                .await
                .unwrap();
            let Fetched::Grid(grid) = fetched else {
                panic!("someone's grid is shown");
            };
            (grid, how, pages_asked(queries_since(server, since).await))
        }
    };

    // The fourth newest is known: it is on the first page, and that is all.
    let (grid, how, asked) = read(vec![post(3)]).await;
    assert_eq!((how.pages, asked), (1, 1));
    assert_eq!(how.reached, Some(post(3)));
    assert_eq!(grid.posts.len(), 12);
    assert!(grid.posts[0].pinned, "the pinned post comes first");
    assert_eq!(grid.posts[0].pk, post(20));

    // The pinned post is known, and is on the first page: it stops nothing.
    // The other known one is on the second page.
    let (grid, how, asked) = read(vec![post(20), post(14)]).await;
    assert_eq!((how.pages, asked), (2, 2));
    assert_eq!(how.reached, Some(post(14)));
    assert_eq!(grid.posts.len(), 24);

    // A post that is nowhere: the pages allowed, and not reached.
    let (grid, how, asked) = read(vec![MediaPk::new(7)]).await;
    assert_eq!((how.pages, asked), (3, 3));
    assert_eq!(how.reached, None);
    assert_eq!(grid.posts.len(), 30);

    // With no post named, the pages asked for, as before.
    let (_, how, asked) = read(Vec::new()).await;
    assert_eq!((how.pages, asked), (3, 3));
    assert_eq!(how.reached, None);
}

/// A private account the viewer does not follow is answered from its profile:
/// its grid is never asked for.
#[tokio::test]
async fn a_private_grid_is_not_asked_for() {
    use snob_engine::posts::{Fetched, fetch_grid};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let fetched = fetch_grid(app.client(), "ghost", common::ME, None, 1, 0)
        .await
        .unwrap();
    assert!(matches!(fetched, Fetched::Hidden { .. }));
    assert!(
        !queries_since(&server, 0)
            .await
            .iter()
            .any(|q| q.contains("Posts")),
        "the grid was asked for"
    );
}

/// A post opens with the app's two GETs, its info and its first page of
/// comments, made from the page it opens on; the next page of comments
/// carries the cursor the first handed out. A reel's link is its info alone,
/// from the reel's page, and no document is loaded for either.
#[tokio::test]
async fn a_post_opens_with_its_info_and_comments_from_its_own_page() {
    use snob_engine::posts::{Fetched, fetch_grid, fetch_link, more_comments, open};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let Fetched::Grid(grid) = fetch_grid(app.client(), "someone", common::ME, None, 1, 0)
        .await
        .unwrap()
    else {
        panic!("someone's grid is shown");
    };
    let since = common::requests(&server).await;
    let (post, mut comments) = open(app.client(), &grid.posts[0]).await.unwrap();
    assert_eq!(post.items.len(), 2);
    assert_eq!(comments.lines.len(), 15);
    assert_eq!(comments.lines[0].replies_count, 3);
    assert_eq!(comments.lines[0].replies[0].author, "user4");
    assert!(
        more_comments(app.client(), &post, &mut comments)
            .await
            .unwrap()
    );
    assert_eq!(comments.lines.len(), 20);
    assert_eq!(comments.next, None);

    let media = ig::post_pk(ig::SOMEONE, 0);
    let asked: Vec<(String, String)> = server.received_requests().await.unwrap_or_default()
        [since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r))
        .map(|r| {
            (
                format!("{}?{}", r.url.path(), r.url.query().unwrap_or_default()),
                ig::header(r, "referer").unwrap_or_default().to_string(),
            )
        })
        .collect();
    let page = format!("/p/{}/", ig::code_of(media));
    assert_eq!(asked.len(), 3, "{asked:?}");
    assert_eq!(asked[0].0, format!("/api/v1/media/{media}/info/?"));
    assert_eq!(
        asked[1].0,
        format!(
            "/api/v1/media/{media}/comments/?can_support_threading=true&permalink_enabled=false"
        )
    );
    assert!(
        asked[2].0.starts_with(&format!(
            "/api/v1/media/{media}/comments/?can_support_threading=true&min_id="
        )),
        "{:?}",
        asked[2]
    );
    assert!(
        asked[2].0.ends_with("&sort_order=popular"),
        "{:?}",
        asked[2]
    );
    assert!(
        asked.iter().all(|(_, referer)| referer.ends_with(&page)),
        "{asked:?}"
    );

    let reel = ig::post_pk(ig::SOMEONE, 1);
    let link = snob_ig::shortcode::Shortcode::parse(&format!(
        "https://www.instagram.com/reel/{}/?utm_source=ig_web_copy_link",
        ig::code_of(reel)
    ))
    .unwrap();
    let since = common::requests(&server).await;
    let post = fetch_link(app.client(), &link).await.unwrap();
    assert_eq!(post.owner, "someone");
    assert_eq!(post.kind, snob_engine::posts::PostKind::Reel);
    let asked: Vec<String> = sent_since(&server, since).await;
    assert_eq!(asked, [format!("/api/v1/media/{reel}/info/")]);

    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// A tab is the profile, then its first query by pk from the profile's
/// page, then the app's navigation to the tab, then its next pages from the
/// tab's page on the tab's route; the fake's oracle answers 400 to any
/// form or variables that are not the app's, and nothing goes out that the
/// allowlist refuses.
#[tokio::test]
async fn a_tab_is_asked_by_pk_from_the_profile() {
    use snob_engine::posts::{Fetched, read_tab};
    use snob_ig::client::ProfileTab;

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.tab_posts = 16);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let (fetched, how) = read_tab(
        app.client(),
        "someone",
        common::ME,
        None,
        ProfileTab::Reels,
        3,
        0,
        &[],
    )
    .await
    .unwrap();
    let Fetched::Grid(grid) = fetched else {
        panic!("someone's reels are shown");
    };
    assert_eq!(how.pages, 3);
    assert_eq!(grid.posts.len(), 12 + 3 + 1, "twelve, then three a page");
    assert_eq!(grid.next, None);
    assert_eq!(grid.posts[0].owner, "someone");
    assert!(grid.posts[0].likes_hidden, "the placeholder 3 is no count");
    assert_eq!(grid.posts[0].taken_at, snob_core::Epoch::default());
    assert_eq!(grid.declared, None, "no counter counts a tab");

    let mut opened = OPENED.to_vec();
    opened.extend([
        "PolarisProfileReelsTabContentQuery",
        "PolarisProfileReelsTabContentQuery_connection",
        "PolarisProfileReelsTabContentQuery_connection",
    ]);
    assert_eq!(queries_since(&server, 0).await, opened);
    let tab_calls: Vec<(String, String, String)> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|r| !ig::is_the_apps(r))
        .filter(|r| {
            r.url.path() == "/ajax/navigation/"
                || url::form_urlencoded::parse(&r.body)
                    .any(|(n, v)| n == "fb_api_req_friendly_name" && v.contains("ReelsTab"))
        })
        .map(|r| {
            let form: Vec<(String, String)> =
                url::form_urlencoded::parse(&r.body).into_owned().collect();
            let field = |name: &str| {
                form.iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            let referer = ig::header(r, "referer").unwrap_or_default();
            let path = referer.split("://").nth(1).and_then(|r| r.split_once('/'));
            (
                field("route_url"),
                format!("/{}", path.map(|(_, p)| p).unwrap_or_default()),
                field("__crn"),
            )
        })
        .collect();
    let reels_route = "comet.igweb.PolarisProfileReelsTabRoute".to_string();
    assert_eq!(tab_calls[0].1, "/someone/", "{tab_calls:?}");
    assert_eq!(
        tab_calls[1],
        (
            "/someone/reels/".to_string(),
            "/someone/reels/".to_string(),
            reels_route.clone()
        ),
        "{tab_calls:?}"
    );
    for next in &tab_calls[2..] {
        assert_eq!(
            (next.1.as_str(), &next.2),
            ("/someone/reels/", &reels_route)
        );
    }
    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// A tab read on as a view scrolls it is one page a call, from the cursor
/// the page before handed out: the app's navigation to the tab once, every
/// page counted in the account's sitting, the rest a full sitting owes paid
/// before the page and told how long, and nothing past the tab's end.
#[tokio::test]
async fn a_tabs_next_page_comes_from_the_cursor_the_page_before_handed_out() {
    use snob_engine::posts::{first_page, page_after};
    use snob_ig::client::ProfileTab;
    use snob_store::store::Store;
    use snob_store::store::rate_budget;

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.tab_posts = 16);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let (paths, budget) = common::budget_at(tmp.path());
    let app = common::app_web(
        &server,
        &world,
        Store::open(&paths).unwrap(),
        budget,
        snob_engine::app::Viewer {
            pk: common::ME,
            username: Some("me".into()),
        },
    );
    let sitting = || rate_budget::sitting(Store::open(&paths).unwrap().conn()).unwrap();
    let someone = Pk::new(ig::SOMEONE);
    app.profile_named("someone", None).await.unwrap();
    let first = first_page(&app, "someone", someone, Some(ProfileTab::Reels))
        .await
        .unwrap();
    assert_eq!(first.posts.len(), 12);
    let mut after = first.after().expect("the tab goes on");

    // The page that filled the sitting was walked a moment ago.
    let now = snob_core::clock::now_ms().get();
    Store::open(&paths)
        .unwrap()
        .conn()
        .execute(
            "UPDATE sitting SET pages = 40, last_page_at = ?1, rest_until = ?2 WHERE id = 1",
            [now, now + 600_000],
        )
        .expect("the sitting can be written");
    let since = common::requests(&server).await;
    let mut told = None;
    let (second, after) = page_after(&app, &mut after, |left| told = Some(left))
        .await
        .unwrap();
    assert!(told.is_some_and(|left| !left.is_zero()), "{told:?}");
    assert_eq!(second.posts.len(), 3);
    assert_eq!(
        sitting().map(|s| (s.pages, s.rest_until)),
        Some((1, None)),
        "the rest was paid and the page began the next sitting"
    );
    let mut after = after.expect("the tab goes on");

    let mut told = None;
    let (third, after) = page_after(&app, &mut after, |left| told = Some(left))
        .await
        .unwrap();
    assert_eq!(told, None, "no rest is owed");
    assert_eq!(third.posts.len(), 1);
    assert_eq!(after, None, "the tab has ended");
    assert_eq!(sitting().map(|s| s.pages), Some(2));

    assert_eq!(
        queries_since(&server, since).await,
        [
            "PolarisProfileReelsTabContentQuery_connection",
            "PolarisProfileReelsTabContentQuery_connection",
        ]
    );
    let navigations = server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r) && r.url.path() == "/ajax/navigation/")
        .count();
    assert_eq!(navigations, 1, "the tab is opened once");
    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// A tab's page that failed after the app was navigated to the tab is
/// asked again from the same place without navigating to it again.
#[tokio::test]
async fn a_tab_opened_for_a_page_that_failed_is_not_opened_again() {
    use snob_engine::posts::{first_page, page_after};
    use snob_ig::client::ProfileTab;
    use wiremock::matchers::{body_string_contains, method, path};

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.tab_posts = 16);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    app.profile_named("someone", None).await.unwrap();
    let first = first_page(
        &app,
        "someone",
        Pk::new(ig::SOMEONE),
        Some(ProfileTab::Reels),
    )
    .await
    .unwrap();
    let mut after = first.after().expect("the tab goes on");

    wiremock::Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("ReelsTabContentQuery_connection"))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let since = common::requests(&server).await;
    assert!(page_after(&app, &mut after, |_| {}).await.is_err());
    let (second, _) = page_after(&app, &mut after, |_| {}).await.unwrap();
    assert_eq!(second.posts.len(), 3);
    let navigations = server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r) && r.url.path() == "/ajax/navigation/")
        .count();
    assert_eq!(navigations, 1, "the tab is opened once");
}

/// The grid itself reads on from the cursor its first page handed out, as
/// its tabs do, and ends where the grid ends.
#[tokio::test]
async fn a_grids_next_page_comes_from_the_cursor_its_first_page_handed_out() {
    use snob_engine::posts::{first_page, page_after};

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.posts = 15);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    app.profile_named("someone", None).await.unwrap();
    let first = first_page(&app, "someone", Pk::new(ig::SOMEONE), None)
        .await
        .unwrap();
    assert_eq!(first.posts.len(), 12);
    let mut after = first.after().expect("the grid goes on");
    let since = common::requests(&server).await;
    let (second, after) = page_after(&app, &mut after, |_| {}).await.unwrap();
    assert_eq!(second.posts.len(), 3);
    assert_eq!(after, None, "the grid has ended");
    assert_eq!(
        queries_since(&server, since).await,
        ["PolarisProfilePostsTabContentQuery_connection"]
    );
    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// An account whose header says it has no posts can still have reels,
/// tagged posts and reposts, and its tabs are asked for; its grid is not.
/// A tagged post and a repost are their authors', and a repost's note is
/// read.
#[tokio::test]
async fn an_account_with_no_posts_can_still_have_reels() {
    use snob_engine::posts::{Fetched, read_grid, read_tab};
    use snob_ig::client::ProfileTab;

    let world = World::new();
    world.person(ig::SOMEONE, |p| {
        p.posts = 0;
        p.tab_posts = 2;
    });
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let (Fetched::Grid(grid), _) = read_grid(app.client(), "someone", common::ME, None, 1, 0, &[])
        .await
        .unwrap()
    else {
        panic!("someone's grid is shown");
    };
    assert!(grid.posts.is_empty());
    for (tab, owner) in [
        (ProfileTab::Reels, "someone"),
        (ProfileTab::Tagged, "user0"),
        (ProfileTab::Reposts, "user0"),
    ] {
        let (Fetched::Grid(read), how) =
            read_tab(app.client(), "someone", common::ME, None, tab, 1, 0, &[])
                .await
                .unwrap()
        else {
            panic!("{tab:?} is shown");
        };
        assert_eq!(read.posts.len(), 2, "{tab:?}");
        assert_eq!(how.pages, 1);
        assert_eq!(read.posts[0].owner, owner, "{tab:?}");
        assert_eq!(read.next, None, "{tab:?}");
        let noted = tab == ProfileTab::Reposts;
        assert_eq!(read.posts[0].notes.is_empty(), !noted, "{tab:?}");
    }
    let queries = queries_since(&server, 0).await;
    assert!(
        !queries.iter().any(|q| q == "PolarisProfilePostsQuery"),
        "{queries:?}"
    );
    for name in [
        "PolarisProfileReelsTabContentQuery",
        "PolarisProfileTaggedTabContentQuery",
        "PolarisProfileRepostsTabContentWWWQuery",
    ] {
        assert!(queries.iter().any(|q| q == name), "{name}: {queries:?}");
    }
    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// A private account the viewer does not follow is answered from its
/// profile: none of its tabs is asked for.
#[tokio::test]
async fn a_private_account_not_followed_is_asked_for_no_tab() {
    use snob_engine::posts::{Fetched, read_tab};
    use snob_ig::client::ProfileTab;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let (fetched, how) = read_tab(
        app.client(),
        "ghost",
        common::ME,
        None,
        ProfileTab::Tagged,
        1,
        0,
        &[],
    )
    .await
    .unwrap();
    assert!(matches!(fetched, Fetched::Hidden { .. }));
    assert_eq!(how.pages, 0);
    assert!(
        !queries_since(&server, 0)
            .await
            .iter()
            .any(|q| q.contains("Tab")),
        "a tab was asked for"
    );
}

/// A tab's posts looked into or saved are read again by their info first,
/// which carries what the tab did not: the date and the video's address.
#[tokio::test]
async fn a_tabs_post_is_read_whole_by_its_info_before_it_is_saved() {
    use snob_engine::posts::{Fetched, filled, read_tab};
    use snob_ig::client::ProfileTab;

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.tab_posts = 2);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let (Fetched::Grid(grid), _) = read_tab(
        app.client(),
        "someone",
        common::ME,
        None,
        ProfileTab::Reels,
        1,
        0,
        &[],
    )
    .await
    .unwrap() else {
        panic!("someone's reels are shown");
    };
    // A tab hands out no video's address, and a reel's item is no copy of
    // it, not its cover under a video's name.
    assert_eq!(grid.posts[0].items[0].url, None);
    assert!(grid.posts[0].items[0].thumb.is_some(), "its cover is drawn");
    let since = common::requests(&server).await;
    let whole = filled(app.client(), grid.posts.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(whole[0].items[0].url.as_deref().unwrap().ends_with(".mp4"));
    assert_ne!(whole[0].taken_at, snob_core::Epoch::default());
    let asked = sent_since(&server, since).await;
    assert_eq!(asked.len(), 2, "one info a post: {asked:?}");
    assert!(
        asked.iter().all(|path| path.ends_with("/info/")),
        "{asked:?}"
    );
}

/// A thread's replies are one GET under its comment, from the post's page,
/// with the app's three keys and an empty cursor; only the threads the page
/// did not show whole are asked for, and a comment named that the post
/// does not hold is a thread not found, the run going on.
#[tokio::test]
async fn replies_are_read_under_their_comment_from_the_posts_page() {
    use snob_engine::posts::{Fetched, fetch_grid, open, read_threads};
    use snob_ig::shortcode::CommentPk;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let Fetched::Grid(grid) = fetch_grid(app.client(), "someone", common::ME, None, 1, 0)
        .await
        .unwrap()
    else {
        panic!("someone's grid is shown");
    };
    let (post, mut comments) = open(app.client(), &grid.posts[0]).await.unwrap();
    let since = common::requests(&server).await;
    let gone = CommentPk::new(18_000_000_000_009_999);
    let named = read_threads(app.client(), &post, &mut comments, 10, &[gone])
        .await
        .unwrap()
        .expect("nothing stopped the reads");
    assert_eq!(named.len(), 1);
    assert!(!named[0].found && named[0].replies.is_empty());
    // The first comment's one preview of three, and the third's none of
    // nine: each thread hands out two fewer than it counts, and says no more.
    assert_eq!(comments.lines[0].replies.len(), 1);
    assert_eq!(comments.lines[0].more_replies, Some(false));
    assert_eq!(comments.lines[2].replies.len(), 7);
    assert_eq!(comments.lines[2].replies_count, 9);
    assert_eq!(
        comments.lines[1].more_replies, None,
        "no replies, not asked"
    );

    let media = ig::post_pk(ig::SOMEONE, 0);
    let page = format!("/p/{}/", ig::code_of(media));
    let asked: Vec<(String, String)> = server.received_requests().await.unwrap_or_default()
        [since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r))
        .map(|r| {
            (
                format!("{}?{}", r.url.path(), r.url.query().unwrap_or_default()),
                ig::header(r, "referer").unwrap_or_default().to_string(),
            )
        })
        .collect();
    let thread = |comment: &str| {
        format!(
            "/api/v1/media/{media}/comments/{comment}/child_comments/\
             ?min_id=&is_chronological=true&paging_direction=view_more"
        )
    };
    let paths: Vec<&str> = asked.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        [
            thread("18000000000000000"),
            thread("18000000000000002"),
            thread("18000000000009999"),
        ],
        "{asked:?}"
    );
    assert!(
        asked.iter().all(|(_, referer)| referer.ends_with(&page)),
        "{asked:?}"
    );
    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// A post's likers are its info and then one GET with no query from its
/// page, and nothing after them: not the `show_many` the app sends, whose
/// answer the likers already hold.
#[tokio::test]
async fn a_posts_likers_are_one_get_from_its_page_and_nothing_follows() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let app = app_on(&server, &world, tmp.path());
    let media = ig::post_pk(ig::SOMEONE, 0);
    let link = snob_ig::shortcode::Shortcode::parse(&ig::code_of(media)).unwrap();
    let since = common::requests(&server).await;
    let post = snob_engine::posts::fetch_link(app.client(), &link)
        .await
        .unwrap();
    let likers = app
        .client()
        .media_likers(post.pk, &post.page())
        .await
        .unwrap();
    assert_eq!(likers.accounts().len(), 4);
    assert!(likers.accounts()[0].1.as_ref().is_some_and(|f| f.following));
    assert_eq!(
        sent_since(&server, since).await,
        [
            format!("/api/v1/media/{media}/info/"),
            format!("/api/v1/media/{media}/likers/"),
        ]
    );
    let page = format!("/p/{}/", ig::code_of(media));
    let referers: Vec<String> = server.received_requests().await.unwrap_or_default()[since..]
        .iter()
        .filter(|r| !ig::is_the_apps(r))
        .map(|r| ig::header(r, "referer").unwrap_or_default().to_string())
        .collect();
    assert!(referers.iter().all(|r| r.ends_with(&page)), "{referers:?}");

    let found = world.audit(&server, &[]).await;
    assert!(found.is_empty(), "{found:?}");
}

/// What the monitor watches of an account whose posts alone are watched.
fn posts_only(name: Option<&str>) -> snob_engine::watch::Watched {
    use snob_engine::watch::{PostsWatch, Watched};
    let watched = match name {
        Some(name) => Watched::asking(name.to_string()),
        None => Watched::own(),
    };
    watched.watching(false, Some(PostsWatch::default()))
}

/// An hour and a minute go by for the posts the monitor read, so the next
/// run is allowed to read them again (`watch::posts::READ_AT_MOST_EVERY`).
fn an_hour_passes(root: &std::path::Path) {
    common::open_db(root)
        .conn()
        .execute(
            "UPDATE watch_post_marks SET read_at = read_at - 3660, baseline_at = baseline_at - 3660",
            [],
        )
        .unwrap();
}

/// The new posts a tick found, by pk, in the grid's order.
fn news_of(tick: &snob_engine::watch::TickReport) -> Vec<u64> {
    tick.report
        .posts
        .as_ref()
        .expect("the posts were looked at")
        .news
        .iter()
        .map(|post| post.pk.get())
        .collect()
}

/// A tick on `watched` through a new app over the database under `root`,
/// committed.
async fn tick_and_commit(
    server: &MockServer,
    world: &World,
    root: &std::path::Path,
    watched: &snob_engine::watch::Watched,
) -> snob_engine::watch::TickReport {
    let mut app = app_on(server, world, root);
    let tick = snob_engine::watch::tick(&mut app, watched).await.unwrap();
    snob_engine::watch::commit(&mut app, &tick, None).unwrap();
    tick
}

/// The first run on an account's posts reads one page, takes what it holds
/// as what is known, and reports nothing.
#[tokio::test]
async fn the_first_run_on_posts_lays_a_baseline_and_reports_none() {
    use snob_core::watch::posts::Basis;

    let world = World::new();
    world.person(ig::SOMEONE, |p| p.posts = 15);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();

    let tick = tick_and_commit(&server, &world, tmp.path(), &posts_only(Some("someone"))).await;
    let posts = tick.report.posts.as_ref().expect("a report on the posts");
    assert_eq!(posts.basis, Basis::Baseline);
    assert!(posts.news.is_empty());
    assert_eq!(posts.pages, 1);
    assert!(!tick.report.has_changes());
    assert_eq!(
        queries_since(&server, 0)
            .await
            .iter()
            .filter(|q| q.contains("ProfilePosts"))
            .count(),
        1,
        "one page of the grid"
    );

    let db = common::open_db(tmp.path());
    let pk = Pk::new(ig::SOMEONE);
    let mark = snob_store::store::watch_posts::mark(db.conn(), pk)
        .unwrap()
        .expect("the baseline is on record");
    assert_eq!(mark.reported_at, None);
    assert_eq!(
        snob_store::store::watch_posts::newest(db.conn(), pk, 100)
            .unwrap()
            .len(),
        12
    );
}

/// A post published after the baseline is news on the next run, and on no
/// run after it.
#[tokio::test]
async fn a_post_published_since_is_reported_once_and_never_again() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    world.person(ig::SOMEONE, |p| p.newer = 2);
    an_hour_passes(tmp.path());
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert_eq!(
        news_of(&tick),
        [
            ig::newer_post(ig::SOMEONE, 1),
            ig::newer_post(ig::SOMEONE, 0)
        ]
    );
    assert!(tick.report.has_changes());
    assert_eq!(tick.report.change_count(), 2);
    assert_eq!(tick.report.list_change_count(), 0);

    an_hour_passes(tmp.path());
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert!(news_of(&tick).is_empty());
    assert!(!tick.report.has_changes());
}

/// A run that finds no new post costs the profile, opened as the app opens
/// one (four reads), and one page of the grid; with the lists of somebody
/// else watched too, the profile the lists opened is the one the grid
/// follows, and the grid is the one read more.
#[tokio::test]
async fn a_run_with_no_new_post_costs_the_profile_and_one_page() {
    use snob_engine::watch::{Consent, PostsWatch, Watched};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    an_hour_passes(tmp.path());
    let since = common::requests(&server).await;
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert_eq!(tick.requests, 5, "{:?}", sent_since(&server, since).await);
    let mut opened = OPENED.to_vec();
    opened.push("PolarisProfilePostsQuery");
    assert_eq!(relay_since(&server, since).await, opened);

    let other = tempfile::tempdir().unwrap();
    let both =
        Watched::consented("someone".into(), Consent).watching(true, Some(PostsWatch::default()));
    tick_and_commit(&server, &world, other.path(), &both).await;
    an_hour_passes(other.path());
    let since = common::requests(&server).await;
    let tick = tick_and_commit(&server, &world, other.path(), &both).await;
    assert_eq!(tick.requests, 5, "{:?}", sent_since(&server, since).await);
    assert_eq!(relay_since(&server, since).await, opened);
}

/// An old post the owner pinned sits on top of the grid: it is never news,
/// and a read that knows it still goes on to the page that holds a post it
/// knows below it.
#[tokio::test]
async fn a_pinned_old_post_is_never_news_and_never_stops_the_read() {
    let world = World::new();
    world.person(ig::SOMEONE, |p| {
        p.posts = 30;
        p.pinned = vec![20];
    });
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    // Fourteen new posts: the first page is the pinned one and eleven of
    // them, the second the other three and posts seen before. The pin is
    // kept on the same post as the new ones push it down the grid.
    world.person(ig::SOMEONE, |p| {
        p.newer = 14;
        p.pinned = vec![20 + 14];
    });
    an_hour_passes(tmp.path());
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    let news = news_of(&tick);
    assert_eq!(news.len(), 14, "{news:?}");
    assert!(!news.contains(&ig::post_pk(ig::SOMEONE, 20)));
    let posts = tick.report.posts.as_ref().unwrap();
    assert_eq!(posts.pages, 2, "the pinned post stopped nothing");
    assert!(posts.complete);
}

/// When the owner deletes posts, the older ones behind them move up the
/// grid with pks never seen; they are older than everything seen, and are
/// not news.
#[tokio::test]
async fn a_post_that_moved_up_after_a_deletion_is_not_news() {
    let world = World::new();
    world.person(ig::SOMEONE, |p| p.posts = 15);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    world.person(ig::SOMEONE, |p| p.deleted = vec![0, 1, 2]);
    an_hour_passes(tmp.path());
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert!(news_of(&tick).is_empty());
    let posts = tick.report.posts.as_ref().unwrap();
    assert_eq!(posts.pages, 1);
    assert!(posts.complete);
}

/// More new posts than three pages hold: the three are read and reported,
/// and the report says the read did not reach what was known.
#[tokio::test]
async fn more_new_posts_than_three_pages_says_the_read_is_not_complete() {
    let world = World::new();
    world.person(ig::SOMEONE, |p| p.posts = 15);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    world.person(ig::SOMEONE, |p| p.newer = 40);
    an_hour_passes(tmp.path());
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    let posts = tick.report.posts.as_ref().unwrap();
    assert_eq!(posts.pages, 3);
    assert_eq!(posts.news.len(), 36);
    assert!(!posts.complete);
}

/// A private account the viewer does not follow is said hidden, from its
/// profile, and its grid is never asked for. Nothing else was looked at, so
/// the run is an error.
#[tokio::test]
async fn a_private_account_not_followed_is_said_hidden_and_its_grid_is_not_asked_for() {
    use snob_engine::watch::posts::PostsSkipped;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let mut app = app_on(&server, &world, tmp.path());
    let tick = snob_engine::watch::tick(&mut app, &posts_only(Some("ghost")))
        .await
        .unwrap();
    assert_eq!(tick.posts_skipped(), Some(PostsSkipped::Hidden));
    assert_eq!(tick.outcome(), snob_engine::exit::ExitCode::Error);
    assert!(
        !queries_since(&server, 0)
            .await
            .iter()
            .any(|q| q.contains("Posts")),
        "the grid was asked for"
    );
}

/// In a cooldown nothing is sent for the posts, not even the profile.
#[tokio::test]
async fn posts_are_not_read_in_a_cooldown() {
    use snob_engine::watch::posts::PostsSkipped;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let (paths, budget) = common::budget_at(tmp.path());
    let app_of = || {
        common::app_web(
            &server,
            &world,
            snob_store::store::Store::open(&paths).unwrap(),
            budget.clone(),
            snob_engine::app::Viewer {
                pk: common::ME,
                username: Some("me".into()),
            },
        )
    };
    let watched = posts_only(Some("someone"));
    let mut app = app_of();
    let first = snob_engine::watch::tick(&mut app, &watched).await.unwrap();
    snob_engine::watch::commit(&mut app, &first, None).unwrap();
    snob_store::store::Store::open(&paths)
        .unwrap()
        .conn()
        .execute("UPDATE watch_post_marks SET read_at = read_at - 3660", [])
        .unwrap();

    common::start_cooldown(&budget, common::HOUR);
    let since = common::requests(&server).await;
    let mut app = app_of();
    let tick = snob_engine::watch::tick(&mut app, &watched).await.unwrap();
    assert_eq!(tick.posts_skipped(), Some(PostsSkipped::Cooldown));
    assert_eq!(tick.outcome(), snob_engine::exit::ExitCode::RateLimited);
    assert!(sent_since(&server, since).await.is_empty());
}

/// Posts read less than an hour ago are not read again, and that is no
/// failure: the run sends nothing and ends well.
#[tokio::test]
async fn posts_read_less_than_an_hour_ago_are_not_read_again() {
    use snob_engine::watch::posts::PostsSkipped;

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    let since = common::requests(&server).await;
    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert_eq!(tick.posts_skipped(), Some(PostsSkipped::ReadRecently));
    assert_eq!(tick.requests, 0);
    assert_eq!(tick.outcome(), snob_engine::exit::ExitCode::Ok);
    assert!(sent_since(&server, since).await.is_empty());
}

/// An entry that watches posts alone walks no list, so it needs no answer
/// to read somebody else's: one built with the question still to ask runs
/// with nobody there.
#[tokio::test]
async fn a_posts_only_entry_walks_no_list_and_asks_no_consent() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    assert!(watched.may_run_unattended());

    let tick = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert!(tick.lists.is_empty());
    assert!(tick.report.followers.is_none() && tick.report.following.is_none());
    assert!(tick.looked());
    let sent = sent_since(&server, 0).await;
    assert!(
        !sent.iter().any(|p| p.contains("/friendships/")),
        "{sent:?}"
    );
    assert!(world.audit(&server, &[]).await.is_empty());
}

/// A push-back on the grid ends the posts of that run, with no retry, and
/// what the lists concluded is still reported and committed; the posts'
/// receipt does not move.
#[tokio::test]
async fn a_push_back_on_the_grid_still_reports_what_the_lists_found() {
    use snob_engine::watch::posts::PostsSkipped;
    use snob_engine::watch::{Consent, PostsWatch, Watched};
    use wiremock::matchers::{body_string_contains, method, path};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let both =
        Watched::consented("someone".into(), Consent).watching(true, Some(PostsWatch::default()));
    tick_and_commit(&server, &world, tmp.path(), &both).await;
    let read_before = snob_store::store::watch_posts::mark(
        common::open_db(tmp.path()).conn(),
        Pk::new(ig::SOMEONE),
    )
    .unwrap()
    .unwrap()
    .read_at;
    an_hour_passes(tmp.path());

    wiremock::Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("PolarisProfilePostsQuery"))
        .respond_with(wiremock::ResponseTemplate::new(429).set_body_string("{}"))
        .with_priority(1)
        .mount(&server)
        .await;
    let tick = tick_and_commit(&server, &world, tmp.path(), &both).await;
    assert!(matches!(
        tick.posts_skipped(),
        Some(PostsSkipped::Incomplete(_, _))
    ));
    assert!(tick.report.posts.is_none());
    assert!(
        tick.lists.iter().all(|list| list.skipped.is_none()),
        "{:?}",
        tick.lists
    );
    assert_eq!(tick.outcome(), snob_engine::exit::ExitCode::Ok);
    let posts_asked = queries_since(&server, 0)
        .await
        .iter()
        .filter(|q| q.as_str() == "PolarisProfilePostsQuery")
        .count();
    assert_eq!(posts_asked, 2, "the baseline's, and the one refused, once");
    assert_eq!(
        snob_store::store::watch_posts::mark(
            common::open_db(tmp.path()).conn(),
            Pk::new(ig::SOMEONE)
        )
        .unwrap()
        .unwrap()
        .read_at,
        read_before - std::time::Duration::from_secs(3660),
        "only the hour that went by moved it"
    );
}

/// A tick whose report was never committed, as a process killed before
/// its commit leaves it, wrote nothing: the next one finds the same posts
/// and reports them.
#[tokio::test]
async fn a_tick_killed_before_its_commit_reports_the_same_posts_next_time() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let watched = posts_only(Some("someone"));
    tick_and_commit(&server, &world, tmp.path(), &watched).await;

    world.person(ig::SOMEONE, |p| p.newer = 1);
    an_hour_passes(tmp.path());
    let mut app = app_on(&server, &world, tmp.path());
    let lost = snob_engine::watch::tick(&mut app, &watched).await.unwrap();
    drop(app);
    let again = tick_and_commit(&server, &world, tmp.path(), &watched).await;
    assert_eq!(news_of(&lost), [ig::newer_post(ig::SOMEONE, 0)]);
    assert_eq!(news_of(&again), news_of(&lost));
}

/// An entry that watches `name`'s posts alone and saves them into `dir`,
/// videos as the app's file, so no ffmpeg is asked after.
fn saving_to(name: &str, dir: &std::path::Path) -> snob_engine::watch::Watched {
    use snob_engine::watch::{PostsWatch, Watched};
    Watched::asking(name.to_string()).watching(
        false,
        Some(PostsWatch {
            save_to: Some(dir.to_path_buf()),
            quality: Some(snob_store::config::VideoQuality::Compat),
        }),
    )
}

/// A tick on `watched`, its posts saved as a run saves them, and, when
/// `commit`, committed.
async fn tick_saving(
    server: &MockServer,
    world: &World,
    root: &std::path::Path,
    watched: &snob_engine::watch::Watched,
    commit: bool,
) -> snob_engine::watch::TickReport {
    let mut app = app_on(server, world, root);
    let mut tick = snob_engine::watch::tick(&mut app, watched).await.unwrap();
    snob_cli::commands::watch::posts::save_new(&app, &mut tick, watched)
        .await
        .unwrap();
    if commit {
        snob_engine::watch::commit(&mut app, &tick, None).unwrap();
    }
    tick
}

/// The names in `dir`, sorted; none when it is not there.
fn files_in(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// How many files of the CDN were asked for since the first `since`
/// requests.
async fn media_since(server: &MockServer, since: usize) -> usize {
    sent_since(server, since)
        .await
        .iter()
        .filter(|path| path.starts_with("/media/"))
        .count()
}

/// A new post's files are saved under the names every download gives them,
/// `<owner>-<code>`, and the report names them.
#[tokio::test]
async fn a_new_posts_files_are_saved_under_their_own_names() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("media");
    let watched = saving_to("someone", &dir);
    tick_saving(&server, &world, tmp.path(), &watched, true).await;

    world.person(ig::SOMEONE, |p| p.newer = 1);
    an_hour_passes(tmp.path());
    let tick = tick_saving(&server, &world, tmp.path(), &watched, true).await;
    let new = ig::newer_post(ig::SOMEONE, 0);
    let name = format!("someone-{}.jpg", ig::code_of(new));
    assert_eq!(files_in(&dir), std::slice::from_ref(&name));
    let saved = &tick.report.posts.as_ref().unwrap().saved;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].files, [name]);
    assert_eq!((saved[0].failed, saved[0].left), (0, 0));
    assert!(
        snob_store::store::watch_posts::pending_saves(
            common::open_db(tmp.path()).conn(),
            Pk::new(ig::SOMEONE)
        )
        .unwrap()
        .is_empty(),
        "nothing is owed"
    );
}

/// Nothing of what the first run takes as known is saved: it is not news.
#[tokio::test]
async fn nothing_of_the_baseline_is_saved() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("media");
    tick_saving(
        &server,
        &world,
        tmp.path(),
        &saving_to("someone", &dir),
        true,
    )
    .await;
    assert!(files_in(&dir).is_empty());
    assert_eq!(media_since(&server, 0).await, 0);
}

/// A run that saved and was stopped before its commit finds the same post
/// next time, and does not fetch what is already in the directory.
#[tokio::test]
async fn a_file_already_there_is_not_fetched_again() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("media");
    let watched = saving_to("someone", &dir);
    tick_saving(&server, &world, tmp.path(), &watched, true).await;

    world.person(ig::SOMEONE, |p| p.newer = 1);
    an_hour_passes(tmp.path());
    tick_saving(&server, &world, tmp.path(), &watched, false).await;
    assert_eq!(files_in(&dir).len(), 1);

    let since = common::requests(&server).await;
    let again = tick_saving(&server, &world, tmp.path(), &watched, true).await;
    assert_eq!(news_of(&again), [ig::newer_post(ig::SOMEONE, 0)]);
    assert_eq!(
        media_since(&server, since).await,
        0,
        "nothing fetched twice"
    );
    assert_eq!(again.report.posts.as_ref().unwrap().saved[0].files.len(), 1);
}

/// A file that failed is owed, and a later run saves it while its post is
/// still on the page it reads, asking for nothing more than that page.
#[tokio::test]
async fn a_file_that_failed_is_tried_again_while_its_post_is_still_on_the_page() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("media");
    let watched = saving_to("someone", &dir);
    tick_saving(&server, &world, tmp.path(), &watched, true).await;

    world.person(ig::SOMEONE, |p| p.newer = 1);
    let new = ig::newer_post(ig::SOMEONE, 0);
    wiremock::Mock::given(wiremock::matchers::path(format!("/media/{new}.jpg")))
        .respond_with(wiremock::ResponseTemplate::new(500))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&server)
        .await;
    an_hour_passes(tmp.path());
    let tick = tick_saving(&server, &world, tmp.path(), &watched, true).await;
    assert_eq!(tick.report.posts.as_ref().unwrap().saved[0].failed, 1);
    assert!(files_in(&dir).is_empty());
    let owed = |root: &std::path::Path| {
        snob_store::store::watch_posts::pending_saves(
            common::open_db(root).conn(),
            Pk::new(ig::SOMEONE),
        )
        .unwrap()
        .len()
    };
    assert_eq!(owed(tmp.path()), 1);

    an_hour_passes(tmp.path());
    let tick = tick_saving(&server, &world, tmp.path(), &watched, true).await;
    assert!(news_of(&tick).is_empty(), "it was reported already");
    assert_eq!(files_in(&dir).len(), 1);
    assert_eq!(owed(tmp.path()), 0);
}

/// A directory that cannot be made fails the saves, not the run: the report
/// still names the new post, and says nothing of it was saved.
#[tokio::test]
async fn a_failed_save_does_not_keep_the_report_from_going_out() {
    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let in_the_way = tmp.path().join("a file");
    std::fs::write(&in_the_way, b"not a directory").unwrap();
    let watched = saving_to("someone", &in_the_way.join("media"));
    tick_saving(&server, &world, tmp.path(), &watched, true).await;

    world.person(ig::SOMEONE, |p| p.newer = 1);
    an_hour_passes(tmp.path());
    let tick = tick_saving(&server, &world, tmp.path(), &watched, true).await;
    assert!(tick.report.has_changes());
    assert_eq!(news_of(&tick), [ig::newer_post(ig::SOMEONE, 0)]);
    let saved = &tick.report.posts.as_ref().unwrap().saved;
    assert!(saved[0].files.is_empty());
    assert_eq!(saved[0].failed, 1);
}

/// The report names every file of a post that was saved, a carousel's
/// photo and video both, in the directory it gives as a whole path. The
/// carousel here is a post brought back from the archive that is newer
/// than everything seen, which is news.
#[tokio::test]
async fn the_report_names_the_files_that_were_saved() {
    let world = World::new();
    world.person(ig::SOMEONE, |p| p.deleted = vec![0]);
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("media");
    let watched = saving_to("someone", &dir);
    tick_saving(&server, &world, tmp.path(), &watched, true).await;

    world.person(ig::SOMEONE, |p| p.deleted.clear());
    an_hour_passes(tmp.path());
    let tick = tick_saving(&server, &world, tmp.path(), &watched, true).await;
    let carousel = ig::post_pk(ig::SOMEONE, 0);
    assert_eq!(news_of(&tick), [carousel]);
    let saved = &tick.report.posts.as_ref().unwrap().saved[0];
    let code = ig::code_of(carousel);
    assert_eq!(
        saved.files,
        [
            format!("someone-{code}-1.jpg"),
            format!("someone-{code}-2.mp4")
        ]
    );
    assert_eq!(files_in(&dir), saved.files);
    assert!(std::path::Path::new(&saved.directory).is_absolute());
}

/// An automation's crossing of the own lists is the same crossing `snob
/// unfollowers` makes, through `engine::list`; a "changes" filter lays a
/// baseline on the first run, and says what came and went against the
/// accounts the last report named, moved only once the report is made.
#[tokio::test]
async fn an_automations_unfollowers_are_told_as_what_changed_since_the_last_report() {
    use snob_core::automation::{Account, Filters, Only, Planned, Read};
    use snob_engine::automations::read::{self, Changes};

    let world = World::new();
    let server = world.serve_web().await;
    let tmp = tempfile::tempdir().unwrap();
    let planned = Planned {
        node: 2,
        account: Account::own(None),
        what: Read::Unfollowers,
        only: Filters::new([Only::Changes]),
        delivered: false,
    };

    let mut app = app_on(&server, &world, tmp.path());
    let first = read::read(&mut app, "a1", &planned).await.unwrap();
    let names = |users: &[snob_core::model::User]| -> Vec<String> {
        users.iter().map(|u| u.username.clone()).collect()
    };
    assert_eq!(
        names(&first.accounts),
        ["user5", "user6", "user7", "user8", "user9"],
        "me follows user0 to user9, and user0 to user4 follow back"
    );
    assert_eq!(first.changes, Some(Changes::Baseline));
    assert!(!first.has_news(), "a baseline says nothing");
    read::commit(&app, "a1", &first, snob_core::Epoch::new(1)).unwrap();

    // The last report named two of them, and one account since gone.
    snob_store::store::automation_marks::set(
        app.db().conn(),
        "a1",
        2,
        snob_core::Epoch::new(1),
        &[Pk::new(1005), Pk::new(1006), Pk::new(1010)],
    )
    .unwrap();
    let second = read::read(&mut app, "a1", &planned).await.unwrap();
    let Some(Changes::Since { added, removed, .. }) = &second.changes else {
        panic!("compared against the mark: {:?}", second.changes);
    };
    assert_eq!(names(added), ["user7", "user8", "user9"]);
    assert_eq!(
        removed.iter().map(|u| u.pk).collect::<Vec<_>>(),
        [Pk::new(1010)]
    );
    assert!(second.has_news());
}
