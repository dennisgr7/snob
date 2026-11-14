//! The client hints and related headers that go with a request, computed from
//! the User-Agent. See "Talking to Instagram" in `AGENTS.md`.
//!
//! **This dresses the `reqwest` path** (`SNOB_NO_BROWSER`, and tests); a real
//! browser sends its own. The browser path takes only [`brand_list`],
//! [`platform_name`] and [`ASBD_ID`] from here.
//!
//! **The set has to agree with itself**: every value is computed from the
//! User-Agent the session was created with, and what cannot be computed is
//! left out. An absent header is honest, an invented one is not.
//! `Accept-Language` describes the person, so it is read from the system; it
//! is the one header the captures saw in `Vary` (132 times in August 2026;
//! no `Sec-Fetch-*` in 4210 `Vary` headers).
//!
//! **The TLS handshake, the HTTP/2 settings and the header order are never
//! imitated**: copying a handshake is a claim to be a browser, it would mean
//! leaving `rustls`, and what decides throttling is the address, the volume
//! and the speed.

/// From which Chromium sends `Priority` on a fetch or XHR.
const PRIORITY_SINCE_CHROMIUM: u32 = 123;

/// What Chromium sends on a fetch it did not prioritize itself.
pub const FETCH_PRIORITY: &str = "u=1, i";

/// What it sends on a top-level navigation. **Measured** on the wire:
/// `u=0, i` on all six navigations, `u=1, i` on all 924 fetches and XHRs.
pub const NAVIGATION_PRIORITY: &str = "u=0, i";

/// From which Chromium offers `zstd`. The same release as
/// [`PRIORITY_SINCE_CHROMIUM`], and a constant of its own: two facts that
/// happened to ship together.
const ZSTD_SINCE_CHROMIUM: u32 = 123;

/// What a current browser offers, stated here because the HTTP client would
/// send `zstd,gzip,deflate,br`, which no browser does. Firefox has offered
/// the same four since 126, so there is no non-Chromium branch.
pub const ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";

/// What Chromium offered before it could decode `zstd`.
pub const ACCEPT_ENCODING_BEFORE_ZSTD: &str = "gzip, deflate, br";

/// Language preference, as a browser would state it, read from the system: a
/// Spanish user on a Spanish address announcing `en-US` is an incoherence.
/// Worked out once.
pub fn accept_language() -> &'static str {
    static VALUE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VALUE.get_or_init(|| language_header(system_locale().as_deref()))
}

/// `es-ES,es;q=0.9` is what Chrome produces for one configured language.
/// English is **not** appended: that would be guessing at the person.
fn language_header(locale: Option<&str>) -> String {
    const FALLBACK: &str = "en-US,en;q=0.9";

    let Some(tag) = locale.map(normalize_tag).filter(|t| !t.is_empty()) else {
        return FALLBACK.to_string();
    };
    match tag.split_once('-') {
        Some((language, _)) => format!("{tag},{language};q=0.9"),
        // A bare language is already its own primary preference.
        None => tag,
    }
}

/// The longest language tag taken: what RFC 5646 says an implementation
/// should make room for.
const LONGEST_TAG: usize = 35;

/// The longest subtag of a language tag (RFC 5646).
const LONGEST_SUBTAG: usize = 8;

/// Unix locales arrive as `es_ES.UTF-8`, and may be the C locale, which
/// describes no language.
///
/// **Only what a language tag can hold comes out**: subtags of one to
/// [`LONGEST_SUBTAG`] ASCII letters and digits between hyphens, the first
/// starting with a letter, [`LONGEST_TAG`] bytes at most. Anything else is
/// no tag, as a missing locale is: the value goes into a header.
fn normalize_tag(raw: &str) -> String {
    let tag = raw
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .trim()
        .replace('_', "-");

    let subtag = |subtag: &str| {
        (1..=LONGEST_SUBTAG).contains(&subtag.len())
            && subtag.bytes().all(|b| b.is_ascii_alphanumeric())
    };
    let is_a_tag = tag.len() <= LONGEST_TAG
        && tag.starts_with(|c: char| c.is_ascii_alphabetic())
        && tag.split('-').all(subtag);
    if matches!(tag.as_str(), "C" | "POSIX") || !is_a_tag {
        return String::new();
    }
    tag
}

#[cfg(windows)]
fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

    /// `LOCALE_NAME_MAX_LENGTH`, which the binding does not export.
    const MAX_LOCALE_UNITS: usize = 85;

    let mut buffer = [0u16; MAX_LOCALE_UNITS];
    // SAFETY: the buffer and the length handed over describe the same array,
    // and Windows writes at most that many units into it.
    let written = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
    if written <= 1 {
        return None;
    }
    // The count includes the terminating null.
    String::from_utf16(&buffer[..written as usize - 1]).ok()
}

/// `LC_ALL` overrides `LANG`, as the C library resolves them.
#[cfg(not(windows))]
fn system_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|value| !value.is_empty())
}

/// Build counter of Instagram's own web bundle: a constant that moves when
/// the bundle is rebuilt. `359341` is what Chrome 151 sent on instagram.com
/// in August 2026; refresh it from a capture, never from a search.
pub const ASBD_ID: &str = "359341";

/// What the browser behind a User-Agent would say about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientHints {
    /// `sec-ch-ua`, formatted. `None` for every non-Chromium browser.
    pub ua_brands: Option<String>,
    /// `sec-ch-ua-platform`, quoted.
    pub platform: &'static str,
    /// `sec-ch-ua-mobile`, an sf-boolean.
    pub mobile: &'static str,
    /// `None` for browsers that send no `Priority` on a fetch, old Chromium
    /// included.
    pub priority: Option<&'static str>,
    /// Never absent: what moves with the version is which codecs are in it.
    pub accept_encoding: &'static str,
}

impl ClientHints {
    pub fn from_user_agent(user_agent: &str) -> Self {
        let major = chromium_major(user_agent);
        Self {
            ua_brands: major.map(|major| brands(brand_of(user_agent), major)),
            platform: platform_of(user_agent),
            mobile: if is_mobile(user_agent) { "?1" } else { "?0" },
            priority: major
                .filter(|major| *major >= PRIORITY_SINCE_CHROMIUM)
                .map(|_| FETCH_PRIORITY),
            accept_encoding: match major {
                Some(major) if major < ZSTD_SINCE_CHROMIUM => ACCEPT_ENCODING_BEFORE_ZSTD,
                _ => ACCEPT_ENCODING,
            },
        }
    }
}

/// The major version out of a Chromium User-Agent: since the User-Agent was
/// reduced, the only part that varies.
fn chromium_major(user_agent: &str) -> Option<u32> {
    let after = user_agent.split("Chrome/").nth(1)?;
    after.split('.').next()?.parse().ok()
}

/// The order matters: every Chromium derivative also says `Chrome/`.
fn brand_of(user_agent: &str) -> &'static str {
    if user_agent.contains("Edg/") {
        "Microsoft Edge"
    } else if user_agent.contains("OPR/") {
        "Opera"
    } else {
        "Google Chrome"
    }
}

/// Builds `sec-ch-ua` as Chromium does: the GREASE entry, Chromium and the
/// brand, **the GREASE entry's spelling and the order of the three derived
/// from the major version** (Chromium's `GetGreasedUserAgentBrandVersion`).
fn brands(brand: &str, major: u32) -> String {
    brand_entries(brand, major)
        .iter()
        .map(|(name, version)| format!("\"{name}\";v=\"{version}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What `navigator.userAgentData.brands` holds. For the headless browser,
/// which describes itself as `HeadlessChrome` and has to be corrected.
pub fn brand_list(user_agent: &str) -> Option<Vec<(String, String)>> {
    let major = chromium_major(user_agent)?;
    Some(brand_entries(brand_of(user_agent), major).to_vec())
}

fn brand_entries(brand: &str, major: u32) -> [(String, String); 3] {
    const GREASE_CHARS: [&str; 11] = [" ", "(", ":", "-", ".", "/", ")", ";", "=", "?", "_"];
    const GREASE_VERSIONS: [&str; 3] = ["8", "99", "24"];
    const ORDERS: [[usize; 3]; 6] = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    let seed = major as usize;
    let greased = (
        format!(
            "Not{}A{}Brand",
            GREASE_CHARS[seed % 11],
            GREASE_CHARS[(seed + 1) % 11],
        ),
        GREASE_VERSIONS[seed % 3].to_string(),
    );

    let entries = [
        greased,
        ("Chromium".to_string(), major.to_string()),
        (brand.to_string(), major.to_string()),
    ];

    let order = ORDERS[seed % 6];
    let mut shuffled: [(String, String); 3] = Default::default();
    for (i, entry) in entries.into_iter().enumerate() {
        shuffled[order[i]] = entry;
    }
    shuffled
}

/// The platform a User-Agent claims, unquoted.
pub fn platform_name(user_agent: &str) -> &'static str {
    platform_of(user_agent).trim_matches('"')
}

/// From the enumerated list the specification allows, already quoted.
fn platform_of(user_agent: &str) -> &'static str {
    if user_agent.contains("Android") {
        "\"Android\""
    } else if user_agent.contains("iPhone") || user_agent.contains("iPad") {
        "\"iOS\""
    } else if user_agent.contains("CrOS") {
        "\"Chrome OS\""
    } else if user_agent.contains("Windows") {
        "\"Windows\""
    } else if user_agent.contains("Macintosh") || user_agent.contains("Mac OS X") {
        "\"macOS\""
    } else if user_agent.contains("Linux") || user_agent.contains("X11") {
        "\"Linux\""
    } else {
        "\"Unknown\""
    }
}

fn is_mobile(user_agent: &str) -> bool {
    user_agent.contains("Mobile") || user_agent.contains("Android")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str =
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:142.0) Gecko/20100101 Firefox/142.0";

    fn desktop(major: u32) -> String {
        format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
        )
    }

    /// Checked against real captures. If this drifts, every request the tool
    /// makes carries a combination Chrome never produces.
    #[test]
    fn the_brand_list_matches_what_chromium_builds() {
        assert_eq!(
            brands("Google Chrome", 128),
            r#""Chromium";v="128", "Not;A=Brand";v="24", "Google Chrome";v="128""#
        );
        assert_eq!(
            brands("Google Chrome", 131),
            r#""Google Chrome";v="131", "Chromium";v="131", "Not_A Brand";v="24""#
        );
        assert_eq!(
            brands("Google Chrome", 141),
            r#""Google Chrome";v="141", "Not?A_Brand";v="8", "Chromium";v="141""#
        );
    }

    /// The order of the three entries is derived from the version, not fixed.
    /// Assuming a constant order is wrong two times out of three.
    #[test]
    fn the_order_moves_with_the_version() {
        let first_entry = |major| {
            brands("Google Chrome", major)
                .split(';')
                .next()
                .unwrap()
                .to_string()
        };
        let seen: std::collections::HashSet<String> = (140..146).map(first_entry).collect();
        assert!(
            seen.len() > 1,
            "the leading brand has to change with the version, got {seen:?}"
        );
    }

    #[test]
    fn a_derivative_names_itself_and_keeps_chromium() {
        let edge = brands("Microsoft Edge", 151);
        assert!(edge.contains(r#""Microsoft Edge";v="151""#), "{edge}");
        assert!(edge.contains(r#""Chromium";v="151""#), "{edge}");
    }

    #[test]
    fn the_major_version_is_read_from_the_user_agent() {
        assert_eq!(chromium_major(&desktop(151)), Some(151));
        assert_eq!(chromium_major("Chrome/99.0.4844.51"), Some(99));
        assert_eq!(chromium_major("curl/8.0"), None);
    }

    /// Firefox and Safari send no client hints at all, so inventing them for
    /// their User-Agent would be a mismatch rather than an improvement.
    #[test]
    fn a_browser_that_sends_no_hints_gets_none() {
        let f = ClientHints::from_user_agent(FIREFOX);
        assert_eq!(f.ua_brands, None);
        // The platform is still known, and it still has to agree with the UA.
        assert_eq!(f.platform, "\"Windows\"");
    }

    /// The contradiction this module exists to prevent: a User-Agent naming one
    /// operating system and a platform hint naming another.
    #[test]
    fn the_platform_agrees_with_the_user_agent() {
        let cases = [
            ("Windows NT 10.0; Win64; x64", "\"Windows\""),
            ("Macintosh; Intel Mac OS X 10_15_7", "\"macOS\""),
            ("X11; Linux x86_64", "\"Linux\""),
            ("X11; CrOS x86_64 14541.0.0", "\"Chrome OS\""),
        ];
        for (platform, expected) in cases {
            let ua = format!(
                "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) \
                 Chrome/151.0.0.0 Safari/537.36"
            );
            let f = ClientHints::from_user_agent(&ua);
            assert_eq!(f.platform, expected, "{ua}");
            assert_eq!(f.mobile, "?0");
        }
    }

    #[test]
    fn a_phone_says_so() {
        let android = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                       (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36";
        let f = ClientHints::from_user_agent(android);
        assert_eq!(f.mobile, "?1");
        assert_eq!(f.platform, "\"Android\"");
    }

    /// The shape Chrome produces for a browser configured with one language,
    /// which is what a system locale describes.
    #[test]
    fn the_language_header_follows_the_locale() {
        assert_eq!(language_header(Some("es-ES")), "es-ES,es;q=0.9");
        assert_eq!(language_header(Some("en-US")), "en-US,en;q=0.9");
        // Unix hands it over with an encoding and an underscore attached.
        assert_eq!(language_header(Some("pt_BR.UTF-8")), "pt-BR,pt;q=0.9");
        // A language with no region is already its own preference; a browser
        // does not repeat it at a lower weight.
        assert_eq!(language_header(Some("eu")), "eu");
    }

    /// A tag keeps every shape a system hands out: scripts, numeric regions,
    /// variants, and Windows' sort suffix, which arrives after an underscore.
    #[test]
    fn a_locale_that_is_a_language_tag_is_taken_as_it_was() {
        for (locale, tag) in [
            ("zh-Hans-CN", "zh-Hans-CN"),
            ("sr_RS@latin", "sr-RS"),
            ("ca_ES.UTF-8@valencia", "ca-ES"),
            ("ca-ES-valencia", "ca-ES-valencia"),
            ("es-419", "es-419"),
            ("ast_ES", "ast-ES"),
            ("de-DE_phoneb", "de-DE-phoneb"),
            (" en_GB ", "en-GB"),
            ("sl-rozaj-biske-1994", "sl-rozaj-biske-1994"),
        ] {
            assert_eq!(normalize_tag(locale), tag, "{locale:?}");
        }
        assert_eq!(language_header(Some("zh-Hans-CN")), "zh-Hans-CN,zh;q=0.9");
    }

    /// What no language tag can hold is no tag: it would go into a header
    /// as it came, or keep the request from being built at all.
    #[test]
    fn a_locale_that_is_no_language_tag_falls_back() {
        for garbage in [
            "en US",
            "en\nUS",
            "en\r\nX-Injected: 1",
            "es_ES;q=1",
            "es,fr",
            "es=ES",
            "fran\u{e7}ais",
            "\u{65e5}\u{672c}",
            "es-ES\u{202e}",
            "en_",
            "en--US",
            "abcdefghi",
            "en-abcdefghi",
            "en-US-aaaaaaaa-bbbbbbbb-cccccccc-dddddddd",
            "1337",
            "*",
        ] {
            assert_eq!(normalize_tag(garbage), "", "{garbage:?}");
            assert_eq!(
                language_header(Some(garbage)),
                "en-US,en;q=0.9",
                "{garbage:?}"
            );
        }
    }

    /// The C locale describes no language at all, and must not become one.
    /// Neither must an empty or nonsense value.
    #[test]
    fn a_locale_that_names_no_language_falls_back() {
        for nothing in [None, Some(""), Some("C"), Some("POSIX"), Some("C.UTF-8")] {
            assert_eq!(language_header(nothing), "en-US,en;q=0.9", "{nothing:?}");
        }
    }

    /// Whatever the machine says, the header has to be one a browser could
    /// have sent.
    #[test]
    fn the_real_system_language_is_well_formed() {
        let value = accept_language();
        assert!(!value.is_empty());
        assert!(
            value.starts_with(|c: char| c.is_ascii_alphabetic()),
            "{value}"
        );
        assert!(!value.contains('_'), "{value}");
        assert!(!value.contains(' ') || value.contains(";q="), "{value}");
    }

    /// Chromium started sending `Priority` on fetch at 123. Claiming to be an
    /// older one and sending it anyway is the same mismatch as inventing
    /// client hints for Firefox.
    #[test]
    fn priority_is_only_sent_by_the_versions_that_send_it() {
        assert_eq!(
            ClientHints::from_user_agent(&desktop(151)).priority,
            Some(FETCH_PRIORITY)
        );
        assert_eq!(
            ClientHints::from_user_agent(&desktop(123)).priority,
            Some(FETCH_PRIORITY)
        );
        assert_eq!(ClientHints::from_user_agent(&desktop(122)).priority, None);

        assert_eq!(ClientHints::from_user_agent(FIREFOX).priority, None);
    }

    /// Chromium began offering `zstd` at 123, the release that also started
    /// sending `Priority`, so a Chrome 120 User-Agent -- an old managed
    /// install, or one typed at `--user-agent` -- offers neither: see
    /// [`ZSTD_SINCE_CHROMIUM`].
    #[test]
    fn the_accept_encoding_follows_the_version() {
        assert_eq!(
            ClientHints::from_user_agent(&desktop(151)).accept_encoding,
            ACCEPT_ENCODING
        );
        assert_eq!(
            ClientHints::from_user_agent(&desktop(123)).accept_encoding,
            ACCEPT_ENCODING
        );

        let old = ClientHints::from_user_agent(&desktop(122));
        assert_eq!(old.accept_encoding, ACCEPT_ENCODING_BEFORE_ZSTD);
        assert_eq!(
            old.priority, None,
            "the two move together, and it is the pair that has to agree"
        );

        // Firefox has offered the same four since 126, so a browser this module
        // knows nothing else about still gets the current string rather than an
        // invented one.
        assert_eq!(
            ClientHints::from_user_agent(FIREFOX).accept_encoding,
            ACCEPT_ENCODING
        );
    }

    #[test]
    fn a_full_desktop_hint_set_is_coherent() {
        let f = ClientHints::from_user_agent(&desktop(141));
        assert_eq!(
            f.ua_brands.as_deref(),
            Some(r#""Google Chrome";v="141", "Not?A_Brand";v="8", "Chromium";v="141""#)
        );
        assert_eq!(f.platform, "\"Windows\"");
        assert_eq!(f.mobile, "?0");
    }
}
