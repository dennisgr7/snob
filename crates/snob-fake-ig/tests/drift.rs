//! Compares the JSON shapes of `snob-fake-ig`'s answers with local session
//! captures to flag schema drift.
//!
//! **Privacy**: recordings in `SNOB_CAPTURES` / `tools/capture/out/` come from
//! real accounts. Only key paths, JSON kind names (`null`, `bool`, `number`,
//! `string`, `array`, `object`), allowlisted operation names and counts are kept
//! or printed — never JSON scalar values, URLs, query parameters, tokens or
//! account identifiers.
//!
//! **Failure rule**: the hand-run capture test is `#[ignore]` so normal test
//! suites stay offline. When run, it fails only if an allowlisted operation
//! seen in the captures fails to replay against the fake with HTTP 200 and
//! valid JSON carrying its expected root key (`data.<root_field>`, `data` for
//! `StoriesTray`, or `status` for REST reads); missing keys, extra keys and
//! disjoint scalar types are reported as drift without failing the build.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use snob_core::Pk;
use snob_core::session::{Session, SessionOrigin};
use snob_fake_ig::tab::TestTab;
use snob_fake_ig::world::{FIRST_USER, SOMEONE, Story, World, post_pk};
use snob_ig::allowlist::Operation;
use snob_ig::client::page::Page;
use snob_ig::graphql::PROFILE_ROUTE;
use snob_ig::shortcode::{CommentPk, MediaPk};
use snob_ig::web::{
    Ask, Call, Told, posts_page_variables, posts_variables, profile_variables, reel_variables,
    reels_page_variables, reels_variables, reposts_page_variables, reposts_variables,
    tagged_page_variables, tagged_variables,
};
use url::Url;
use wiremock::MockServer;

const CAP_BYTES: u64 = 4 << 20;
const TIMEOUT_MS: u64 = 20_000;
const TEST_SESSION_ID: &str = "42%3AAbCdEfGh%3A20";
const TEST_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl Kind {
    fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(_) => Self::Bool,
            Value::Number(_) => Self::Number,
            Value::String(_) => Self::String,
            Value::Array(_) => Self::Array,
            Value::Object(_) => Self::Object,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool => "bool",
            Self::Number => "number",
            Self::String => "string",
            Self::Array => "array",
            Self::Object => "object",
        }
    }
}

fn format_kinds(kinds: &BTreeSet<Kind>) -> String {
    kinds
        .iter()
        .map(|k| k.label())
        .collect::<Vec<_>>()
        .join(" | ")
}

type Shape = BTreeMap<String, BTreeSet<Kind>>;

#[derive(Debug, Default)]
struct ShapeDiff {
    real_only: Vec<(String, BTreeSet<Kind>)>,
    fake_only: Vec<(String, BTreeSet<Kind>)>,
    type_mismatches: Vec<(String, BTreeSet<Kind>, BTreeSet<Kind>)>,
}

fn non_null_kinds(kinds: &BTreeSet<Kind>) -> BTreeSet<Kind> {
    kinds.iter().copied().filter(|k| *k != Kind::Null).collect()
}

fn diff_shapes(real: &Shape, fake: &Shape) -> ShapeDiff {
    let mut diff = ShapeDiff::default();
    for (path, real_kinds) in real {
        if let Some(fake_kinds) = fake.get(path) {
            let r_nn = non_null_kinds(real_kinds);
            let f_nn = non_null_kinds(fake_kinds);
            if !r_nn.is_empty() && !f_nn.is_empty() && r_nn.is_disjoint(&f_nn) {
                diff.type_mismatches
                    .push((path.clone(), real_kinds.clone(), fake_kinds.clone()));
            }
        } else {
            diff.real_only.push((path.clone(), real_kinds.clone()));
        }
    }
    for (path, fake_kinds) in fake {
        if !real.contains_key(path) {
            diff.fake_only.push((path.clone(), fake_kinds.clone()));
        }
    }
    diff
}

fn merge_value(value: &Value, shape: &mut Shape) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                collect_value(key, child, shape);
            }
        }
        other => collect_value("$", other, shape),
    }
}

fn collect_value(path: &str, value: &Value, shape: &mut Shape) {
    let kind = Kind::of(value);
    shape.entry(path.to_string()).or_default().insert(kind);
    match value {
        Value::Array(items) => {
            let item_path = format!("{path}[]");
            for item in items {
                collect_value(&item_path, item, shape);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let child_path = format!("{path}.{key}");
                collect_value(&child_path, child, shape);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Parses one JSON value or a stream of newline-delimited JSON objects after
/// stripping an optional `for (;;);` XSSI prefix.
fn parse_documents(body: &str) -> Vec<Value> {
    let text = body.trim_start();
    let text = text.strip_prefix("for (;;);").unwrap_or(text).trim();
    if text.is_empty() {
        return Vec::new();
    }
    if let Ok(single) = serde_json::from_str::<Value>(text) {
        return vec![single];
    }
    let mut docs = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(doc) = serde_json::from_str::<Value>(trimmed) else {
            return Vec::new();
        };
        docs.push(doc);
    }
    docs
}

fn extract_graphql_body_shape(body: &str, shape: &mut Shape) -> usize {
    let mut parsed_docs = 0;
    for doc in parse_documents(body) {
        let Some(obj) = doc.as_object() else {
            continue;
        };
        // Skip incremental/deferred GraphQL delivery patches (`path: [...]`).
        if obj.get("path").is_some_and(Value::is_array) {
            continue;
        }
        // Skip error-only answers that carry no `"data"`, or `"data": null`
        // paired with `"errors"`.
        let Some(data) = obj.get("data") else {
            continue;
        };
        if data.is_null() && obj.get("errors").is_some() {
            continue;
        }
        merge_value(&doc, shape);
        parsed_docs += 1;
    }
    parsed_docs
}

fn extract_rest_body_shape(body: &str, shape: &mut Shape) -> usize {
    let mut parsed_docs = 0;
    for doc in parse_documents(body) {
        if doc.is_object() {
            merge_value(&doc, shape);
            parsed_docs += 1;
        }
    }
    parsed_docs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestKind {
    Followers,
    Following,
    MutualFollowers,
    MediaInfo,
    Comments,
    Likers,
    Replies,
}

impl RestKind {
    fn label(self) -> &'static str {
        match self {
            Self::Followers => "REST GET /api/v1/friendships/{pk}/followers/",
            Self::Following => "REST GET /api/v1/friendships/{pk}/following/",
            Self::MutualFollowers => "REST GET /api/v1/friendships/{pk}/mutual_followers/",
            Self::MediaInfo => "REST GET /api/v1/media/{pk}/info/",
            Self::Comments => "REST GET /api/v1/media/{pk}/comments/",
            Self::Likers => "REST GET /api/v1/media/{pk}/likers/",
            Self::Replies => "REST GET /api/v1/media/{pk}/comments/{comment_pk}/child_comments/",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Graphql(Operation),
    Rest(RestKind),
}

impl Target {
    fn key(self) -> String {
        match self {
            Self::Graphql(op) => format!("0:{}", op.friendly_name()),
            Self::Rest(rest) => format!("1:{}", rest.label()),
        }
    }

    fn display_name(self) -> String {
        match self {
            Self::Graphql(op) => format!("{} ({})", op.friendly_name(), op.path()),
            Self::Rest(rest) => rest.label().to_string(),
        }
    }

    fn is_write(self) -> bool {
        matches!(self, Self::Graphql(op) if op.write().is_some())
    }
}

fn is_digits(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit())
}

fn is_media_id(segment: &str) -> bool {
    match segment.split_once('_') {
        Some((media, owner)) => is_digits(media) && is_digits(owner),
        None => is_digits(segment),
    }
}

fn classify_rest_path(path: &str) -> Option<RestKind> {
    let trimmed = path.trim_matches('/');
    let segments: Vec<&str> = trimmed.split('/').collect();
    match segments.as_slice() {
        ["api", "v1", "friendships", pk, "followers"] if is_digits(pk) => Some(RestKind::Followers),
        ["api", "v1", "friendships", pk, "following"] if is_digits(pk) => Some(RestKind::Following),
        ["api", "v1", "friendships", pk, "mutual_followers"] if is_digits(pk) => {
            Some(RestKind::MutualFollowers)
        }
        ["api", "v1", "media", media, "info"] if is_media_id(media) => Some(RestKind::MediaInfo),
        ["api", "v1", "media", media, "comments"] if is_media_id(media) => Some(RestKind::Comments),
        ["api", "v1", "media", media, "likers"] if is_media_id(media) => Some(RestKind::Likers),
        [
            "api",
            "v1",
            "media",
            media,
            "comments",
            comment,
            "child_comments",
        ] if is_media_id(media) && is_digits(comment) => Some(RestKind::Replies),
        _ => None,
    }
}

fn header_ci<'a>(event: &'a Value, name: &str) -> Option<&'a str> {
    event
        .get("requestHeaders")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.as_str())
}

fn classify_event(event: &Value) -> Option<Target> {
    let kind = event.get("kind").and_then(Value::as_str)?;
    if !matches!(kind, "api" | "fetch" | "xhr") {
        return None;
    }
    if event.get("status").and_then(Value::as_u64) != Some(200) {
        return None;
    }
    let method = event.get("method").and_then(Value::as_str)?;
    let url = Url::parse(event.get("url").and_then(Value::as_str)?).ok()?;
    let path = url.path();

    if method == "POST" && matches!(path, "/api/graphql" | "/graphql/query") {
        let post_data = event.get("postData").and_then(Value::as_str).unwrap_or("");
        let form: Vec<(String, String)> = url::form_urlencoded::parse(post_data.as_bytes())
            .into_owned()
            .collect();
        let form_field = |key: &str| form.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if let Some(name) = form_field("fb_api_req_friendly_name")
            .or_else(|| header_ci(event, "x-fb-friendly-name"))
            && let Some(op) = Operation::named(name)
        {
            return Some(Target::Graphql(op));
        }
        if let Some(doc_id) = form_field("doc_id")
            && let Some(op) = Operation::ALL.into_iter().find(|op| op.doc_id() == doc_id)
        {
            return Some(Target::Graphql(op));
        }
        return None;
    }

    if method == "GET" {
        return classify_rest_path(path).map(Target::Rest);
    }

    None
}

fn repo_root() -> Option<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|d| d.join("Cargo.lock").is_file())
        .map(Path::to_path_buf)
}

fn collect_capture_files(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if path.is_file() {
        files.push(path.to_path_buf());
        return files;
    }
    if !path.is_dir() {
        return files;
    }
    let direct = path.join("events.jsonl");
    if direct.is_file() {
        files.push(direct);
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return files;
    };
    let mut subdirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    for subdir in subdirs {
        let candidate = subdir.join("events.jsonl");
        if candidate.is_file() {
            files.push(candidate);
        }
    }
    files
}

fn capture_files() -> Vec<PathBuf> {
    if let Some(env_path) = std::env::var_os("SNOB_CAPTURES").filter(|s| !s.is_empty()) {
        return collect_capture_files(Path::new(&env_path));
    }
    let Some(root) = repo_root() else {
        return Vec::new();
    };
    collect_capture_files(&root.join("tools").join("capture").join("out"))
}

struct CapturedShape {
    target: Target,
    samples: usize,
    shape: Shape,
}

fn load_captures(files: &[PathBuf]) -> BTreeMap<String, CapturedShape> {
    let mut by_target: BTreeMap<String, CapturedShape> = BTreeMap::new();
    for file in files {
        let Ok(opened) = File::open(file) else {
            continue;
        };
        let reader = BufReader::new(opened);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            let Some(target) = classify_event(&event) else {
                continue;
            };
            let Some(body) = event.get("body").and_then(Value::as_str) else {
                continue;
            };
            if body.trim().is_empty() {
                continue;
            }
            let entry = by_target
                .entry(target.key())
                .or_insert_with(|| CapturedShape {
                    target,
                    samples: 0,
                    shape: Shape::new(),
                });
            let added = match target {
                Target::Graphql(_) => extract_graphql_body_shape(body, &mut entry.shape),
                Target::Rest(_) => extract_rest_body_shape(body, &mut entry.shape),
            };
            entry.samples += usize::from(added > 0);
        }
    }
    by_target
}

fn build_world() -> World {
    let world = World::new();
    world.person(SOMEONE, |p| {
        p.posts = 15;
        p.tab_posts = 16;
        p.stories.push(Story {
            pk: "3000000000000000202".into(),
            taken_at: 1_790_000_200,
            video: true,
        });
    });
    world
}

fn call_on(server: &MockServer, ask: Ask, referrer: &str) -> Call {
    Call {
        ask,
        origin: server.uri(),
        referrer: referrer.into(),
        claim: "0".into(),
        cap: CAP_BYTES,
        timeout_ms: TIMEOUT_MS,
    }
}

async fn ask_json(
    tab: &TestTab,
    server: &MockServer,
    ask: Ask,
    referrer: &str,
    what: &str,
) -> Value {
    let Told::Answer(answer) = tab
        .ask(call_on(server, ask, referrer))
        .await
        .unwrap_or_else(|e| panic!("{what}: page ask failed: {e}"))
    else {
        panic!("{what}: expected Told::Answer");
    };
    assert_eq!(
        answer.status, 200,
        "{what}: expected HTTP 200 from fake, got {}",
        answer.status
    );
    let docs = parse_documents(&answer.body);
    assert!(
        !docs.is_empty(),
        "{what}: fake did not return valid JSON (body len {})",
        answer.body.len()
    );
    docs.into_iter().next().unwrap()
}

async fn query_op(
    tab: &TestTab,
    server: &MockServer,
    op: Operation,
    variables: String,
    referrer: &str,
) -> Value {
    let ask = Ask::Query {
        operation: op,
        variables,
    };
    let doc = ask_json(tab, server, ask, referrer, op.friendly_name()).await;
    if op == Operation::StoriesTray {
        assert!(
            doc.get("data").is_some(),
            "{}: missing top-level `data` key",
            op.friendly_name()
        );
    } else {
        assert!(
            doc.get("data")
                .and_then(|d| d.get(op.root_field()))
                .is_some(),
            "{}: missing `data.{}` in fake answer",
            op.friendly_name(),
            op.root_field()
        );
    }
    doc
}

async fn query_rest(
    tab: &TestTab,
    server: &MockServer,
    read: snob_ig::allowlist::Rest,
    query: &[(&str, &str)],
    label: &str,
) -> Value {
    let ask = Ask::Rest {
        read,
        query: query
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    };
    let doc = ask_json(tab, server, ask, "/someone/", label).await;
    assert!(
        doc.get("status").is_some(),
        "{label}: missing `status` key in fake REST answer"
    );
    doc
}

async fn replay_target(tab: &TestTab, server: &MockServer, target: Target) -> Shape {
    let someone = Pk::new(SOMEONE);
    let mut shape = Shape::new();

    match target {
        Target::Graphql(op) => {
            let doc = match op {
                Operation::ProfilePage => {
                    query_op(tab, server, op, profile_variables(someone), "/someone/").await
                }
                Operation::HoverCard => {
                    query_op(
                        tab,
                        server,
                        op,
                        format!(r#"{{"userID":"{SOMEONE}"}}"#),
                        "/someone/",
                    )
                    .await
                }
                Operation::HighlightsTray => {
                    query_op(
                        tab,
                        server,
                        op,
                        format!(r#"{{"user_id":"{SOMEONE}"}}"#),
                        "/someone/",
                    )
                    .await
                }
                Operation::HighlightsPage => {
                    let h1 = "highlight:17900000000000001".to_string();
                    let h2 = "highlight:17900000000000002".to_string();
                    query_op(
                        tab,
                        server,
                        op,
                        reel_variables(&h1, &[h1.clone(), h2]),
                        "/someone/",
                    )
                    .await
                }
                Operation::ReelGallery | Operation::ReelGalleryPage => {
                    let s = SOMEONE.to_string();
                    let u0 = FIRST_USER.to_string();
                    query_op(tab, server, op, reel_variables(&s, &[s.clone(), u0]), "/").await
                }
                Operation::StoriesTray => query_op(tab, server, op, "{}".into(), "/").await,
                Operation::NoteBubble => {
                    query_op(
                        tab,
                        server,
                        op,
                        format!(r#"{{"user_id":"{SOMEONE}"}}"#),
                        "/someone/",
                    )
                    .await
                }
                Operation::SchoolPartnerBadge => {
                    query_op(
                        tab,
                        server,
                        op,
                        format!(r#"{{"igid":"{SOMEONE}"}}"#),
                        "/someone/",
                    )
                    .await
                }
                Operation::ProfilePosts => {
                    query_op(tab, server, op, posts_variables("someone"), "/someone/").await
                }
                Operation::ProfilePostsPage => {
                    query_op(
                        tab,
                        server,
                        op,
                        posts_page_variables("someone", "posts:12"),
                        "/someone/",
                    )
                    .await
                }
                Operation::ProfileReels => {
                    query_op(tab, server, op, reels_variables(someone), "/someone/").await
                }
                Operation::ProfileReelsPage => {
                    query_op(
                        tab,
                        server,
                        op,
                        reels_page_variables(someone, "AQHTreels12"),
                        "/someone/reels/",
                    )
                    .await
                }
                Operation::ProfileTagged => {
                    query_op(tab, server, op, tagged_variables(someone), "/someone/").await
                }
                Operation::ProfileTaggedPage => {
                    let first = query_op(
                        tab,
                        server,
                        Operation::ProfileTagged,
                        tagged_variables(someone),
                        "/someone/",
                    )
                    .await;
                    let after = first
                        .pointer("/data/xdt_api__v1__usertags__user_id__feed_connection/page_info/end_cursor")
                        .and_then(Value::as_str)
                        .expect("ProfileTagged did not hand out an end_cursor");
                    query_op(
                        tab,
                        server,
                        op,
                        tagged_page_variables(someone, after),
                        "/someone/tagged/",
                    )
                    .await
                }
                Operation::ProfileReposts => {
                    query_op(tab, server, op, reposts_variables(someone), "/someone/").await
                }
                Operation::ProfileRepostsPage => {
                    query_op(
                        tab,
                        server,
                        op,
                        reposts_page_variables(someone, "AQHTreposts12"),
                        "/someone/reposts/",
                    )
                    .await
                }
                Operation::Follow | Operation::Unfollow => {
                    let mutation = op.write().expect("write mutation");
                    tab.ask(call_on(
                        server,
                        Ask::Document {
                            path: "/someone/".into(),
                        },
                        "/",
                    ))
                    .await
                    .unwrap_or_else(|e| {
                        panic!("{}: failed loading profile: {e}", op.friendly_name())
                    });
                    let doc = ask_json(
                        tab,
                        server,
                        Ask::Write {
                            mutation,
                            variables: format!(r#"{{"target_user_id":"{SOMEONE}"}}"#),
                            doc_id: op.doc_id().into(),
                            route: PROFILE_ROUTE.into(),
                        },
                        "/someone/",
                        op.friendly_name(),
                    )
                    .await;
                    assert!(
                        doc.get("data")
                            .and_then(|d| d.get(op.root_field()))
                            .is_some(),
                        "{}: missing `data.{}` in fake write answer",
                        op.friendly_name(),
                        op.root_field()
                    );
                    doc
                }
            };
            merge_value(&doc, &mut shape);
        }
        Target::Rest(rest) => {
            use snob_ig::allowlist::Rest;
            let label = rest.label();
            match rest {
                RestKind::Followers => {
                    let doc = query_rest(
                        tab,
                        server,
                        Rest::Followers(someone),
                        &[("count", "12"), ("search_surface", "follow_list_page")],
                        label,
                    )
                    .await;
                    merge_value(&doc, &mut shape);
                }
                RestKind::Following => {
                    let doc = query_rest(
                        tab,
                        server,
                        Rest::Following(someone),
                        &[("count", "6")],
                        label,
                    )
                    .await;
                    merge_value(&doc, &mut shape);
                }
                RestKind::MutualFollowers => {
                    let doc = query_rest(
                        tab,
                        server,
                        Rest::MutualFollowers(someone),
                        &[("page_size", "5")],
                        label,
                    )
                    .await;
                    merge_value(&doc, &mut shape);
                }
                RestKind::MediaInfo => {
                    // Sample both the carousel post (0) and reel post (1) so
                    // the fake shape covers all emitted media variants.
                    for post_idx in [0, 1] {
                        let media = MediaPk::new(post_pk(SOMEONE, post_idx));
                        let doc = query_rest(tab, server, Rest::MediaInfo(media), &[], label).await;
                        merge_value(&doc, &mut shape);
                    }
                }
                RestKind::Comments => {
                    let media = MediaPk::new(post_pk(SOMEONE, 0));
                    let doc = query_rest(
                        tab,
                        server,
                        Rest::Comments(media),
                        &[
                            ("can_support_threading", "true"),
                            ("permalink_enabled", "false"),
                        ],
                        label,
                    )
                    .await;
                    merge_value(&doc, &mut shape);
                }
                RestKind::Likers => {
                    let media = MediaPk::new(post_pk(SOMEONE, 0));
                    let doc = query_rest(tab, server, Rest::Likers(media), &[], label).await;
                    merge_value(&doc, &mut shape);
                }
                RestKind::Replies => {
                    let media = MediaPk::new(post_pk(SOMEONE, 0));
                    // Comment 2 has 9 counted replies (7 handed out) so both
                    // string and numeric `user.pk` variants appear.
                    let comment = CommentPk::new(18_000_000_000_000_002);
                    let doc = query_rest(
                        tab,
                        server,
                        Rest::Replies(media, comment),
                        &[
                            ("min_id", ""),
                            ("is_chronological", "true"),
                            ("paging_direction", "view_more"),
                        ],
                        label,
                    )
                    .await;
                    merge_value(&doc, &mut shape);
                }
            }
        }
    }

    shape
}

fn print_report(entry: &CapturedShape, diff: &ShapeDiff) {
    println!(
        "=== {} (samples: {}) ===",
        entry.target.display_name(),
        entry.samples
    );
    println!("  real_only ({}):", diff.real_only.len());
    for (path, kinds) in &diff.real_only {
        println!("    + {path}: {}", format_kinds(kinds));
    }
    println!("  fake_only ({}):", diff.fake_only.len());
    for (path, kinds) in &diff.fake_only {
        println!("    - {path}: {}", format_kinds(kinds));
    }
    println!("  type_mismatches ({}):", diff.type_mismatches.len());
    for (path, real_kinds, fake_kinds) in &diff.type_mismatches {
        println!(
            "    ! {path}: real=[{}] vs fake=[{}]",
            format_kinds(real_kinds),
            format_kinds(fake_kinds)
        );
    }
}

#[tokio::test]
#[ignore = "compares the fake with local captures; run by hand"]
async fn the_fake_shapes_are_compared_with_captured_answers() {
    let files = capture_files();
    if files.is_empty() {
        eprintln!("drift: no capture files found; skipping");
        return;
    }

    let captured = load_captures(&files);
    if captured.is_empty() {
        eprintln!(
            "drift: read {} file(s) with no allowlisted HTTP 200 JSON answers; skipping",
            files.len()
        );
        return;
    }

    let world = build_world();
    let server = world.serve_web().await;
    let session =
        Session::from_sessionid(TEST_SESSION_ID, TEST_USER_AGENT, SessionOrigin::Paste).unwrap();
    let tab = TestTab::new(&world, &session);

    // Replay reads before writes, since writes navigate the tab to `/someone/`.
    let mut entries: Vec<&CapturedShape> = captured.values().collect();
    entries.sort_by_key(|e| (e.target.is_write(), e.target.key()));

    let mut compared = 0usize;
    for entry in entries {
        let fake_shape = replay_target(&tab, &server, entry.target).await;
        let diff = diff_shapes(&entry.shape, &fake_shape);
        print_report(entry, &diff);
        compared += 1;
    }

    println!(
        "drift summary: compared {compared} operation(s) across {} capture file(s)",
        files.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shape_extraction_collapses_arrays_and_collects_all_scalar_kinds() {
        let mut shape = Shape::new();
        let a = json!({
            "data": {
                "user": { "pk": 1, "active": true, "bio": null },
                "edges": [{ "node": { "id": "10" } }, { "node": { "id": 11 } }],
                "empty": []
            }
        });
        let b = json!({
            "data": {
                "user": null,
                "edges": []
            }
        });
        merge_value(&a, &mut shape);
        merge_value(&b, &mut shape);

        assert_eq!(
            shape["data.user"],
            BTreeSet::from([Kind::Object, Kind::Null])
        );
        assert_eq!(shape["data.user.pk"], BTreeSet::from([Kind::Number]));
        assert_eq!(shape["data.user.active"], BTreeSet::from([Kind::Bool]));
        assert_eq!(shape["data.user.bio"], BTreeSet::from([Kind::Null]));
        assert_eq!(
            shape["data.edges[].node.id"],
            BTreeSet::from([Kind::String, Kind::Number])
        );
        assert_eq!(shape["data.empty"], BTreeSet::from([Kind::Array]));
        assert!(!shape.contains_key("data.empty[]"));
    }

    #[test]
    fn shape_diff_separates_added_missing_and_disjoint_non_null_types() {
        let mut real = Shape::new();
        merge_value(
            &json!({ "pk": "42", "bio": "hi", "extra": 1, "overlap": "ok" }),
            &mut real,
        );
        merge_value(&json!({ "overlap": 2 }), &mut real);

        let mut fake = Shape::new();
        merge_value(
            &json!({ "pk": 42, "bio": null, "invented": true, "overlap": "ok" }),
            &mut fake,
        );

        let diff = diff_shapes(&real, &fake);
        assert_eq!(diff.real_only.len(), 1);
        assert_eq!(diff.real_only[0].0, "extra");
        assert_eq!(diff.fake_only.len(), 1);
        assert_eq!(diff.fake_only[0].0, "invented");
        assert_eq!(diff.type_mismatches.len(), 1);
        assert_eq!(diff.type_mismatches[0].0, "pk");
    }

    #[test]
    fn documents_parser_strips_for_prefix_and_skips_deferred_patches() {
        let mut shape = Shape::new();
        let body = concat!(
            "for (;;);{\"data\":{\"user\":{\"pk\":\"1\"}},\"extensions\":{\"is_final\":false}}\n",
            "{\"data\":{\"extra\":true},\"path\":[\"user\"]}\n",
            "{\"errors\":[{\"message\":\"fail\"}]}\n",
            "{\"data\":null,\"errors\":[{\"message\":\"fail\"}]}"
        );
        let docs = extract_graphql_body_shape(body, &mut shape);
        assert_eq!(docs, 1);
        assert!(shape.contains_key("data.user.pk"));
        assert!(!shape.contains_key("data.extra"));
        assert!(!shape.contains_key("errors"));
    }

    #[test]
    fn allowlist_classifier_keeps_only_status_200_operations_and_rest_reads() {
        let ok_gql = json!({
            "kind": "api",
            "method": "POST",
            "url": "https://www.instagram.com/api/graphql",
            "status": 200,
            "postData": format!("doc_id={}&fb_api_req_friendly_name=PolarisProfilePageContentQuery", Operation::ProfilePage.doc_id())
        });
        assert_eq!(
            classify_event(&ok_gql),
            Some(Target::Graphql(Operation::ProfilePage))
        );

        let ok_by_doc_id = json!({
            "kind": "fetch",
            "method": "POST",
            "url": "https://www.instagram.com/graphql/query",
            "status": 200,
            "postData": format!("doc_id={}", Operation::HighlightsPage.doc_id())
        });
        assert_eq!(
            classify_event(&ok_by_doc_id),
            Some(Target::Graphql(Operation::HighlightsPage))
        );

        let non_200 = json!({
            "kind": "api",
            "method": "POST",
            "url": "https://www.instagram.com/api/graphql",
            "status": 500,
            "postData": "fb_api_req_friendly_name=PolarisProfilePageContentQuery"
        });
        assert_eq!(classify_event(&non_200), None);

        let app_noise = json!({
            "kind": "api",
            "method": "POST",
            "url": "https://www.instagram.com/api/graphql",
            "status": 200,
            "postData": "fb_api_req_friendly_name=QuickPromotionSupportIGSchemaBatchFetchQuery"
        });
        assert_eq!(classify_event(&app_noise), None);

        let rest = json!({
            "kind": "api",
            "method": "GET",
            "url": "https://www.instagram.com/api/v1/media/3800000000000000000_9001/comments/18000000000000002/child_comments/?min_id=",
            "status": 200
        });
        assert_eq!(classify_event(&rest), Some(Target::Rest(RestKind::Replies)));
    }

    #[test]
    fn capture_discovery_handles_missing_single_file_and_ordered_runs() {
        struct ScratchDir(PathBuf);
        impl Drop for ScratchDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = ScratchDir(
            std::env::temp_dir().join(format!("snob-drift-test-{}", std::process::id())),
        );
        let _ = std::fs::remove_dir_all(&dir.0);
        assert!(collect_capture_files(&dir.0).is_empty());

        std::fs::create_dir_all(dir.0.join("2026-11-14")).unwrap();
        std::fs::create_dir_all(dir.0.join("2026-11-13")).unwrap();
        std::fs::create_dir_all(dir.0.join("empty")).unwrap();
        let f1 = dir.0.join("2026-11-13").join("events.jsonl");
        let f2 = dir.0.join("2026-11-14").join("events.jsonl");
        std::fs::write(&f1, "").unwrap();
        std::fs::write(&f2, "").unwrap();

        assert_eq!(collect_capture_files(&dir.0), vec![f1.clone(), f2]);
        assert_eq!(collect_capture_files(&f1), vec![f1]);
    }
}
