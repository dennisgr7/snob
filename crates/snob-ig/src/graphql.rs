//! The two writes, which the web serves through Relay only, and what a
//! mutation is built from.
//!
//! The August 2026 capture of the real web client showed the Follow button
//! sending `POST /api/graphql` with `fb_api_req_friendly_name:
//! usePolarisFollowMutation`. The REST spellings are the mobile app's:
//! `/api/v1/friendships/create/{pk}/` answers 200 with the app's HTML shell and
//! `/web/friendships/{pk}/follow/` answers 404 (both sent live; neither
//! changed anything).
//!
//! `fb_dtsg` and `lsd` come from the HTML of a logged-in page, `jazoest` is
//! computed ([`jazoest`]), and the `doc_id` is **discovered from the page's own
//! JavaScript** ([`doc_id_in`]), since Instagram rotates them. From the browser
//! the tab builds the app's 29 fields ([`crate::web`]); without one, the twelve
//! of [`mutation_body`], and none of the app's telemetry, which would describe
//! a browsing session that did not happen.

use snob_core::secret::Secret;

/// What a run already knows about the mutation identifiers. Discovery costs
/// megabytes of bundles, so it is cached: once per rotation, not per follow.
///
/// A trait because the database remembers and this crate opens none.
/// **Deliberately not `Send + Sync`**: `rusqlite::Connection` is neither.
pub trait DocIds {
    fn get(&self, friendly_name: &str) -> Option<String>;
    fn put(&self, friendly_name: &str, doc_id: &str);
}

/// The two tokens a page hands out, which together authorize a mutation.
/// Credentials, hence `Secret`.
#[derive(Debug)]
pub struct PageTokens {
    pub fb_dtsg: Secret,
    pub lsd: Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Mutation {
    Follow,
    Unfollow,
}

impl Mutation {
    /// Every write this program can make, as a value a test can read. The
    /// three matches below have no wildcard arm on purpose.
    pub const ALL: [Self; 2] = [Self::Follow, Self::Unfollow];

    /// Where the write goes: on the mutation, never at the call site.
    pub fn path(self) -> &'static str {
        match self {
            Self::Follow | Self::Unfollow => "/api/graphql",
        }
    }

    /// The name the request announces itself with, and the one looked for
    /// when discovering the `doc_id`: one constant, so they cannot disagree.
    pub fn friendly_name(self) -> &'static str {
        match self {
            Self::Follow => "usePolarisFollowMutation",
            Self::Unfollow => "usePolarisUnfollowMutation",
        }
    }

    /// The identifier as captured on 21 August 2026 (Chrome 151, `POST
    /// /api/graphql`): a seed.
    ///
    /// Discovering it on the way in is not buildable: the mutation lives in a
    /// chunk loaded on demand, in none of the about 440 bundles a profile page
    /// names (measured August 2026: sixty opened, about seven megabytes, no
    /// match). When the seed is refused,
    /// [`crate::client::IgClient::doc_id_for`] tries the walk anyway.
    pub fn seed_doc_id(self) -> &'static str {
        match self {
            Self::Follow => "26508036048874888",
            Self::Unfollow => "27789106940691111",
        }
    }
}

/// Instagram's checksum over `fb_dtsg`: the literal `2`, then the sum of the
/// token's bytes, as Facebook's own client computes it. Derived, not a secret.
pub fn jazoest(fb_dtsg: &str) -> String {
    let sum: u32 = fb_dtsg.bytes().map(u32::from).sum();
    format!("2{sum}")
}

/// Pulls `fb_dtsg` and `lsd` out of a rendered page's bootstrap JSON
/// (`["DTSGInitData",[],{"token":"..."}]`, `["LSD",[],{"token":"..."}]`),
/// without parsing megabytes of it.
///
/// `None` when either is missing. A logged-out page carries both too: the
/// page's `PolarisViewer` tells it apart, and the write reads that first.
pub fn extract_tokens(html: &str) -> Option<PageTokens> {
    Some(PageTokens {
        // **The bracket and the comma are part of the marker**: the page also
        // holds `LSDatabaseSingletonLazyWrapper`, and a wrong token would be
        // refused as a stale identifier and spend a second write slot.
        fb_dtsg: Secret::new(token_after(html, r#"["DTSGInitData","#)?),
        lsd: Secret::new(token_after(html, r#"["LSD","#)?),
    })
}

/// The first `"token":"…"` within reach of `marker`. Bounded, so a page
/// missing `DTSGInitData` cannot hand back the `lsd` token as `fb_dtsg`. The
/// window's end is floored to a character boundary: slicing mid-character
/// panics.
fn token_after(html: &str, marker: &str) -> Option<String> {
    /// In every capture the token is within forty bytes of the marker.
    const REACH: usize = 512;

    let at = html.find(marker)?;
    let window = &html[at..html.floor_char_boundary(at + REACH)];
    let key = "\"token\":\"";
    let start = window.find(key)? + key.len();
    let end = window[start..].find('"')? + start;
    let token = &window[start..end];
    (!token.is_empty()).then(|| token.to_string())
}

/// Finds the `doc_id` of a named mutation in a compiled Relay artifact, where
/// `name:"usePolarisFollowMutation"` sits beside `id:"26508036048874888"`, in
/// either order. Only a long, all-digit id is taken.
///
/// **The id taken is the one nearest the name, not the first in the window**:
/// the two mutations sit side by side in one chunk, and a first match would
/// follow when asked to unfollow.
pub fn doc_id_in(js: &str, friendly_name: &str) -> Option<String> {
    const REACH: usize = 400;
    /// Instagram's ids are seventeen digits today; no unrelated numeric field
    /// nearby reaches ten.
    const SHORTEST: usize = 10;

    let quoted = format!("\"{friendly_name}\"");
    let at = js.find(&quoted)?;
    // Floored to character boundaries: slicing mid-character panics, and
    // `from_utf8_lossy` leaves three-byte replacement characters.
    let from = js.floor_char_boundary(at.saturating_sub(REACH));
    let to = js.floor_char_boundary(at + quoted.len() + REACH);
    let window = &js[from..to];
    // "Nearest" is the gap between the two spans, not the distance between
    // their starts: measured start to start, the follow id would win the
    // search for the unfollow one.
    let name_at = at - from;
    let name_ends = name_at + quoted.len();

    let mut best: Option<(usize, String)> = None;
    // The same field before and after minification.
    for key in ["id:\"", "\"id\":\""] {
        let mut offset = 0;
        while let Some(found) = window[offset..].find(key) {
            let start = offset + found + key.len();
            let Some(len) = window[start..].find('"') else {
                break;
            };
            let candidate = &window[start..start + len];
            if candidate.len() >= SHORTEST && candidate.bytes().all(|b| b.is_ascii_digit()) {
                let ends = start + len;
                let gap = if start >= name_ends {
                    start - name_ends
                } else {
                    name_at.saturating_sub(ends)
                };
                if best.as_ref().is_none_or(|(near, _)| gap < *near) {
                    best = Some((gap, candidate.to_string()));
                }
            }
            offset = start;
        }
    }
    best.map(|(_, id)| id)
}

/// The `variables` object, as the site sends it. `nav_chain` is a constant:
/// assembling one would invent a path through the site nobody walked.
pub(crate) fn variables(target_pk: snob_core::Pk) -> String {
    format!(
        r#"{{"target_user_id":"{target_pk}","container_module":"profile","nav_chain":"{NAV_CHAIN}"}}"#
    )
}

const NAV_CHAIN: &str = "PolarisProfilePostsTabRoot:profilePage:1:via_cold_start";

/// The `__crn` of a write made from a profile: the route a profile document
/// loads cold, which [`NAV_CHAIN`] describes too.
pub const PROFILE_ROUTE: &str = "comet.igweb.PolarisProfilePostsTabRoute";

/// The trailing slash anchors the host: without it
/// `static.cdninstagram.com.evil.example` would match.
const BUNDLE_PREFIX: &str = "https://static.cdninstagram.com/";

/// Whether `url` is a bundle a `doc_id` may be looked for in. Applied to any
/// list of bundles, whoever made it, before one is downloaded.
pub(crate) fn is_bundle(url: &str) -> bool {
    url.starts_with(BUNDLE_PREFIX) && url.ends_with(".js")
}

/// Every JavaScript bundle a page pulls in, **in the page's order**: it lists
/// the chunk its own route needs early. Only the fixed script host is
/// accepted. Most arrive with their slashes escaped, inside JSON strings
/// (`https:\/\/static.cdninstagram.com\/…`): reading only the plain ones
/// finds six of 437.
pub fn bundles_in(html: &str) -> Vec<String> {
    const HOST: &str = "static.cdninstagram.com";

    let mut found: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find(HOST) {
        let line_start = rest[..at].rfind("https:").filter(|s| at - s <= 10);
        let from = &rest[line_start.unwrap_or(at)..];
        // A backslash is no terminator here: it escapes a slash.
        let end = from
            .find(|c: char| c == '"' || c == '\'' || c == '<' || c == ')' || c.is_whitespace())
            .unwrap_or(from.len());
        let url = from[..end].replace("\\/", "/");
        if is_bundle(&url) && !found.contains(&url) {
            found.push(url);
        }
        rest = &rest[at + HOST.len()..];
    }
    found
}

/// The form body of a mutation, in the browser's order. `__user=0` is what
/// the real request sends, logged in.
pub fn mutation_body(
    tokens: &PageTokens,
    mutation: Mutation,
    doc_id: &str,
    target_pk: snob_core::Pk,
) -> Vec<(String, String)> {
    let fb_dtsg = tokens.fb_dtsg.expose().to_string();
    let jazoest = jazoest(&fb_dtsg);
    vec![
        ("__d".into(), "www".into()),
        ("__user".into(), "0".into()),
        ("__a".into(), "1".into()),
        ("__comet_req".into(), "7".into()),
        ("fb_dtsg".into(), fb_dtsg),
        ("jazoest".into(), jazoest),
        ("lsd".into(), tokens.lsd.expose().to_string()),
        ("fb_api_caller_class".into(), "RelayModern".into()),
        (
            "fb_api_req_friendly_name".into(),
            mutation.friendly_name().into(),
        ),
        ("server_timestamps".into(), "true".into()),
        ("variables".into(), variables(target_pk)),
        ("doc_id".into(), doc_id.into()),
    ]
}

#[cfg(test)]
mod tests {
    use snob_core::Pk;

    use super::*;

    /// **There are two writes, and there is no third.**
    ///
    /// The rule this pins is the one in `AGENTS.md`, and it is pinned in two
    /// directions because there are two ways to break it. Adding a variant is
    /// already a compile error -- `path`, `friendly_name` and `seed_doc_id`
    /// have no wildcard arm between them. What this catches is the quieter
    /// one: pointing a variant that already exists at a different operation,
    /// which changes what the program does to somebody else's account without
    /// changing a single type.
    ///
    /// The names are written out rather than derived, so that changing one is
    /// a decision somebody has to make here, in front of this comment.
    #[test]
    fn the_only_two_writes_are_the_two_that_are_named() {
        assert_eq!(Mutation::ALL.len(), 2);
        assert_eq!(
            Mutation::ALL.map(Mutation::friendly_name),
            ["usePolarisFollowMutation", "usePolarisUnfollowMutation"],
        );
        for mutation in Mutation::ALL {
            assert_eq!(mutation.path(), "/api/graphql");
            // Nothing that registers a view. The web client's operation for
            // that is named in `crates/snob-core/tests/no_seen.rs`, which is
            // the one file allowed to write it down; this checks the value that
            // would actually go on the wire, which that guard cannot see.
            let name = mutation.friendly_name();
            assert!(
                !name.contains("Seen"),
                "a write that marks something seen: {name}"
            );
        }
    }

    /// The literal `2` and the sum of the bytes. Checked against a hand-worked
    /// value rather than against the implementation restated.
    #[test]
    fn jazoest_is_two_and_the_sum_of_the_bytes() {
        // 'a' is 97, three of them is 291.
        assert_eq!(jazoest("aaa"), "2291");
        assert_eq!(jazoest(""), "20");
    }

    /// The shape the page really has, trimmed to the two markers.
    const PAGE: &str = r#"...{"define":[["DTSGInitData",[],{"token":"NAfyABC:123:456","async_get_token":"x"},258],
        ["LSD",[],{"token":"G8i4s1ETVk"},323]]}..."#;

    #[test]
    fn both_tokens_come_out_of_a_rendered_page() {
        let tokens = extract_tokens(PAGE).expect("a logged-in page has both");
        assert_eq!(tokens.fb_dtsg.expose(), "NAfyABC:123:456");
        assert_eq!(tokens.lsd.expose(), "G8i4s1ETVk");
    }

    /// A page without `DTSGInitData` has no `fb_dtsg`, and that has to be
    /// `None` rather than the next token further down the file — which is what
    /// an unbounded search would return, and would send the `lsd` where the
    /// `fb_dtsg` goes.
    #[test]
    fn a_page_without_the_marker_does_not_borrow_the_next_token() {
        let no_dtsg = r#"["LSD",[],{"token":"onlythisone"}]"#;
        assert!(extract_tokens(no_dtsg).is_none());
    }

    /// **A decoy before the real entry must not win.**
    ///
    /// The test above covers the marker being *absent*. This covers it being
    /// present twice, which is the case the real page is one reordering away
    /// from: `LSDatabaseSingletonLazyWrapper` appears five times in a profile
    /// page, and a bare three-character `LSD` search would have taken whatever
    /// token sat near the first one it met.
    #[test]
    fn a_name_that_merely_starts_with_the_marker_is_not_the_marker() {
        let page = concat!(
            r#"["LSDatabaseSingletonLazyWrapper",[],{"token":"WRONG"},7],"#,
            r#"["DTSGInitData",[],{"token":"dtsg"},1],"#,
            r#"["LSD",[],{"token":"right"},2]"#
        );
        let tokens = extract_tokens(page).expect("both tokens are there");
        assert_eq!(tokens.lsd.expose(), "right");
        assert_eq!(tokens.fb_dtsg.expose(), "dtsg");
    }

    /// **A page with an accent in it must not end the process.**
    ///
    /// The window is a byte count and the page is UTF-8, so a character lying
    /// across the far edge was a panic rather than a miss. Swept across the
    /// edge rather than placed on it, because the arithmetic that decides
    /// which byte the edge is on is the arithmetic under test.
    #[test]
    fn a_multi_byte_character_on_the_window_edge_is_not_a_panic() {
        let marker = "[\"DTSGInitData\",";
        // The marker is sixteen bytes and the reach is five hundred and
        // twelve, so the edge falls on the character when the filler is around
        // four hundred and ninety-five. Swept, not pinned, so the test does not
        // depend on my arithmetic being right.
        for filler in 490..500 {
            let page = format!(
                "{marker}{}\u{e9}[],{{\"token\":\"t\"}}][\"LSD\",[],{{\"token\":\"l\"}}]",
                "x".repeat(filler)
            );
            // The answer may be `None` -- the token is past the reach at these
            // lengths. What it may not be is an abort.
            let _ = extract_tokens(&page);
        }

        // And with the character in the way but the token still inside, the
        // token still comes out.
        let near = format!("{marker}\u{e9}[],{{\"token\":\"t\"}}][\"LSD\",[],{{\"token\":\"l\"}}]");
        let tokens = extract_tokens(&near).expect("both tokens are within reach");
        assert_eq!(tokens.fb_dtsg.expose(), "t");
        assert_eq!(tokens.lsd.expose(), "l");
    }

    /// The same hazard on the recovery path, where it matters more: the text is
    /// `from_utf8_lossy` of a minified bundle, so it is dense with three-byte
    /// replacement characters, and this is the code that only runs on the day a
    /// `doc_id` has already rotated.
    #[test]
    fn a_lossy_bundle_does_not_panic_the_discovery_walk() {
        let name = "\"usePolarisFollowMutation\"";
        let tail = ",id:\"26508036048874888\"";

        // Below the window: a replacement character where `at - 400` falls.
        for pad in 394..404 {
            let js = format!("\u{fffd}{}{name}{tail}", "y".repeat(pad));
            assert_eq!(
                doc_id_in(&js, "usePolarisFollowMutation").as_deref(),
                Some("26508036048874888"),
                "lower edge, pad {pad}"
            );
        }

        // Above it: one where `at + len + 400` falls.
        for pad in 370..382 {
            let js = format!("{name}{tail}{}\u{fffd}", "z".repeat(pad));
            assert_eq!(
                doc_id_in(&js, "usePolarisFollowMutation").as_deref(),
                Some("26508036048874888"),
                "upper edge, pad {pad}"
            );
        }
    }

    /// Minified Relay, in both the orders the minifier produces.
    #[test]
    fn the_doc_id_is_found_either_side_of_the_name() {
        let after = r#"params:{id:"26508036048874888",metadata:{},name:"usePolarisFollowMutation",operationKind:"mutation"}"#;
        let before = r#"params:{name:"usePolarisFollowMutation",operationKind:"mutation",id:"26508036048874888"}"#;
        for js in [after, before] {
            assert_eq!(
                doc_id_in(js, "usePolarisFollowMutation").as_deref(),
                Some("26508036048874888"),
                "{js}"
            );
        }
    }

    /// The two mutations must not be confused for each other. They sit in the
    /// same file, so a search that found the nearest id rather than the right
    /// one would follow when asked to unfollow.
    #[test]
    fn each_mutation_gets_its_own_id() {
        let js = concat!(
            r#"{name:"usePolarisFollowMutation",id:"26508036048874888"},"#,
            r#"{name:"usePolarisUnfollowMutation",id:"27789106940691111"}"#
        );
        assert_eq!(
            doc_id_in(js, "usePolarisUnfollowMutation").as_deref(),
            Some("27789106940691111")
        );
        assert_eq!(
            doc_id_in(js, "usePolarisFollowMutation").as_deref(),
            Some("26508036048874888")
        );
    }

    /// A chunk that does not contain the mutation says so, rather than handing
    /// back somebody else's id.
    #[test]
    fn a_chunk_without_the_mutation_finds_nothing() {
        let js = r#"params:{id:"11111111111111111",name:"useSomethingElseMutation"}"#;
        assert!(doc_id_in(js, "usePolarisFollowMutation").is_none());
    }

    /// A short number near the name is not an id. Without the length floor,
    /// `version:2` or an array index next to the name would be sent as one.
    #[test]
    fn a_short_number_is_not_mistaken_for_an_id() {
        let js = r#"{id:"7",name:"usePolarisFollowMutation"}"#;
        assert!(doc_id_in(js, "usePolarisFollowMutation").is_none());
    }

    #[test]
    fn the_bundles_come_out_in_the_order_the_page_names_them() {
        let html = concat!(
            r#"<script src="https://static.cdninstagram.com/rsrc.php/v4/yD/r/first.js"></script>"#,
            r#"<link href="https://static.cdninstagram.com/rsrc.php/v4/yE/r/second.js">"#
        );
        let found = bundles_in(html);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].ends_with("first.js"));
        assert!(found[1].ends_with("second.js"));
    }

    /// **The escaped form is most of them.** A page writes a handful plainly in
    /// `<script src>` and every other one inside a JSON string, where the
    /// slashes are escaped. Reading only the plain ones looks like it works and
    /// finds one and a half per cent of what is there — measured on a real
    /// profile page: six out of four hundred and thirty-seven.
    #[test]
    fn a_bundle_written_inside_a_json_string_is_found_too() {
        let html = r#"{"src":"https:\/\/static.cdninstagram.com\/rsrc.php\/v4\/lazy.js","c":1}"#;
        let found = bundles_in(html);
        assert_eq!(
            found,
            vec!["https://static.cdninstagram.com/rsrc.php/v4/lazy.js"],
            "the escaped form has to be unescaped, not skipped"
        );
    }

    /// A page names the same bundle in more than one place. Fetching it twice
    /// is two requests for one answer.
    #[test]
    fn a_bundle_named_twice_is_fetched_once() {
        let once = "https://static.cdninstagram.com/rsrc.php/v4/a.js";
        let html = format!(r#"<script src="{once}"></script><script src="{once}"></script>"#);
        assert_eq!(bundles_in(&html).len(), 1);
    }

    /// **Only Instagram's script host.** A page is full of URLs and some of
    /// them point at places this tool has no business fetching from; the host
    /// is fixed in the code rather than read out of the document, which is what
    /// makes walking the list safe.
    #[test]
    fn a_script_somewhere_else_is_not_walked() {
        let html = r#"<script src="https://evil.example/rsrc.php/x.js"></script>
                      <script src="https://static.cdninstagram.com.evil.example/a.js"></script>"#;
        assert!(bundles_in(html).is_empty());
    }

    /// The same rule for a list handed over whole, whoever made it.
    #[test]
    fn a_bundle_is_a_script_on_the_fixed_cdn_address() {
        assert!(is_bundle(
            "https://static.cdninstagram.com/rsrc.php/v4/a.js"
        ));
        for not in [
            "http://127.0.0.1:9/rsrc.php/v4/a.js",
            "https://static.cdninstagram.com.evil.example/a.js",
            "http://static.cdninstagram.com/a.js",
            "https://static.cdninstagram.com/a.css",
            "https://evil.example/https://static.cdninstagram.com/a.js",
        ] {
            assert!(!is_bundle(not), "{not}");
        }
    }

    /// Things that are not JavaScript are not JavaScript.
    #[test]
    fn only_scripts_are_walked() {
        let html = r#"<img src="https://static.cdninstagram.com/rsrc.php/v4/logo.png">"#;
        assert!(bundles_in(html).is_empty());
    }

    #[test]
    fn the_body_carries_what_the_site_carries() {
        let tokens = PageTokens {
            fb_dtsg: Secret::new("TOKEN".to_string()),
            lsd: Secret::new("LSD".to_string()),
        };
        let body = mutation_body(&tokens, Mutation::Unfollow, "27789106940691111", Pk::new(7));
        let field = |name: &str| {
            body.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            field("fb_api_req_friendly_name"),
            "usePolarisUnfollowMutation"
        );
        assert_eq!(field("doc_id"), "27789106940691111");
        assert_eq!(field("fb_dtsg"), "TOKEN");
        // Derived, not fetched, and derived from the token beside it.
        assert_eq!(field("jazoest"), jazoest("TOKEN"));
        assert!(field("variables").contains(r#""target_user_id":"7""#));
    }

    /// **The telemetry the browser attaches is not sent.** It is Relay's record
    /// of what the page had already loaded; inventing it would describe a
    /// browsing session that did not happen, and it is kilobytes long.
    #[test]
    fn no_relay_telemetry_is_invented() {
        let tokens = PageTokens {
            fb_dtsg: Secret::new("TOKEN".to_string()),
            lsd: Secret::new("LSD".to_string()),
        };
        let body = mutation_body(&tokens, Mutation::Follow, "1", Pk::new(7));
        for invented in ["__dyn", "__csr", "__hsdp", "__hblp", "__sjsp", "__spin_t"] {
            assert!(
                !body.iter().any(|(k, _)| k == invented),
                "{invented} was made up"
            );
        }
    }

    #[test]
    fn the_variables_name_the_account_and_the_screen() {
        let vars = variables(Pk::new(528817151));
        assert!(vars.contains(r#""target_user_id":"528817151""#), "{vars}");
        assert!(vars.contains(r#""container_module":"profile""#), "{vars}");
    }
}
