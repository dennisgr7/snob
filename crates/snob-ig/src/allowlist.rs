//! The calls snob may send from the page, and the check that refuses the rest
//! before they leave. See "Talking to Instagram" in `AGENTS.md`.
//!
//! **An allowlist, because the app's own writes cannot be listed**: its seen
//! signals, view counts and search history go out under names and numbers
//! that change. The registry is [`Operation`], [`Rest`], the route
//! definitions, [`crate::web::NAVIGATION`] and [`crate::web::SHOW_MANY`]; call
//! sites pass these values, never a path or a name.
//!
//! - [`refused`] reads a request itself: a registry POST on its own endpoint
//!   under its own `doc_id` (a write by name and endpoint alone, since its
//!   number rotates); a GET of a [`Rest`] read; a navigation to the home page
//!   or a profile, never to one of the app's screens.
//! - [`refused_ask`] holds an intent ([`crate::web::Ask`]) to the same
//!   registry before it is handed over and again where it is built.
//! - **A read's `doc_id` is pinned and follows the app when it moves**:
//!   [`vouched`] checks the app's own call and [`Rediscovered`] holds its
//!   number, which [`refused_with`] then lets out.
//! - [`refused_app`] is for the app's own traffic in the tab: the reads of
//!   `APP_READS` beyond the registry's, and **never a write, under any
//!   number**.

use std::collections::HashMap;

use snob_core::Pk;
use url::Url;

use crate::client::page::{Method, PageRequest};
use crate::graphql::Mutation;
use crate::shortcode::{CommentPk, MediaPk};
use crate::web::{Ask, BULK_ROUTE_DEFINITIONS, NAVIGATION, SHOW_MANY};

/// Which Relay endpoint an operation is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `POST /api/graphql`.
    Relay,
    /// `POST /graphql/query`, which names the answer's root field in a header.
    RelayQuery,
}

/// A Relay operation snob may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Operation {
    /// A profile's header, by pk.
    ProfilePage,
    /// A profile's counters, by pk: the cheapest header read.
    HoverCard,
    /// A profile's highlights, without their items.
    HighlightsTray,
    /// The items of highlights.
    HighlightsPage,
    /// The stories of the reels in a tray.
    ReelGallery,
    ReelGalleryPage,
    /// Which reels there are, in the order the gallery takes them.
    StoriesTray,
    /// The note over a profile's picture: sent with every profile opened
    /// (`IgClient::profile_burst`), its answer not read.
    NoteBubble,
    /// The school badge: the other read of that burst.
    SchoolPartnerBadge,
    /// The first page of a profile's grid, by name.
    ProfilePosts,
    ProfilePostsPage,
    /// A profile's reels tab, by pk, and its next page.
    ProfileReels,
    ProfileReelsPage,
    /// The posts a profile is tagged in, by pk, and their next page.
    ProfileTagged,
    ProfileTaggedPage,
    /// What a profile reposted, by pk, and its next page.
    ProfileReposts,
    ProfileRepostsPage,
    Follow,
    Unfollow,
}

impl Operation {
    pub const ALL: [Self; 19] = [
        Self::ProfilePage,
        Self::HoverCard,
        Self::HighlightsTray,
        Self::HighlightsPage,
        Self::ReelGallery,
        Self::ReelGalleryPage,
        Self::StoriesTray,
        Self::NoteBubble,
        Self::SchoolPartnerBadge,
        Self::ProfilePosts,
        Self::ProfilePostsPage,
        Self::ProfileReels,
        Self::ProfileReelsPage,
        Self::ProfileTagged,
        Self::ProfileTaggedPage,
        Self::ProfileReposts,
        Self::ProfileRepostsPage,
        Self::Follow,
        Self::Unfollow,
    ];

    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.friendly_name() == name)
    }

    pub fn friendly_name(self) -> &'static str {
        match self {
            Self::ProfilePage => "PolarisProfilePageContentQuery",
            Self::HoverCard => "PolarisUserHoverCardContentV2Query",
            Self::HighlightsTray => "PolarisProfileStoryHighlightsTrayContentQuery",
            Self::HighlightsPage => "PolarisStoriesV3HighlightsPageQuery",
            Self::ReelGallery => "PolarisStoriesV3ReelPageGalleryQuery",
            Self::ReelGalleryPage => "PolarisStoriesV3ReelPageGalleryPaginationQuery",
            Self::StoriesTray => "PolarisStoriesV3TrayContainerQuery",
            Self::NoteBubble => "PolarisProfileNoteBubbleQuery",
            Self::SchoolPartnerBadge => "PolarisSchoolPartnerProfileBadgeQuery",
            Self::ProfilePosts => "PolarisProfilePostsQuery",
            Self::ProfilePostsPage => "PolarisProfilePostsTabContentQuery_connection",
            Self::ProfileReels => "PolarisProfileReelsTabContentQuery",
            Self::ProfileReelsPage => "PolarisProfileReelsTabContentQuery_connection",
            Self::ProfileTagged => "PolarisProfileTaggedTabContentQuery",
            Self::ProfileTaggedPage => "PolarisProfileTaggedTabContentQuery_connection",
            Self::ProfileReposts => "PolarisProfileRepostsTabContentWWWQuery",
            Self::ProfileRepostsPage => "PolarisProfileRepostsTabContentWWWRefetchQuery",
            Self::Follow => Mutation::Follow.friendly_name(),
            Self::Unfollow => Mutation::Unfollow.friendly_name(),
        }
    }

    /// The operation's `doc_id`, as captured: 2026-09-29 (unchanged on
    /// 2026-10-01), except the profile's header, the stories tray, the
    /// gallery's two and the grid's two, from 2026-11-13. A write's is the
    /// seed its discovery starts from.
    ///
    /// A moved one goes out under the number the app's own call vouches for
    /// ([`vouched`]). **The grid's next page and the six of the reels, tagged
    /// and reposts tabs never are**: the app does not send them from a
    /// document the tab loads, so they go out under these numbers alone
    /// (captures of 2026-11-13 and 2026-11-14) and stop answering when one
    /// moves (AGENTS.md, "Open questions").
    pub fn doc_id(self) -> &'static str {
        match self {
            Self::ProfilePage => "40158838120366999",
            Self::HoverCard => "28949219061332276",
            Self::HighlightsTray => "26970053832668570",
            Self::HighlightsPage => "28325328583775973",
            Self::ReelGallery => "29088280257466613",
            Self::ReelGalleryPage => "29455599790792632",
            Self::StoriesTray => "38657722893875056",
            Self::NoteBubble => "38260824646898178",
            Self::SchoolPartnerBadge => "28647748031575520",
            Self::ProfilePosts => "29114630661562380",
            Self::ProfilePostsPage => "29290744380560118",
            Self::ProfileReels => "28280418711631396",
            Self::ProfileReelsPage => "28237651492507044",
            Self::ProfileTagged => "28196060226733657",
            Self::ProfileTaggedPage => "29256185780680839",
            Self::ProfileReposts => "29116356611389607",
            Self::ProfileRepostsPage => "29259241890370275",
            Self::Follow => Mutation::Follow.seed_doc_id(),
            Self::Unfollow => Mutation::Unfollow.seed_doc_id(),
        }
    }

    /// The endpoint the app sends it to. It moves one now and then: the
    /// grid's two went from `/graphql/query` to `/api/graphql` between the
    /// captures of 2026-10-01 and 2026-11-13 (13 calls of 13).
    pub fn family(self) -> Family {
        match self {
            Self::HighlightsPage | Self::ReelGallery | Self::ReelGalleryPage => Family::RelayQuery,
            _ => Family::Relay,
        }
    }

    pub fn path(self) -> &'static str {
        match self.family() {
            Family::RelayQuery => "/graphql/query",
            _ => "/api/graphql",
        }
    }

    /// The field of `data` the answer is under, as the recorded app's answers
    /// came; on `/graphql/query` it is also sent, as `X-Root-Field-Name`.
    pub fn root_field(self) -> &'static str {
        match self {
            Self::ProfilePage => "user",
            Self::HoverCard => "xig_user_by_igid_v2",
            Self::HighlightsTray => "highlights",
            Self::HighlightsPage | Self::ReelGallery | Self::ReelGalleryPage => {
                "xdt_api__v1__feed__reels_media__connection"
            }
            Self::StoriesTray => "xdt_api__v1__feed__reels_tray",
            Self::NoteBubble => "xdt_get_inbox_tray_items",
            Self::SchoolPartnerBadge => "xig_user_by_igid_v2",
            Self::ProfilePosts | Self::ProfilePostsPage => {
                "xdt_api__v1__feed__user_timeline_graphql_connection"
            }
            // Captures of 2026-11-13 and 2026-11-14, every call.
            Self::ProfileReels
            | Self::ProfileReelsPage
            | Self::ProfileReposts
            | Self::ProfileRepostsPage => "fetch__XDTUserDict",
            Self::ProfileTagged | Self::ProfileTaggedPage => {
                "xdt_api__v1__usertags__user_id__feed_connection"
            }
            Self::Follow => "xdt_create_friendship",
            Self::Unfollow => "xdt_destroy_friendship",
        }
    }

    pub fn write(self) -> Option<Mutation> {
        match self {
            Self::Follow => Some(Mutation::Follow),
            Self::Unfollow => Some(Mutation::Unfollow),
            _ => None,
        }
    }

    pub fn of_write(mutation: Mutation) -> Self {
        match mutation {
            Mutation::Follow => Self::Follow,
            Mutation::Unfollow => Self::Unfollow,
        }
    }

    /// The keys of the variables that name what is asked, in the app's order;
    /// the first tells the operation's call from another's.
    pub fn asked_keys(self) -> &'static [&'static str] {
        match self {
            Self::ProfilePage => &["id"],
            Self::HoverCard => &["userID"],
            Self::HighlightsTray | Self::NoteBubble => &["user_id"],
            Self::SchoolPartnerBadge => &["igid"],
            Self::HighlightsPage | Self::ReelGallery => {
                &["initial_reel_id", "reel_ids", "first", "last"]
            }
            Self::ReelGalleryPage => &[
                "initial_reel_id",
                "after",
                "before",
                "first",
                "is_highlight",
                "last",
                "reel_ids",
            ],
            Self::StoriesTray => &["data", "suggestedUsersData"],
            Self::ProfilePosts => &["username"],
            Self::ProfilePostsPage => &["after", "before", "first", "last", "username"],
            Self::ProfileReels => &["data", "user_id"],
            Self::ProfileReelsPage => &["after", "data", "first", "id"],
            Self::ProfileTagged => &["count", "user_id"],
            Self::ProfileTaggedPage => &["after", "before", "count", "first", "last", "user_id"],
            Self::ProfileReposts => &["user_id"],
            Self::ProfileRepostsPage => &["max_id", "id"],
            Self::Follow | Self::Unfollow => &["target_user_id"],
        }
    }
}

/// Whether the app's own call of the read `operation` under `doc_id` lets
/// that number stand in for the registry's: a read, on its own endpoint,
/// whose variables carry the key that names what it asks, and whose
/// `X-Root-Field-Name` is the operation's on `/graphql/query` and absent on
/// `/api/graphql`.
pub fn vouched(
    operation: Operation,
    path: &str,
    doc_id: &str,
    variables: &str,
    root_field: Option<&str>,
) -> bool {
    let asked = operation.asked_keys().first().copied();
    let named = serde_json::from_str::<serde_json::Value>(variables)
        .ok()
        .is_some_and(|variables| asked.is_some_and(|key| variables.get(key).is_some()));
    let root_ok = match operation.family() {
        Family::RelayQuery => root_field == Some(operation.root_field()),
        Family::Relay => root_field.is_none(),
    };
    operation.write().is_none() && path == operation.path() && is_doc_id(doc_id) && named && root_ok
}

/// Digits, and as many as Instagram's have ever been.
fn is_doc_id(doc_id: &str) -> bool {
    (10..=20).contains(&doc_id.len()) && doc_id.bytes().all(|b| b.is_ascii_digit())
}

/// The numbers the app was seen to send the registry's reads under, when they
/// are not the registry's. Filled only from calls [`vouched`] held for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rediscovered(HashMap<Operation, String>);

impl Rediscovered {
    pub fn of(operation: Operation, doc_id: &str) -> Self {
        let mut found = Self::default();
        found.insert(operation, doc_id);
        found
    }

    /// Ignores a write (its number is found in the app's code) and anything
    /// that is not a `doc_id`.
    pub fn insert(&mut self, operation: Operation, doc_id: &str) {
        if operation.write().is_none() && is_doc_id(doc_id) {
            self.0.insert(operation, doc_id.to_string());
        }
    }

    pub fn get(&self, operation: Operation) -> Option<&str> {
        self.0.get(&operation).map(String::as_str)
    }
}

/// A REST read snob may send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Rest {
    Followers(Pk),
    Following(Pk),
    MutualFollowers(Pk),
    /// A post, by its pk: asked the moment one opens (about twenty times in
    /// the captures of 2026-10-01, always beside [`Rest::Comments`]).
    MediaInfo(MediaPk),
    /// A page of a post's comments; the next ones by the `min_id` handed out.
    Comments(MediaPk),
    /// The accounts that liked a post: no query, one answer of 105 to 125
    /// accounts with no cursor, a sample (captures of 2026-11-13 and
    /// 2026-11-14, where the list scrolled to its end asked for no more).
    Likers(MediaPk),
    /// The replies under a comment (7 of 7 over the captures of 2026-11-13
    /// and 2026-11-14, always the same three keys). **The first page only**:
    /// no cursor has been seen, and `min_id` is held empty.
    Replies(MediaPk, CommentPk),
}

impl Rest {
    pub fn path(self) -> String {
        match self {
            Self::Followers(pk) => format!("/api/v1/friendships/{pk}/followers/"),
            Self::Following(pk) => format!("/api/v1/friendships/{pk}/following/"),
            Self::MutualFollowers(pk) => format!("/api/v1/friendships/{pk}/mutual_followers/"),
            Self::MediaInfo(pk) => format!("/api/v1/media/{pk}/info/"),
            Self::Comments(pk) => format!("/api/v1/media/{pk}/comments/"),
            Self::Likers(pk) => format!("/api/v1/media/{pk}/likers/"),
            Self::Replies(pk, comment) => {
                format!("/api/v1/media/{pk}/comments/{comment}/child_comments/")
            }
        }
    }

    /// The query keys the app sends, and the one value a key is held to.
    fn keys(self) -> &'static [(&'static str, Option<&'static str>)] {
        match self {
            Self::Followers(_) => &[
                ("count", None),
                ("max_id", None),
                ("search_surface", Some("follow_list_page")),
            ],
            Self::Following(_) => &[("count", None), ("max_id", None)],
            Self::MutualFollowers(_) => &[("page_size", None), ("max_id", None)],
            Self::MediaInfo(_) | Self::Likers(_) => &[],
            Self::Comments(_) => &[
                ("can_support_threading", Some("true")),
                ("permalink_enabled", Some("false")),
                ("min_id", None),
                ("sort_order", Some("popular")),
            ],
            // Each held to the one value the app sent, the cursor empty.
            Self::Replies(..) => &[
                ("min_id", Some("")),
                ("is_chronological", Some("true")),
                ("paging_direction", Some("view_more")),
            ],
        }
    }

    /// Whether the read goes out with every one of its keys, in order, and no
    /// other.
    fn takes_every_key(self) -> bool {
        matches!(self, Self::Replies(..))
    }
}

fn posted(request: &PageRequest, path: &str, found: &Rediscovered) -> bool {
    match path {
        BULK_ROUTE_DEFINITIONS => true,
        NAVIGATION => navigation(request),
        SHOW_MANY => show_many(request),
        _ => operation_posted(request, path, found).is_some(),
    }
}

fn form_of(request: &PageRequest) -> Vec<(String, String)> {
    url::form_urlencoded::parse(request.body.as_deref().unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

/// Whether `request` is the app's navigation to a route snob may be on: one
/// `route_url`, a [`list_route`]. The app's navigations to its own screens,
/// the story viewer among them, stay refused.
fn navigation(request: &PageRequest) -> bool {
    let form = form_of(request);
    let mut routes = form.iter().filter(|(name, _)| name == "route_url");
    let route = routes.next();
    routes.next().is_none() && route.is_some_and(|(_, route)| list_route(route))
}

/// More than a list page's twelve, and the fifty a list endpoint serves at most.
pub const MOST_STATUSES: usize = 50;

/// Whether `request` is `friendships/show_many` as the app sends it after a
/// list page (16 of 16 in the capture of 2026-10-01): `user_ids`, `jazoest`,
/// `fb_dtsg`, and nothing else.
///
/// **A read, though it is a POST**: it changes nothing, is paid as a read,
/// and can never reach the write bucket. A write is a
/// [`crate::graphql::Mutation`] and nothing else.
fn show_many(request: &PageRequest) -> bool {
    let form = form_of(request);
    let names: Vec<&str> = form.iter().map(|(name, _)| name.as_str()).collect();
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    names == ["user_ids", "jazoest", "fb_dtsg"]
        && field("user_ids").is_some_and(|ids| statuses_of(ids).is_some())
        && field("jazoest").is_some_and(|j| !j.is_empty() && j.bytes().all(|b| b.is_ascii_digit()))
}

fn statuses_of(ids: &str) -> Option<usize> {
    let count = ids.split(',').count();
    let pks = ids
        .split(',')
        .all(|pk| !pk.is_empty() && pk.len() <= 20 && pk.bytes().all(|b| b.is_ascii_digit()));
    (pks && (1..=MOST_STATUSES).contains(&count)).then_some(count)
}

/// Whether `route` is one snob may navigate the app to: the home page, a
/// profile ([`document`]), `/<name>/followers/mutualOnly`, or one of
/// [`PROFILE_TABS`] (6 of 6 in the capture of 2026-11-14). Never one of the
/// app's screens: `/reels/` alone is a screen.
pub fn list_route(route: &str) -> bool {
    if document(route) {
        return true;
    }
    let under_a_profile = |tail: &str| {
        route
            .strip_suffix(tail)
            .is_some_and(|profile| profile != "/" && document(profile))
    };
    under_a_profile("followers/mutualOnly")
        || PROFILE_TABS
            .iter()
            .any(|tab| under_a_profile(&format!("{tab}/")))
}

/// The tabs of a profile snob reads besides its grid: `/<name>/<tab>/`.
pub const PROFILE_TABS: [&str; 3] = ["reels", "tagged", "reposts"];

fn operation_posted(request: &PageRequest, path: &str, found: &Rediscovered) -> Option<Operation> {
    let form: Vec<(String, String)> =
        url::form_urlencoded::parse(request.body.as_deref()?.as_bytes())
            .into_owned()
            .collect();
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let name = field("fb_api_req_friendly_name")?;
    let operation = Operation::named(name)?;
    let announced = request
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("x-fb-friendly-name"))
        .map(|(_, v)| v.as_str());
    let doc_id = field("doc_id")?;
    let holds = operation.path() == path
        && announced == Some(name)
        && (operation.write().is_some()
            || doc_id == operation.doc_id()
            || found.get(operation) == Some(doc_id));
    holds.then_some(operation)
}

/// The app's own screens, which a profile's name can never be. Some mark what
/// they show as seen the moment it is shown.
const SCREENS: [&str; 5] = ["stories", "explore", "direct", "reels", "accounts"];

/// One to thirty ASCII letters, digits, dots and underscores: every name
/// Instagram hands out. A percent-encoded name is no profile's.
pub fn profile_name(name: &str) -> bool {
    (1..=30).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
}

/// What `ask` is, when the page must not be asked it: only the method and the
/// path, so a refused `Pk` does not print its name.
pub fn refused_ask(ask: &Ask) -> Option<String> {
    match ask {
        Ask::Viewer | Ask::Tray => None,
        Ask::Query { operation, .. } => operation
            .write()
            .is_some()
            .then(|| format!("POST {}", operation.path())),
        Ask::Write { .. } => None,
        // The host is [`refused_asset`]'s to judge, where the site is known.
        Ask::Asset { url } => Url::parse(url)
            .map_or(true, |url| !matches!(url.scheme(), "http" | "https"))
            .then(|| "a download from an address that is not a web one".to_string()),
        Ask::Pk { name } => (!profile_name(name)).then(|| format!("POST {BULK_ROUTE_DEFINITIONS}")),
        Ask::Navigation { route } => (!list_route(route)).then(|| format!("POST {NAVIGATION}")),
        Ask::Statuses { pks } => {
            (!(1..=MOST_STATUSES).contains(&pks.len())).then(|| format!("POST {SHOW_MANY}"))
        }
        Ask::Document { path } => (path.contains('?') || !document(path))
            .then(|| format!("a navigation to {}", path.escape_debug())),
        Ask::Rest { read, query } => {
            let keys = read.keys();
            let held = query.iter().all(|(name, value)| {
                keys.iter()
                    .any(|(key, only)| key == name && only.is_none_or(|only| only == value))
            });
            let whole = !read.takes_every_key()
                || query
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .eq(keys.iter().map(|(key, _)| *key));
            (!(held && whole)).then(|| format!("GET {}", read.path()))
        }
    }
}

/// Why the tab must not fetch `url` as an asset of the site at `origin`. The
/// one copy of where a file may come from, asked by the client and again by
/// the tab.
///
/// **HTTPS, and one of the two hosts Instagram serves media from** (the
/// leading dot stops `evilcdninstagram.com`), or exactly the site's own
/// scheme, host and port when the site is not Instagram (a test's server).
/// Never Instagram itself: its API is not a file to read unpaced.
pub fn refused_asset(origin: &str, url: &str) -> Option<String> {
    const CDN_HOSTS: [&str; 2] = ["cdninstagram.com", "fbcdn.net"];

    let refusal = |url: &Url| {
        Some(format!(
            "a download from {}",
            url.host_str().unwrap_or("nowhere")
        ))
    };
    let Ok(url) = Url::parse(url) else {
        return Some("a download from an address that does not parse".into());
    };
    let instagram = |host: &str| host == "instagram.com" || host.ends_with(".instagram.com");
    if Url::parse(origin).is_ok_and(|origin| {
        !origin.host_str().is_some_and(instagram)
            && origin.scheme() == url.scheme()
            && origin.host_str() == url.host_str()
            && origin.port_or_known_default() == url.port_or_known_default()
    }) {
        return None;
    }
    let cdn = url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            CDN_HOSTS
                .iter()
                .any(|cdn| host == *cdn || host.ends_with(&format!(".{cdn}")))
        });
    if cdn { None } else { refusal(&url) }
}

/// What `request` is, when the page must not send it. Only the method and the
/// path: the rest can carry tokens.
pub fn refused(request: &PageRequest) -> Option<String> {
    refused_with(request, &Rediscovered::default())
}

/// [`refused`], also letting a read out under a number in `found`.
pub fn refused_with(request: &PageRequest, found: &Rediscovered) -> Option<String> {
    let method = match request.method {
        Method::Get if request.navigate => "a navigation to",
        Method::Get => "GET",
        Method::Post => "POST",
    };
    let Ok(url) = Url::parse(&request.url) else {
        return Some(format!("{method} an address that does not parse"));
    };
    let path = url.path();
    let allowed = match (request.method, request.navigate) {
        (Method::Post, false) => posted(request, path, found),
        (Method::Get, false) => rest_read(path),
        (Method::Get, true) => url.query().is_none() && document(path),
        (Method::Post, true) => false,
    };
    (!allowed).then(|| format!("{method} {path}"))
}

/// A read the app sends on its own as a document loads, which the tab lets
/// through and snob never sends.
struct AppRead {
    name: &'static str,
    path: &'static str,
    number: Number,
}

enum Number {
    Pinned(&'static str),
    /// Any number the call vouches for as [`vouched`] would: the asked key
    /// among its variables, and `X-Root-Field-Name` as its endpoint has it.
    Vouched {
        root_field: Option<&'static str>,
        asked_key: &'static str,
    },
}

/// The app's own boot reads the tab lets out beyond the registry's: the
/// promotions, the two unread counts, and what a profile asks about the
/// account it shows, its posts and suggestions. None writes or says what was
/// looked at. (Refused, a profile the tab loaded was one whose app never
/// asked for its grid, which no person's browser is; decided 2026-10-01.)
///
/// **Vouched, not pinned, where the number moves**: the posts' moved on
/// 2026-10-01 and 2026-11-13, the promotions' and the counts' by 2026-11-13
/// (keys seen on all 29 and 4 calls of that capture).
///
/// **Left out on purpose**: the inbox and thread reads, the share sheet, the
/// search boxes, the sync with Facebook, and the beacons (`/ajax/bz`,
/// `/ajax/qm/`), which batch the app's impressions.
const APP_READS: [AppRead; 7] = [
    AppRead {
        name: "QuickPromotionSupportIGSchemaBatchFetchQuery",
        path: "/api/graphql",
        number: Number::Vouched {
            root_field: None,
            asked_key: "surface_nux_ids",
        },
    },
    AppRead {
        name: "IGDBadgeCountOffMsysQuery",
        path: "/api/graphql",
        number: Number::Vouched {
            root_field: None,
            asked_key: "device_id_for_iris_subscription",
        },
    },
    AppRead {
        name: "IGDChatTabsJewelOffMsysQuery",
        path: "/api/graphql",
        number: Number::Vouched {
            root_field: None,
            asked_key: "device_id_for_iris_subscription",
        },
    },
    AppRead {
        name: "PolarisProfileDirectOrPartnershipInboxMessageEligibilityQuery",
        path: "/api/graphql",
        number: Number::Pinned("26821612144133136"),
    },
    AppRead {
        name: "PolarisProfileSuggestedUsersWithPreloadableQuery",
        path: "/api/graphql",
        number: Number::Pinned("27929823133325729"),
    },
    // The app's own spelling, with the word twice.
    AppRead {
        name: "PolarisProfileSuggestedUsersWithLazyQueryQuery",
        path: "/api/graphql",
        number: Number::Pinned("28011006998510477"),
    },
    AppRead {
        name: "PolarisProfilePostsQuery",
        path: "/api/graphql",
        number: Number::Vouched {
            root_field: None,
            asked_key: "username",
        },
    },
];

/// What a request the app sent on its own is, when the tab must not let it
/// out: [`refused_with`]'s answer unless it is one of [`APP_READS`], and
/// **a follow or an unfollow whatever its number**.
pub fn refused_app(request: &PageRequest, found: &Rediscovered) -> Option<String> {
    if let Some(write) = a_write(request, found) {
        return Some(write);
    }
    let what = refused_with(request, found)?;
    (!app_read(request)).then_some(what)
}

/// `POST <path>` when `request` is one of the registry's two writes.
fn a_write(request: &PageRequest, found: &Rediscovered) -> Option<String> {
    if request.method != Method::Post || request.navigate {
        return None;
    }
    let url = Url::parse(&request.url).ok()?;
    let path = url.path();
    operation_posted(request, path, found)?.write()?;
    Some(format!("POST {path}"))
}

fn app_read(request: &PageRequest) -> bool {
    if request.method != Method::Post || request.navigate {
        return false;
    }
    let Ok(url) = Url::parse(&request.url) else {
        return false;
    };
    let Some(body) = request.body.as_deref() else {
        return false;
    };
    let form: Vec<(String, String)> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let field = |name: &str| {
        form.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let (Some(name), Some(doc_id)) = (field("fb_api_req_friendly_name"), field("doc_id")) else {
        return false;
    };
    let Some(read) = APP_READS.iter().find(|read| read.name == name) else {
        return false;
    };
    let number = match read.number {
        Number::Pinned(pinned) => doc_id == pinned,
        Number::Vouched {
            root_field,
            asked_key,
        } => {
            let asks = field("variables")
                .and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok())
                .is_some_and(|v| v.get(asked_key).is_some());
            is_doc_id(doc_id) && header("x-root-field-name") == root_field && asks
        }
    };
    url.path() == read.path && header("x-fb-friendly-name") == Some(name) && number
}

/// Whether `path` is one of [`Rest`]'s, by pks of digits. Nothing else under
/// a post: not the share link, which ties a visit to whoever shared it.
fn rest_read(path: &str) -> bool {
    let digits = |pk: &str| !pk.is_empty() && pk.bytes().all(|b| b.is_ascii_digit());
    let under = |prefix: &str, ends: &[&str]| {
        path.strip_prefix(prefix)
            .and_then(|rest| rest.split_once('/'))
            .is_some_and(|(pk, rest)| digits(pk) && ends.contains(&rest))
    };
    let replies = path
        .strip_prefix("/api/v1/media/")
        .and_then(|rest| rest.split_once("/comments/"))
        .and_then(|(pk, rest)| Some((pk, rest.strip_suffix("/child_comments/")?)))
        .is_some_and(|(pk, comment)| digits(pk) && digits(comment));
    under(
        "/api/v1/friendships/",
        &["followers/", "following/", "mutual_followers/"],
    ) || under("/api/v1/media/", &["info/", "comments/", "likers/"])
        || replies
}

/// The home page or a profile's: where the tab may go.
pub fn document(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    path.strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
        .is_some_and(|name| {
            profile_name(name) && !SCREENS.iter().any(|s| s.eq_ignore_ascii_case(name))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://www.instagram.com";

    fn request(
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> PageRequest {
        PageRequest {
            method,
            url: format!("{BASE}{path}"),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            referrer: format!("{BASE}/someone/"),
            body: body.map(str::to_string),
            navigate: false,
            cap: 1 << 20,
            timeout_ms: 30_000,
        }
    }

    fn relay(path: &str, name: &str, doc_id: &str) -> PageRequest {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("fb_dtsg", "relay:token"),
                ("fb_api_req_friendly_name", name),
                ("variables", r#"{"id":"2345678901"}"#),
                ("doc_id", doc_id),
            ])
            .finish();
        request(
            Method::Post,
            path,
            &[("X-FB-Friendly-Name", name)],
            Some(&body),
        )
    }

    /// Written out rather than derived, so changing one is a decision made
    /// here.
    #[test]
    fn the_registry_is_the_calls_the_web_client_reads_with_and_the_two_writes() {
        let listed: Vec<_> = Operation::ALL
            .map(|op| (op.friendly_name(), op.doc_id(), op.path(), op.root_field()))
            .into();
        assert_eq!(
            listed,
            [
                (
                    "PolarisProfilePageContentQuery",
                    "40158838120366999",
                    "/api/graphql",
                    "user"
                ),
                (
                    "PolarisUserHoverCardContentV2Query",
                    "28949219061332276",
                    "/api/graphql",
                    "xig_user_by_igid_v2"
                ),
                (
                    "PolarisProfileStoryHighlightsTrayContentQuery",
                    "26970053832668570",
                    "/api/graphql",
                    "highlights"
                ),
                (
                    "PolarisStoriesV3HighlightsPageQuery",
                    "28325328583775973",
                    "/graphql/query",
                    "xdt_api__v1__feed__reels_media__connection"
                ),
                (
                    "PolarisStoriesV3ReelPageGalleryQuery",
                    "29088280257466613",
                    "/graphql/query",
                    "xdt_api__v1__feed__reels_media__connection"
                ),
                (
                    "PolarisStoriesV3ReelPageGalleryPaginationQuery",
                    "29455599790792632",
                    "/graphql/query",
                    "xdt_api__v1__feed__reels_media__connection"
                ),
                (
                    "PolarisStoriesV3TrayContainerQuery",
                    "38657722893875056",
                    "/api/graphql",
                    "xdt_api__v1__feed__reels_tray"
                ),
                (
                    "PolarisProfileNoteBubbleQuery",
                    "38260824646898178",
                    "/api/graphql",
                    "xdt_get_inbox_tray_items"
                ),
                (
                    "PolarisSchoolPartnerProfileBadgeQuery",
                    "28647748031575520",
                    "/api/graphql",
                    "xig_user_by_igid_v2"
                ),
                (
                    "PolarisProfilePostsQuery",
                    "29114630661562380",
                    "/api/graphql",
                    "xdt_api__v1__feed__user_timeline_graphql_connection"
                ),
                (
                    "PolarisProfilePostsTabContentQuery_connection",
                    "29290744380560118",
                    "/api/graphql",
                    "xdt_api__v1__feed__user_timeline_graphql_connection"
                ),
                (
                    "PolarisProfileReelsTabContentQuery",
                    "28280418711631396",
                    "/api/graphql",
                    "fetch__XDTUserDict"
                ),
                (
                    "PolarisProfileReelsTabContentQuery_connection",
                    "28237651492507044",
                    "/api/graphql",
                    "fetch__XDTUserDict"
                ),
                (
                    "PolarisProfileTaggedTabContentQuery",
                    "28196060226733657",
                    "/api/graphql",
                    "xdt_api__v1__usertags__user_id__feed_connection"
                ),
                (
                    "PolarisProfileTaggedTabContentQuery_connection",
                    "29256185780680839",
                    "/api/graphql",
                    "xdt_api__v1__usertags__user_id__feed_connection"
                ),
                (
                    "PolarisProfileRepostsTabContentWWWQuery",
                    "29116356611389607",
                    "/api/graphql",
                    "fetch__XDTUserDict"
                ),
                (
                    "PolarisProfileRepostsTabContentWWWRefetchQuery",
                    "29259241890370275",
                    "/api/graphql",
                    "fetch__XDTUserDict"
                ),
                (
                    "usePolarisFollowMutation",
                    "26508036048874888",
                    "/api/graphql",
                    "xdt_create_friendship"
                ),
                (
                    "usePolarisUnfollowMutation",
                    "27789106940691111",
                    "/api/graphql",
                    "xdt_destroy_friendship"
                ),
            ]
        );
        // Every read is a query, the grid's next page under the app's name
        // for a query's connection, and the only writes are the two there are.
        for operation in Operation::ALL {
            let name = operation.friendly_name();
            assert_eq!(
                operation.write().is_some(),
                !(name.ends_with("Query") || name.ends_with("Query_connection")),
                "{name}"
            );
        }
        assert_eq!(
            Operation::ALL
                .iter()
                .filter_map(|op| op.write())
                .collect::<Vec<_>>(),
            Mutation::ALL
        );

        let pk = Pk::new(2_345_678_901);
        let rest = [
            (
                Rest::Followers(pk),
                "/api/v1/friendships/2345678901/followers/",
            ),
            (
                Rest::Following(pk),
                "/api/v1/friendships/2345678901/following/",
            ),
            (
                Rest::MutualFollowers(pk),
                "/api/v1/friendships/2345678901/mutual_followers/",
            ),
            (
                Rest::MediaInfo(MediaPk::new(3_807_824_420_233_075_826)),
                "/api/v1/media/3807824420233075826/info/",
            ),
            (
                Rest::Comments(MediaPk::new(3_807_824_420_233_075_826)),
                "/api/v1/media/3807824420233075826/comments/",
            ),
            (
                Rest::Likers(MediaPk::new(3_807_824_420_233_075_826)),
                "/api/v1/media/3807824420233075826/likers/",
            ),
            (
                Rest::Replies(
                    MediaPk::new(3_807_824_420_233_075_826),
                    CommentPk::new(18_012_345_678_901_234),
                ),
                "/api/v1/media/3807824420233075826/comments/18012345678901234/child_comments/",
            ),
        ];
        for (read, path) in rest {
            assert_eq!(read.path(), path);
        }
    }

    /// Each operation is found by the name the app gives it, and a name the
    /// registry does not hold finds nothing.
    #[test]
    fn an_operation_is_found_by_its_friendly_name() {
        for operation in Operation::ALL {
            assert_eq!(Operation::named(operation.friendly_name()), Some(operation));
        }
        assert_eq!(Operation::named("SomeBadgeQuery"), None);
        assert_eq!(Operation::named(""), None);
    }

    #[test]
    fn every_operation_of_the_registry_goes_out_on_its_own_endpoint() {
        for operation in Operation::ALL {
            let sent = relay(
                operation.path(),
                operation.friendly_name(),
                operation.doc_id(),
            );
            assert_eq!(refused(&sent), None, "{operation:?}");
            let other = match operation.family() {
                Family::RelayQuery => "/api/graphql",
                _ => "/graphql/query",
            };
            let elsewhere = relay(other, operation.friendly_name(), operation.doc_id());
            assert!(refused(&elsewhere).is_some(), "{operation:?} on {other}");
        }
    }

    /// Anything the app sends that is not in the registry, whatever it is
    /// called: made-up names, as the real ones are not written here.
    #[test]
    fn a_relay_call_not_in_the_registry_is_refused() {
        for (name, doc_id) in [
            ("PolarisSomethingElseQuery", "1000000000000001"),
            ("usePolarisSomethingMutation", "1000000000000002"),
        ] {
            let sent = relay("/api/graphql", name, doc_id);
            assert_eq!(refused(&sent).as_deref(), Some("POST /api/graphql"));
        }
    }

    /// A known name with another operation's number is another operation.
    #[test]
    fn a_query_goes_out_only_with_its_own_doc_id() {
        let profile = Operation::ProfilePage;
        let sent = relay("/api/graphql", profile.friendly_name(), "1000000000000001");
        assert!(refused(&sent).is_some());
        let sent = relay(
            "/api/graphql",
            profile.friendly_name(),
            Operation::HoverCard.doc_id(),
        );
        assert!(refused(&sent).is_some());
    }

    /// A write's number rotates and is rediscovered beside its name, so the
    /// name and the endpoint are what hold it.
    #[test]
    fn a_write_goes_out_with_a_rediscovered_doc_id() {
        let follow = Operation::Follow;
        let sent = relay("/api/graphql", follow.friendly_name(), "1000000000000003");
        assert_eq!(refused(&sent), None);
    }

    /// The header and the form name the same operation, or nothing goes.
    #[test]
    fn a_relay_call_whose_header_names_another_operation_is_refused() {
        let hover = Operation::HoverCard;
        let mut sent = relay("/api/graphql", hover.friendly_name(), hover.doc_id());
        sent.headers = vec![(
            "X-FB-Friendly-Name".into(),
            "PolarisSomethingElseQuery".into(),
        )];
        assert!(refused(&sent).is_some());
        sent.headers.clear();
        assert!(refused(&sent).is_some());
        sent.body = None;
        assert!(refused(&sent).is_some());
    }

    #[test]
    fn no_other_post_goes_out() {
        let routes = request(
            Method::Post,
            BULK_ROUTE_DEFINITIONS,
            &[],
            Some("route_urls[0]=%2Fa%2F"),
        );
        assert_eq!(refused(&routes), None);
        for path in [
            "/api/v1/friendships/show_many/",
            "/api/v1/web/something/",
            "/ajax/navigation/",
            "/ajax/bz",
        ] {
            let sent = request(Method::Post, path, &[], Some("a=b"));
            assert_eq!(refused(&sent), Some(format!("POST {path}")));
        }
    }

    /// The page fetches the friendship lists and nothing else by GET: every
    /// other read is a query, so the REST reads the client sends without a
    /// browser are refused from it, a highlight's items among them.
    #[test]
    fn a_get_goes_out_only_for_a_friendship_list() {
        for path in [
            "/api/v1/friendships/2345678901/followers/?count=12&search_surface=follow_list_page",
            "/api/v1/friendships/2345678901/following/?count=12&max_id=12",
            "/api/v1/friendships/2345678901/mutual_followers/?page_size=12",
            "/api/v1/media/3807824420233075826/info/",
            "/api/v1/media/3807824420233075826/comments/?can_support_threading=true&permalink_enabled=false",
            "/api/v1/media/3807824420233075826/likers/",
            "/api/v1/media/3807824420233075826/comments/18000000000000000/child_comments/?min_id=&is_chronological=true&paging_direction=view_more",
        ] {
            assert_eq!(
                refused(&request(Method::Get, path, &[], None)),
                None,
                "{path}"
            );
        }
        for path in [
            "/api/v1/users/web_profile_info/?username=someone",
            "/api/v1/users/2345678901/info/",
            "/api/v1/feed/reels_media/?reel_ids=2345678901",
            "/api/v1/feed/reels_media/?reel_ids=highlight%3A17900000000000001",
            "/api/v1/highlights/2345678901/highlights_tray/",
            "/web/search/topsearch/?query=someone",
            "/api/v1/friendships/pending/",
            "/api/v1/friendships/show_many/",
            "/api/v1/friendships/someone/following/",
            "/api/v1/friendships/2345678901/following/extra/",
            "/api/v1/friendships//followers/",
            "/api/v1/media/3807824420233075826/permalink/",
            "/api/v1/media/3807824420233075826/likers/extra/",
            "/api/v1/media/DTYHCKvDNxy/likers/",
            "/api/v1/media/3807824420233075826/comments/abc/child_comments/",
            "/api/v1/media/3807824420233075826/comments/18000000000000000/child_comments/extra/",
            "/api/v1/media/3807824420233075826/comments/18000000000000000/",
            "/api/v1/media/3807824420233075826/comments//child_comments/",
            "/api/v1/media/DTYHCKvDNxy/comments/18000000000000000/child_comments/",
            "/api/v1/media/3807824420233075826/comments/1/2/child_comments/",
            "/api/v1/media/DTYHCKvDNxy/info/",
            "/api/v1/media//info/",
            "/api/graphql",
            "/graphql/query",
            "/ajax/bulk-route-definitions/",
        ] {
            let path_alone = path.split('?').next().unwrap();
            assert_eq!(
                refused(&request(Method::Get, path, &[], None)),
                Some(format!("GET {path_alone}")),
                "{path}"
            );
        }
    }

    #[test]
    fn the_page_goes_only_to_the_home_page_or_a_profile() {
        let navigation = |path: &str| {
            let mut sent = request(Method::Get, path, &[], None);
            sent.navigate = true;
            sent
        };
        for path in ["/", "/someone/", "/some.one_else/"] {
            assert_eq!(refused(&navigation(path)), None, "{path}");
        }
        for path in ["/Stories/", "/REELS/", "/Explore/", "/some%2Eone/"] {
            assert!(refused(&navigation(path)).is_some(), "{path}");
        }
        for path in [
            "/stories/someone/",
            "/stories/",
            "/explore/",
            "/direct/inbox/",
            "/reels/",
            "/someone/followers/",
            "/someone",
            "/someone/?next=x",
        ] {
            assert!(refused(&navigation(path)).is_some(), "{path}");
        }
        assert_eq!(
            refused(&navigation("/stories/")).as_deref(),
            Some("a navigation to /stories/")
        );
        let mut posted = navigation("/someone/");
        posted.method = Method::Post;
        assert!(refused(&posted).is_some());
        let mut nowhere = request(Method::Get, "/", &[], None);
        nowhere.url = "not an address".into();
        assert!(refused(&nowhere).is_some());
    }

    /// A screen is a screen whatever its case, and a name is only ever one
    /// Instagram could have handed out.
    #[test]
    fn a_profile_is_only_a_name_instagram_could_hand_out() {
        for name in ["someone", "a.b_c", "A1", &"a".repeat(30)] {
            assert!(profile_name(name), "{name}");
            assert!(document(&format!("/{name}/")), "{name}");
        }
        for name in [
            "",
            &"a".repeat(31),
            "some-one",
            "s\u{f6}meone",
            "some%2Fone",
            "some one",
            "..%2F",
        ] {
            assert!(!profile_name(name), "{name}");
        }
        for path in [
            "/Stories/",
            "/REELS/",
            "/Explore/",
            "/Direct/",
            "/aCCounts/",
            "/some%2Eone/",
            "/%73tories/",
            "/some-one/",
        ] {
            assert!(!document(path), "{path}");
        }
    }

    fn rest(read: Rest, query: &[(&str, &str)]) -> Ask {
        Ask::Rest {
            read,
            query: query
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// The intent is held to the registry before anything is built from
    /// it.
    #[test]
    fn an_ask_is_held_to_the_registry() {
        let pk = Pk::new(2_345_678_901);
        let allowed = [
            Ask::Viewer,
            Ask::Tray,
            Ask::Pk {
                name: "someone".into(),
            },
            Ask::Document { path: "/".into() },
            Ask::Document {
                path: "/some.one_else/".into(),
            },
            Ask::Query {
                operation: Operation::ProfilePage,
                variables: "{}".into(),
            },
            Ask::Write {
                mutation: Mutation::Follow,
                variables: "{}".into(),
                doc_id: "1000000000000003".into(),
                route: "comet.igweb.PolarisProfilePostsTabRoute".into(),
            },
            rest(
                Rest::Followers(pk),
                &[
                    ("count", "12"),
                    ("max_id", "24"),
                    ("search_surface", "follow_list_page"),
                ],
            ),
            rest(Rest::Following(pk), &[("count", "12"), ("max_id", "24")]),
            rest(
                Rest::MutualFollowers(pk),
                &[("page_size", "12"), ("max_id", "x")],
            ),
            rest(Rest::Following(pk), &[]),
            rest(
                Rest::MediaInfo(MediaPk::new(3_807_824_420_233_075_826)),
                &[],
            ),
            rest(
                Rest::Comments(MediaPk::new(3_807_824_420_233_075_826)),
                &[
                    ("can_support_threading", "true"),
                    ("permalink_enabled", "false"),
                ],
            ),
            rest(
                Rest::Comments(MediaPk::new(3_807_824_420_233_075_826)),
                &[
                    ("can_support_threading", "true"),
                    ("min_id", r#"{"cached_comments_cursor":"1"}"#),
                    ("sort_order", "popular"),
                ],
            ),
            rest(Rest::Likers(MediaPk::new(3_807_824_420_233_075_826)), &[]),
            rest(
                Rest::Replies(
                    MediaPk::new(3_807_824_420_233_075_826),
                    CommentPk::new(18_012_345_678_901_234),
                ),
                &[
                    ("min_id", ""),
                    ("is_chronological", "true"),
                    ("paging_direction", "view_more"),
                ],
            ),
            Ask::Navigation { route: "/".into() },
            Ask::Navigation {
                route: "/someone/followers/mutualOnly".into(),
            },
            Ask::Statuses { pks: vec![pk] },
            Ask::Statuses {
                pks: vec![pk; MOST_STATUSES],
            },
        ];
        for ask in allowed {
            assert_eq!(refused_ask(&ask), None, "{ask:?}");
        }

        let refusals = [
            (
                Ask::Query {
                    operation: Operation::Follow,
                    variables: "{}".into(),
                },
                "POST /api/graphql",
            ),
            (
                Ask::Query {
                    operation: Operation::Unfollow,
                    variables: "{}".into(),
                },
                "POST /api/graphql",
            ),
            (
                Ask::Pk {
                    name: "stories/someone".into(),
                },
                "POST /ajax/bulk-route-definitions/",
            ),
            (
                Ask::Pk { name: "".into() },
                "POST /ajax/bulk-route-definitions/",
            ),
            (
                Ask::Document {
                    path: "/stories/".into(),
                },
                "a navigation to /stories/",
            ),
            (
                Ask::Document {
                    path: "/Stories/".into(),
                },
                "a navigation to /Stories/",
            ),
            (
                Ask::Document {
                    path: "/someone/?x=1".into(),
                },
                "a navigation to /someone/?x=1",
            ),
            (
                Ask::Document {
                    path: "/someone/followers/".into(),
                },
                "a navigation to /someone/followers/",
            ),
            (
                rest(
                    Rest::Following(pk),
                    &[("search_surface", "follow_list_page")],
                ),
                "GET /api/v1/friendships/2345678901/following/",
            ),
            (
                rest(Rest::Followers(pk), &[("search_surface", "elsewhere")]),
                "GET /api/v1/friendships/2345678901/followers/",
            ),
            (
                rest(Rest::Followers(pk), &[("query", "a")]),
                "GET /api/v1/friendships/2345678901/followers/",
            ),
            (
                rest(Rest::MutualFollowers(pk), &[("count", "12")]),
                "GET /api/v1/friendships/2345678901/mutual_followers/",
            ),
            (
                rest(
                    Rest::MediaInfo(MediaPk::new(3_807_824_420_233_075_826)),
                    &[("count", "12")],
                ),
                "GET /api/v1/media/3807824420233075826/info/",
            ),
            (
                rest(
                    Rest::Comments(MediaPk::new(3_807_824_420_233_075_826)),
                    &[("permalink_enabled", "true")],
                ),
                "GET /api/v1/media/3807824420233075826/comments/",
            ),
            (
                rest(
                    Rest::Comments(MediaPk::new(3_807_824_420_233_075_826)),
                    &[("sort_order", "newest")],
                ),
                "GET /api/v1/media/3807824420233075826/comments/",
            ),
            // Asked with no query, as the app asks it: a cursor or a count
            // would be a second page nobody has seen the app ask for.
            (
                rest(
                    Rest::Likers(MediaPk::new(3_807_824_420_233_075_826)),
                    &[("max_id", "1")],
                ),
                "GET /api/v1/media/3807824420233075826/likers/",
            ),
            (
                Ask::Navigation {
                    route: "/stories/someone/".into(),
                },
                "POST /ajax/navigation/",
            ),
            (
                Ask::Statuses { pks: Vec::new() },
                "POST /api/v1/friendships/show_many/",
            ),
            (
                Ask::Statuses {
                    pks: vec![pk; MOST_STATUSES + 1],
                },
                "POST /api/v1/friendships/show_many/",
            ),
        ];
        for (ask, said) in refusals {
            assert_eq!(refused_ask(&ask).as_deref(), Some(said), "{ask:?}");
        }
    }

    /// A thread's replies go out with the app's three keys, in its order,
    /// the cursor empty, and in no other shape: a cursor, another direction,
    /// a key left out or one more are each a page the app never asked for.
    #[test]
    fn a_thread_goes_out_with_the_apps_three_keys_and_an_empty_cursor() {
        let replies = Rest::Replies(
            MediaPk::new(3_807_824_420_233_075_826),
            CommentPk::new(18_012_345_678_901_234),
        );
        let said =
            "GET /api/v1/media/3807824420233075826/comments/18012345678901234/child_comments/";
        let apps = [
            ("min_id", ""),
            ("is_chronological", "true"),
            ("paging_direction", "view_more"),
        ];
        assert_eq!(refused_ask(&rest(replies, &apps)), None);
        for query in [
            vec![
                ("min_id", "18012345678901235"),
                ("is_chronological", "true"),
                ("paging_direction", "view_more"),
            ],
            vec![
                ("min_id", ""),
                ("is_chronological", "true"),
                ("paging_direction", "view_less"),
            ],
            vec![
                ("min_id", ""),
                ("is_chronological", "false"),
                ("paging_direction", "view_more"),
            ],
            vec![
                ("is_chronological", "true"),
                ("paging_direction", "view_more"),
            ],
            vec![
                ("is_chronological", "true"),
                ("min_id", ""),
                ("paging_direction", "view_more"),
            ],
            vec![
                ("min_id", ""),
                ("is_chronological", "true"),
                ("paging_direction", "view_more"),
                ("max_id", "1"),
            ],
            vec![],
        ] {
            assert_eq!(
                refused_ask(&rest(replies, &query)).as_deref(),
                Some(said),
                "{query:?}"
            );
        }
    }

    /// What makes the app's call vouch for the number it sent a read under:
    /// the operation's own endpoint and variables, and its root field where
    /// the app names one. A write never does.
    #[test]
    fn a_call_of_the_app_vouches_for_a_read_only_by_what_it_asks() {
        let profile = Operation::ProfilePage;
        let moved = "1000000000000777";
        let vouch = |operation: Operation, path, doc_id, variables, root| {
            vouched(operation, path, doc_id, variables, root)
        };
        assert!(vouch(profile, "/api/graphql", moved, r#"{"id":"1"}"#, None));
        // Wrong endpoint, wrong variables, a number that is none, a root
        // field sent where the app sends none.
        assert!(!vouch(
            profile,
            "/graphql/query",
            moved,
            r#"{"id":"1"}"#,
            None
        ));
        assert!(!vouch(
            profile,
            "/api/graphql",
            moved,
            r#"{"userID":"1"}"#,
            None
        ));
        assert!(!vouch(profile, "/api/graphql", moved, "[1]", None));
        assert!(!vouch(profile, "/api/graphql", moved, "not json", None));
        for doc_id in ["", "123", "10000000000007x7", "10000000000007770000000"] {
            assert!(
                !vouch(profile, "/api/graphql", doc_id, r#"{"id":"1"}"#, None),
                "{doc_id}"
            );
        }
        assert!(!vouch(
            profile,
            "/api/graphql",
            moved,
            r#"{"id":"1"}"#,
            Some("user")
        ));
        // On /graphql/query the root field is the operation's.
        let gallery = Operation::ReelGallery;
        let variables = r#"{"initial_reel_id":"1"}"#;
        let root = Some(gallery.root_field());
        assert!(vouch(gallery, "/graphql/query", moved, variables, root));
        assert!(!vouch(gallery, "/graphql/query", moved, variables, None));
        assert!(!vouch(
            gallery,
            "/graphql/query",
            moved,
            variables,
            Some("user")
        ));
        // A write's number is found in the app's code, not here.
        for write in [Operation::Follow, Operation::Unfollow] {
            assert!(!vouch(
                write,
                "/api/graphql",
                moved,
                r#"{"target_user_id":"1"}"#,
                None
            ));
        }
    }

    /// A number held for a read lets that read out under it, and only that
    /// read, only on its own endpoint and under its own name; nothing else
    /// about the check moves.
    #[test]
    fn a_number_held_for_a_read_lets_that_read_out_and_nothing_else() {
        let moved = "1000000000000777";
        let profile = Operation::ProfilePage;
        let sent = relay("/api/graphql", profile.friendly_name(), moved);
        assert!(refused(&sent).is_some());
        let found = Rediscovered::of(profile, moved);
        assert_eq!(refused_with(&sent, &found), None);
        // Held for another operation, or under another number: refused.
        let hover = Operation::HoverCard;
        assert!(refused_with(&sent, &Rediscovered::of(hover, moved)).is_some());
        assert!(refused_with(&sent, &Rediscovered::of(profile, "1000000000000778")).is_some());
        // The header still has to name the same operation, and the endpoint
        // is still the operation's.
        let mut other = relay("/api/graphql", profile.friendly_name(), moved);
        other.headers.clear();
        assert!(refused_with(&other, &found).is_some());
        let elsewhere = relay("/graphql/query", profile.friendly_name(), moved);
        assert!(refused_with(&elsewhere, &found).is_some());
        // A write takes none: the number is not a read's to hold.
        let follow = Operation::Follow;
        assert_eq!(Rediscovered::of(follow, moved), Rediscovered::default());
        // Nor does a number that is not one.
        assert_eq!(Rediscovered::of(profile, "abc"), Rediscovered::default());
        assert_eq!(found.get(profile), Some(moved));
        assert_eq!(found.get(hover), None);
    }

    /// An app read as the app sends it: its name, on its own endpoint, under
    /// `doc_id`, with `variables`, naming `root` where the endpoint takes one.
    fn app_sent(read: &AppRead, doc_id: &str, variables: &str, root: Option<&str>) -> PageRequest {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("fb_api_req_friendly_name", read.name),
                ("variables", variables),
                ("doc_id", doc_id),
            ])
            .finish();
        let mut headers = vec![("X-FB-Friendly-Name", read.name)];
        if let Some(root) = root {
            headers.push(("X-Root-Field-Name", root));
        }
        request(Method::Post, read.path, &headers, Some(&body))
    }

    /// The app's own reads that write nothing go out by name and number, on
    /// their own endpoint; the rest of what the app sends does not.
    #[test]
    fn the_apps_boot_reads_go_out_by_name_and_number_and_nothing_else_does() {
        let nobody = Rediscovered::default();
        for read in &APP_READS {
            let Number::Pinned(doc_id) = read.number else {
                continue;
            };
            let sent = app_sent(read, doc_id, r#"{"id":"1"}"#, None);
            assert!(refused(&sent).is_some(), "{} is not snob's", read.name);
            assert_eq!(refused_app(&sent, &nobody), None, "{}", read.name);
            // Under another number, on another endpoint, or announced as
            // another name, it is anybody's guess.
            let moved = app_sent(read, "1000000000000999", "{}", None);
            assert!(refused_app(&moved, &nobody).is_some(), "{}", read.name);
            let mut elsewhere = sent.clone();
            elsewhere.url = format!("{BASE}/graphql/query");
            assert!(refused_app(&elsewhere, &nobody).is_some(), "{}", read.name);
            let mut renamed = sent.clone();
            renamed.headers = vec![("X-FB-Friendly-Name".into(), "SomethingElse".into())];
            assert!(refused_app(&renamed, &nobody).is_some(), "{}", read.name);
        }
        // Reads of the app's that are left out, a write, and the beacons.
        for name in [
            "IGDThreadDetailQuery",
            "IGDInboxTrayQuery",
            "PolarisSearchBoxRefetchableQuery",
            "PolarisAPIGetFrCookieQuery",
            "usePolarisSomethingMutation",
        ] {
            let sent = relay("/api/graphql", name, "1000000000000001");
            assert!(refused_app(&sent, &nobody).is_some(), "{name}");
        }
        for path in ["/ajax/bz", "/ajax/qm/", "/ajax/navigation/"] {
            let sent = request(Method::Post, path, &[], Some("a=b"));
            assert_eq!(
                refused_app(&sent, &nobody),
                Some(format!("POST {path}")),
                "{path}"
            );
        }
        // A GET and a navigation are the page's, as before.
        let get = request(Method::Get, "/api/v1/web/something/", &[], None);
        assert!(refused_app(&get, &nobody).is_some());
        // The registry's own go on as ever, and a rediscovered number with
        // them.
        let profile = Operation::ProfilePage;
        let sent = relay("/api/graphql", profile.friendly_name(), profile.doc_id());
        assert_eq!(refused_app(&sent, &nobody), None);
        let moved = relay("/api/graphql", profile.friendly_name(), "1000000000000777");
        assert!(refused_app(&moved, &nobody).is_some());
        assert_eq!(
            refused_app(&moved, &Rediscovered::of(profile, "1000000000000777")),
            None
        );
    }

    /// A follow or an unfollow the app sends on its own is refused under any
    /// number, its seed's and a rediscovered one's included, though the same
    /// form is one [`refused`] lets out for snob.
    #[test]
    fn the_apps_own_follow_and_unfollow_are_refused_under_any_number() {
        for write in [Operation::Follow, Operation::Unfollow] {
            for doc_id in [write.doc_id(), "1000000000000003"] {
                let sent = relay(write.path(), write.friendly_name(), doc_id);
                assert_eq!(refused(&sent), None, "snob's {write:?}");
                let found = Rediscovered::of(write, doc_id);
                for found in [&Rediscovered::default(), &found] {
                    assert_eq!(
                        refused_app(&sent, found),
                        Some(format!("POST {}", write.path())),
                        "the app's {write:?} under {doc_id}"
                    );
                }
            }
        }
    }

    /// The profile's posts go out under whatever number the app sends them,
    /// the three captured among them included, when the call vouches for it:
    /// on `/api/graphql`, where the app has sent them since 2026-11-13, with
    /// no root-field header, asking by `username`. Not on `/graphql/query`,
    /// where they were before, not naming a root field, not without the name
    /// it asks by, and not under something that is not a number.
    #[test]
    fn the_profile_posts_go_out_under_any_number_the_call_vouches_for() {
        let nobody = Rediscovered::default();
        let posts = APP_READS
            .iter()
            .find(|read| read.name == "PolarisProfilePostsQuery")
            .unwrap();
        let asks = r#"{"data":{"count":12},"username":"someone"}"#;
        for doc_id in [
            "28570182382647478",
            "28991540097136703",
            "29114630661562380",
            "1000000000000777",
        ] {
            let sent = app_sent(posts, doc_id, asks, None);
            // Snob's own goes out under the registry's number alone, or one
            // the app was seen to send; the app's under any it vouches for.
            assert_eq!(
                refused(&sent).is_none(),
                doc_id == Operation::ProfilePosts.doc_id(),
                "{doc_id}"
            );
            assert_eq!(refused_app(&sent, &nobody), None, "{doc_id}");
        }
        // A number not the registry's: the app's call stands on its own.
        let doc_id = "28570182382647478";
        let root = "xdt_api__v1__feed__user_timeline_graphql_connection";
        let mut before = app_sent(posts, doc_id, asks, Some(root));
        before.url = format!("{BASE}/graphql/query");
        let refusals = [
            before,
            app_sent(posts, doc_id, asks, Some(root)),
            app_sent(posts, doc_id, r#"{"data":{"count":12}}"#, None),
            app_sent(posts, doc_id, "not json", None),
            app_sent(posts, "12ab", asks, None),
        ];
        for (i, sent) in refusals.iter().enumerate() {
            assert!(refused_app(sent, &nobody).is_some(), "case {i}");
        }
    }

    /// The promotions and the two message counts are held by their name and
    /// the key they ask by, not by a number: all three moved between
    /// 2026-10-01 and 2026-11-13, and a pinned number refused the app's own
    /// call at the next move. Under any number, then, but not without the
    /// key, not naming a root field `/api/graphql` never names, and not on
    /// the other endpoint.
    #[test]
    fn an_app_read_is_held_by_its_name_and_key_not_a_pinned_number() {
        let nobody = Rediscovered::default();
        let held: Vec<_> = APP_READS
            .iter()
            .filter(|read| read.name != "PolarisProfilePostsQuery")
            .filter_map(|read| match read.number {
                Number::Vouched {
                    root_field: None,
                    asked_key,
                } => Some((read, asked_key)),
                _ => None,
            })
            .collect();
        let names: Vec<_> = held.iter().map(|(read, _)| read.name).collect();
        assert_eq!(
            names,
            [
                "QuickPromotionSupportIGSchemaBatchFetchQuery",
                "IGDBadgeCountOffMsysQuery",
                "IGDChatTabsJewelOffMsysQuery"
            ]
        );
        for (read, key) in held {
            let asks = format!(r#"{{"{key}":"1"}}"#);
            for doc_id in ["26673487622279953", "28774579702164828", "1000000000000999"] {
                let sent = app_sent(read, doc_id, &asks, None);
                assert!(refused(&sent).is_some(), "{} is not snob's", read.name);
                assert_eq!(refused_app(&sent, &nobody), None, "{} {doc_id}", read.name);
            }
            let doc_id = "1000000000000999";
            let mut elsewhere = app_sent(read, doc_id, &asks, None);
            elsewhere.url = format!("{BASE}/graphql/query");
            let refusals = [
                elsewhere,
                app_sent(read, doc_id, r#"{"other":"1"}"#, None),
                app_sent(read, doc_id, &asks, Some("data")),
                app_sent(read, "not-a-number", &asks, None),
            ];
            for (i, sent) in refusals.iter().enumerate() {
                assert!(
                    refused_app(sent, &nobody).is_some(),
                    "{} case {i}",
                    read.name
                );
            }
        }
    }

    /// The reads a profile's own document sends that were refused until
    /// 2026-10-01 go out now: its posts, and the accounts it suggests.
    #[test]
    fn a_profiles_posts_and_suggestions_are_the_apps_reads() {
        let names: Vec<&str> = APP_READS.iter().map(|read| read.name).collect();
        for name in [
            "PolarisProfilePostsQuery",
            "PolarisProfileSuggestedUsersWithPreloadableQuery",
            "PolarisProfileSuggestedUsersWithLazyQueryQuery",
        ] {
            assert!(names.contains(&name), "{name}");
        }
    }

    /// Every one of the app's boot reads is a query by its name, and none is
    /// the registry's, so what snob sends is never widened by it, but one:
    /// the posts' first page, which the app sends on every profile document
    /// under any number the call vouches for, and which snob sends itself
    /// under the registry's number or one the app was seen to send.
    #[test]
    fn the_apps_boot_reads_are_queries_that_are_not_the_registrys() {
        for read in &APP_READS {
            assert!(read.name.ends_with("Query"), "{}", read.name);
            let snobs = Operation::named(read.name);
            assert!(
                snobs.is_none() || snobs == Some(Operation::ProfilePosts),
                "{}",
                read.name
            );
            assert!(
                matches!(read.path, "/api/graphql" | "/graphql/query"),
                "{}",
                read.name
            );
            if let Number::Pinned(doc_id) = read.number {
                assert!(is_doc_id(doc_id), "{}", read.name);
            }
        }
    }

    /// `show_many` goes out as the app sends it after a list page and in no
    /// other shape: the pks, then `jazoest` and `fb_dtsg`, nothing more.
    #[test]
    fn show_many_goes_out_only_as_the_app_sends_it() {
        let posted = |body: &str| request(Method::Post, SHOW_MANY, &[], Some(body));
        for body in [
            "user_ids=1&jazoest=22858&fb_dtsg=t",
            "user_ids=2345678901%2C3456789012%2C7&jazoest=22858&fb_dtsg=t",
        ] {
            assert_eq!(refused(&posted(body)), None, "{body}");
        }
        let fifty_one = (0..=MOST_STATUSES)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("%2C");
        let too_many = format!("user_ids={fifty_one}&jazoest=22858&fb_dtsg=t");
        for body in [
            "",
            "user_ids=&jazoest=22858&fb_dtsg=t",
            "user_ids=1%2C&jazoest=22858&fb_dtsg=t",
            "user_ids=someone&jazoest=22858&fb_dtsg=t",
            "jazoest=22858&fb_dtsg=t&user_ids=1",
            "user_ids=1&jazoest=x&fb_dtsg=t",
            "user_ids=1&jazoest=22858&fb_dtsg=t&target_user_id=1",
            "user_ids=1&jazoest=22858",
            too_many.as_str(),
        ] {
            assert_eq!(
                refused(&posted(body)).as_deref(),
                Some("POST /api/v1/friendships/show_many/"),
                "{body}"
            );
        }
        // A GET of it is not one.
        let get = request(Method::Get, SHOW_MANY, &[], None);
        assert!(refused(&get).is_some());
    }

    /// The app's navigation goes out to the home page, a profile, and a
    /// profile's mutual followers, and to none of the app's screens.
    #[test]
    fn a_navigation_goes_out_only_to_a_route_snob_reads_from() {
        let posted = |route: &str| {
            let body = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs([
                    ("client_previous_actor_id", "17841400000000001"),
                    ("route_url", route),
                    ("routing_namespace", "igx_www"),
                ])
                .finish();
            request(Method::Post, NAVIGATION, &[], Some(&body))
        };
        for route in [
            "/",
            "/someone/",
            "/some.one_else/followers/mutualOnly",
            "/someone/reels/",
            "/someone/tagged/",
            "/someone/reposts/",
        ] {
            assert_eq!(refused(&posted(route)), None, "{route}");
            assert!(list_route(route), "{route}");
        }
        for route in [
            "/stories/someone/",
            "/stories/highlights/17938499744579885/?r=1",
            "/explore/",
            "/direct/inbox/",
            "/reels/",
            "/stories/reels/",
            "/someone/reels",
            "/someone/reels/x/",
            "/someone/saved/",
            "/someone/followers/",
            "/someone/followers/mutualFirst",
            "/followers/mutualOnly",
            "/stories/followers/mutualOnly",
            "/someone/?x=1",
        ] {
            assert_eq!(
                refused(&posted(route)).as_deref(),
                Some("POST /ajax/navigation/"),
                "{route}"
            );
            assert!(!list_route(route), "{route}");
        }
        // Two routes in one form, or none, are not one navigation.
        let twice = request(
            Method::Post,
            NAVIGATION,
            &[],
            Some("route_url=%2Fa%2F&route_url=%2Fstories%2F"),
        );
        assert!(refused(&twice).is_some());
        let none = request(Method::Post, NAVIGATION, &[], Some("routing_namespace=x"));
        assert!(refused(&none).is_some());
    }

    /// A tab's route is one the app's router is let move to, never a
    /// document the tab loads: the tab stays on the profile, as with the
    /// mutual followers, and `/reels/` alone is the app's reels screen.
    #[test]
    fn a_tab_route_is_navigated_to_only_by_the_router() {
        for tab in PROFILE_TABS {
            let route = format!("/someone/{tab}/");
            assert!(list_route(&route), "{route}");
            assert!(!document(&route), "{route}");
            let mut sent = request(Method::Get, &route, &[], None);
            sent.navigate = true;
            assert!(refused(&sent).is_some(), "{route}");
            assert_eq!(
                refused_ask(&Ask::Document {
                    path: route.clone()
                })
                .as_deref(),
                Some(format!("a navigation to {route}").as_str())
            );
        }
    }

    /// The reels screen is no profile, and no tab of one.
    #[test]
    fn the_reels_screen_is_still_no_profile() {
        assert!(!document("/reels/"));
        assert!(!list_route("/reels/"));
        assert!(!list_route("/reels/reels/"));
    }

    /// Each tab's two queries go out on `/api/graphql` under their numbers,
    /// and under no other number and on no other endpoint.
    #[test]
    fn a_tab_goes_out_under_its_number_on_api_graphql() {
        for operation in [
            Operation::ProfileReels,
            Operation::ProfileReelsPage,
            Operation::ProfileTagged,
            Operation::ProfileTaggedPage,
            Operation::ProfileReposts,
            Operation::ProfileRepostsPage,
        ] {
            assert_eq!(operation.family(), Family::Relay, "{operation:?}");
            let sent = relay(
                "/api/graphql",
                operation.friendly_name(),
                operation.doc_id(),
            );
            assert_eq!(refused(&sent), None, "{operation:?}");
            let moved = relay(
                "/api/graphql",
                operation.friendly_name(),
                "1000000000000777",
            );
            assert!(refused(&moved).is_some(), "{operation:?}");
            let elsewhere = relay(
                "/graphql/query",
                operation.friendly_name(),
                operation.doc_id(),
            );
            assert!(refused(&elsewhere).is_some(), "{operation:?}");
        }
    }

    /// Only the method and the path: the form carries tokens.
    #[test]
    fn a_refusal_names_no_token() {
        let sent = relay("/api/graphql", "PolarisSomethingElseQuery", "1");
        let said = refused(&sent).unwrap();
        assert!(!said.contains("relay:token"), "{said}");
    }
}
