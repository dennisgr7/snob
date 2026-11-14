//! The Instagram session the tool reuses.
//!
//! The two credential fields are [`Secret`]; anything added here that is
//! itself a credential belongs in the same type.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::secret::Secret;
use crate::{Epoch, Pk};

pub const SESSION_SCHEMA_VERSION: u32 = 1;

/// What the Windows Credential Manager accepts: `CRED_MAX_CREDENTIAL_BLOB_SIZE`,
/// 5*512 **bytes** of the UTF-16 the keyring backend writes. Above it, the
/// session is stored as [`Session::minimal`].
pub const MAX_KEYRING_SECRET_BYTES: usize = 5 * 512;

/// How much room a string takes in the Windows Credential Manager: two bytes
/// per UTF-16 unit, so four for a character outside the Basic Multilingual
/// Plane.
pub fn keyring_bytes(text: &str) -> usize {
    text.encode_utf16().count() * 2
}

/// Longest accepted User-Agent. Real ones run to about 130 characters.
const MAX_USER_AGENT_CHARS: usize = 512;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("the sessionid is empty")]
    EmptySessionId,
    #[error("unrecognized sessionid format: it should start with your numeric account id")]
    MalformedSessionId,
    #[error("the User-Agent is empty")]
    EmptyUserAgent,
    #[error("that does not look like a browser User-Agent: it should start with \"Mozilla/5.0\"")]
    SuspiciousUserAgent,
    #[error("the User-Agent is too long ({0} characters)")]
    UserAgentTooLong(usize),
    #[error("the stored session uses format {found}, and this version understands {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
}

/// Where the session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOrigin {
    Browser,
    Paste,
}

impl SessionOrigin {
    /// The stable token of the serialized form and of JSON output; `Display`
    /// is the wording for a person and may change.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Browser => "browser",
            Self::Paste => "paste",
        }
    }
}

impl std::fmt::Display for SessionOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Browser => "browser",
            Self::Paste => "manual paste",
        })
    }
}

/// `Debug` is derived, and safe to derive: both credential fields are
/// [`Secret`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub schema_version: u32,
    pub ds_user_id: Pk,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub sessionid: Secret,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csrftoken: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ig_did: Option<String>,
    /// Meta's browser cookie. Absent from a paste.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datr: Option<String>,
    pub user_agent: String,
    /// The user gave this User-Agent explicitly, so no refresh may rewrite it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub user_agent_pinned: bool,
    /// When the installed browser's version was last compared against the
    /// User-Agent. Absent means never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent_checked_at: Option<Epoch>,
    /// Which browser the User-Agent describes, so a refresh on a machine with
    /// several follows the one the session was made in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    pub origin: SessionOrigin,
    pub created_at: Epoch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_at: Option<Epoch>,
}

impl Session {
    /// Builds a session from the `sessionid` and the User-Agent of the browser
    /// that produced it. The account id is read off the start of the
    /// `sessionid`.
    pub fn from_sessionid(
        raw_sessionid: &str,
        user_agent: &str,
        origin: SessionOrigin,
    ) -> Result<Self, SessionError> {
        let sessionid = clean_sessionid(raw_sessionid)?;
        let ds_user_id = extract_pk(sessionid)?;
        let user_agent = validate_user_agent(user_agent)?;

        Ok(Self {
            schema_version: SESSION_SCHEMA_VERSION,
            ds_user_id,
            username: None,
            sessionid: Secret::new(sessionid),
            csrftoken: None,
            mid: None,
            ig_did: None,
            datr: None,
            user_agent,
            user_agent_pinned: false,
            user_agent_checked_at: None,
            browser: None,
            origin,
            created_at: crate::clock::now(),
            validated_at: None,
        })
    }

    /// Which session this is, without being it: sixteen hex digits of its
    /// SHA-256, safe to write beside a browser profile and compare.
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.sessionid.expose().as_bytes())
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Value of the `Cookie` header. The `sessionid` goes as the browser
    /// handed it over, undecoded.
    ///
    /// **One buffer of the exact size, cleared on drop**: this runs on every
    /// request, and a `String` that grows leaves each outgrown buffer, a
    /// plaintext copy of the cookie, behind in freed memory.
    pub fn cookie_header(&self) -> Zeroizing<String> {
        let pk = self.ds_user_id.to_string();
        // An empty value is left out: a cookie present with no value is not an
        // absent one to Instagram, and no browser sends that shape.
        let optional = [
            ("csrftoken", self.csrftoken.as_ref().map(Secret::expose)),
            ("mid", self.mid.as_deref()),
            ("ig_did", self.ig_did.as_deref()),
            ("datr", self.datr.as_deref()),
        ]
        .map(|(name, value)| (name, value.filter(|v| !v.is_empty())));

        const SESSIONID: &str = "sessionid=";
        const DS_USER_ID: &str = "; ds_user_id=";
        let capacity = SESSIONID.len()
            + self.sessionid.expose().len()
            + DS_USER_ID.len()
            + pk.len()
            + optional
                .iter()
                .filter_map(|(name, value)| value.map(|v| "; ".len() + name.len() + 1 + v.len()))
                .sum::<usize>();

        let mut out = Zeroizing::new(String::with_capacity(capacity));
        out.push_str(SESSIONID);
        out.push_str(self.sessionid.expose());
        out.push_str(DS_USER_ID);
        out.push_str(&pk);
        for (name, value) in optional {
            if let Some(value) = value {
                out.push_str("; ");
                out.push_str(name);
                out.push('=');
                out.push_str(value);
            }
        }
        debug_assert_eq!(out.len(), capacity, "the cookie outgrew its buffer");
        out
    }

    pub fn check_schema(&self) -> Result<(), SessionError> {
        if self.schema_version > SESSION_SCHEMA_VERSION {
            return Err(SessionError::UnsupportedSchema {
                found: self.schema_version,
                expected: SESSION_SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    pub fn mark_validated(&mut self) {
        self.validated_at = Some(crate::clock::now());
    }

    /// Only what the session cannot work without, for a store that cannot take
    /// the full record.
    #[must_use]
    pub fn minimal(&self) -> Self {
        Self {
            username: None,
            csrftoken: None,
            mid: None,
            ig_did: None,
            datr: None,
            ..self.clone()
        }
    }
}

/// Tolerates what people paste: whitespace, the `sessionid=` pair, quotes, and
/// the whole `Cookie` row, of which only the sessionid is kept (the value ends
/// at the first `;`).
///
/// **No whitespace or control character inside it**: no cookie value holds
/// one, and the value goes into a header as it is.
fn clean_sessionid(raw: &str) -> Result<&str, SessionError> {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("sessionid=") {
        s = rest.trim();
    }
    s = s.trim_matches(['"', '\'']).trim();
    s = s.split(';').next().unwrap_or(s).trim();
    s = s.trim_matches(['"', '\'']).trim();

    if s.is_empty() {
        return Err(SessionError::EmptySessionId);
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(SessionError::MalformedSessionId);
    }
    Ok(s)
}

/// The account a `sessionid` belongs to, however its colon is written: a
/// pasted `42:abc`, or the `42%3Aabc` Instagram rotates it to.
pub fn account_in(sessionid: &str) -> Option<Pk> {
    extract_pk(sessionid).ok()
}

/// The `sessionid` starts with the account's numeric id, up to a colon that
/// usually arrives escaped as `%3A`.
///
/// Read in place: a normalized copy would be a copy of the credential that
/// nothing clears.
fn extract_pk(sessionid: &str) -> Result<Pk, SessionError> {
    let head = [":", "%3A", "%3a"]
        .iter()
        .filter_map(|separator| sessionid.find(separator))
        .min()
        .map(|end| &sessionid[..end])
        .filter(|head| !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit()))
        .ok_or(SessionError::MalformedSessionId)?;

    head.parse::<Pk>()
        .map_err(|_| SessionError::MalformedSessionId)
}

fn validate_user_agent(ua: &str) -> Result<String, SessionError> {
    let ua = ua.trim().trim_matches(['"', '\'']).trim();
    if ua.is_empty() {
        return Err(SessionError::EmptyUserAgent);
    }
    let length = ua.chars().count();
    if length > MAX_USER_AGENT_CHARS {
        return Err(SessionError::UserAgentTooLong(length));
    }
    if !ua.starts_with("Mozilla/5.0") {
        return Err(SessionError::SuspiciousUserAgent);
    }
    Ok(ua.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
    const SID: &str = "71234567890%3AAbCdEfGhIjKl%3A20%3AAYc123";
    /// The account every `sessionid` in this module begins with.
    const PK: Pk = Pk::new(71234567890);

    fn session() -> Session {
        Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap()
    }

    #[test]
    fn the_account_id_is_taken_from_the_sessionid() {
        assert_eq!(session().ds_user_id, PK);
    }

    #[test]
    fn an_unescaped_colon_is_accepted() {
        let s = Session::from_sessionid("71234567890:AbCd:20", UA, SessionOrigin::Paste).unwrap();
        assert_eq!(s.ds_user_id, PK);
    }

    /// However the colon is written, the account is the same one, and a
    /// longer id that begins with the same digits is another.
    #[test]
    fn the_account_in_a_sessionid_is_read_however_its_colon_is_written() {
        for sessionid in [
            "71234567890%3AAbCd%3A20",
            "71234567890%3aAbCd%3a20",
            "71234567890:AbCd:20",
        ] {
            assert_eq!(account_in(sessionid), Some(PK), "{sessionid}");
        }
        assert_ne!(account_in("712345678901%3AAbCd"), Some(PK));
        assert_eq!(account_in("noseparator"), None);
    }

    #[test]
    fn it_tolerates_how_people_paste() {
        for input in [
            "  71234567890%3AAbCd%3A20  ",
            "sessionid=71234567890%3AAbCd%3A20",
            "\"71234567890%3AAbCd%3A20\"",
            "71234567890%3AAbCd%3A20;",
        ] {
            let s = Session::from_sessionid(input, UA, SessionOrigin::Paste)
                .unwrap_or_else(|e| panic!("\"{input}\" should be accepted, but failed: {e}"));
            assert_eq!(s.ds_user_id, PK);
            assert_eq!(s.sessionid.expose(), "71234567890%3AAbCd%3A20");
        }
    }

    #[test]
    fn an_invalid_sessionid_is_rejected() {
        for input in ["", "   ", "noseparator", "%3Anoprefix", "abc%3Adef"] {
            assert!(
                Session::from_sessionid(input, UA, SessionOrigin::Paste).is_err(),
                "\"{input}\" should be rejected"
            );
        }
    }

    #[test]
    fn a_user_agent_that_is_not_a_browser_is_rejected() {
        assert!(matches!(
            Session::from_sessionid(SID, "curl/8.0", SessionOrigin::Paste),
            Err(SessionError::SuspiciousUserAgent)
        ));
        assert!(matches!(
            Session::from_sessionid(SID, "  ", SessionOrigin::Paste),
            Err(SessionError::EmptyUserAgent)
        ));
        let long = format!("Mozilla/5.0 {}", "x".repeat(600));
        assert!(matches!(
            Session::from_sessionid(SID, &long, SessionOrigin::Paste),
            Err(SessionError::UserAgentTooLong(_))
        ));
    }

    #[test]
    fn the_sessionid_travels_undecoded_in_the_cookie() {
        let header = session().cookie_header();
        assert!(header.contains("sessionid=71234567890%3AAbCdEfGhIjKl%3A20%3AAYc123"));
        assert!(header.contains("ds_user_id=71234567890"));
    }

    #[test]
    fn the_cookie_omits_absent_fields() {
        let header = session().cookie_header();
        assert!(!header.contains("csrftoken"));
        assert!(!header.contains("mid="));
    }

    /// A cookie sent with an empty value is worse than one not sent at all:
    /// Instagram's session and CSRF checks read it as present-but-blank, which
    /// is a shape no browser produces. Absent fields are already left out
    /// above; this pins that a field that exists but is empty is too, so a
    /// later `unwrap_or_default` cannot quietly start emitting `mid=`.
    #[test]
    fn no_cookie_is_ever_sent_with_an_empty_value() {
        let mut s = session();
        s.csrftoken = Some(Secret::new(""));
        s.mid = Some(String::new());
        s.ig_did = Some(String::new());
        s.datr = Some(String::new());

        let header = s.cookie_header();
        for pair in header.split("; ") {
            let (name, value) = pair.split_once('=').expect("every cookie is a pair");
            assert!(!value.is_empty(), "{name} went out empty: {}", *header);
        }
    }

    #[test]
    fn debug_does_not_leak_the_credential() {
        let mut s = session();
        s.csrftoken = Some(Secret::new("SeCrEtToKeN"));
        let dump = format!("{s:?}");
        assert!(
            !dump.contains("AbCdEfGhIjKl") && !dump.contains("SeCrEtToKeN"),
            "Debug leaked a credential: {dump}"
        );
        assert!(dump.contains("<hidden>"));
        // Derived rather than written out, so a field added later cannot go
        // missing from the dump.
        assert!(dump.contains("browser"), "{dump}");
        assert!(dump.contains("user_agent_pinned"), "{dump}");
    }

    /// Copying the whole `Cookie` row out of the developer tools is the
    /// commonest way to get this wrong, and `clean_sessionid` says what it
    /// would cost.
    #[test]
    fn pasting_the_whole_cookie_string_keeps_only_the_sessionid() {
        let whole_row = "sessionid=71234567890%3AAbCd%3A20; csrftoken=abc123; mid=ZZZ; ig_did=Q";
        let s = Session::from_sessionid(whole_row, UA, SessionOrigin::Paste).unwrap();

        assert_eq!(s.sessionid.expose(), "71234567890%3AAbCd%3A20");
        assert_eq!(s.ds_user_id, PK);
        assert!(!s.cookie_header().contains("csrftoken=abc123"));
    }

    #[test]
    fn serialization_round_trip() {
        let original = session();
        let json = serde_json::to_string(&original).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ds_user_id, original.ds_user_id);
        assert_eq!(back.sessionid.expose(), original.sessionid.expose());
        assert_eq!(back.user_agent, original.user_agent);
        assert_eq!(back.origin, original.origin);
    }

    /// The machine-readable token must not follow the human wording around.
    #[test]
    fn the_origin_token_is_independent_of_its_display_text() {
        assert_eq!(SessionOrigin::Paste.as_str(), "paste");
        assert_eq!(SessionOrigin::Paste.to_string(), "manual paste");
        assert_eq!(
            serde_json::to_string(&SessionOrigin::Paste).unwrap(),
            r#""paste""#
        );
    }

    /// Every optional field set at once, because the ceiling only matters for
    /// the fullest session there can be. A test that leaves the newest fields
    /// out stops measuring what it claims to measure the moment one is added.
    #[test]
    fn a_realistic_session_fits_the_windows_keyring() {
        let mut s = session();
        s.username = Some("a_fairly_long_username".into());
        s.csrftoken = Some("x".repeat(32).into());
        s.mid = Some("y".repeat(28));
        s.ig_did = Some("Z".repeat(36));
        s.user_agent_pinned = true;
        s.user_agent_checked_at = Some(Epoch::new(1_722_700_000));
        s.browser = Some("Chrome".into());
        s.validated_at = Some(Epoch::new(1_722_700_000));
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            keyring_bytes(&json) < MAX_KEYRING_SECRET_BYTES,
            "it takes {} bytes, over the Windows ceiling of {MAX_KEYRING_SECRET_BYTES}",
            keyring_bytes(&json)
        );
    }

    /// Windows measures the secret in UTF-16 bytes, not in characters. A
    /// character outside the Basic Multilingual Plane is four bytes, so
    /// counting characters let a value pass this check and then be refused by
    /// Windows with an error that explains nothing.
    #[test]
    fn the_ceiling_is_measured_the_way_windows_measures_it() {
        assert_eq!(keyring_bytes("abc"), 6);
        // Two UTF-16 units, one character.
        assert_eq!("\u{1f600}".chars().count(), 1);
        assert_eq!(keyring_bytes("\u{1f600}"), 4);

        // A string under the ceiling in characters can still be over it in
        // the bytes Windows counts.
        let past_the_ceiling = "\u{1f600}".repeat(MAX_KEYRING_SECRET_BYTES / 4 + 1);
        assert!(past_the_ceiling.chars().count() < MAX_KEYRING_SECRET_BYTES);
        assert!(keyring_bytes(&past_the_ceiling) > MAX_KEYRING_SECRET_BYTES);
    }

    #[test]
    fn a_newer_format_than_we_understand_is_rejected() {
        let mut s = session();
        s.schema_version = SESSION_SCHEMA_VERSION + 1;
        assert!(matches!(
            s.check_schema(),
            Err(SessionError::UnsupportedSchema { .. })
        ));
    }

    /// The value goes into a `Cookie` header as it is: a line break or a
    /// space inside it is two cookies pasted as one, or a header cut short.
    #[test]
    fn a_sessionid_with_whitespace_or_a_control_inside_is_rejected() {
        for input in [
            "71234567890%3AAbCd\r\nX-Injected: 1",
            "71234567890%3AAbCd csrftoken=abc123",
            "71234567890%3AAb\tCd",
            "71234567890%3AAb\u{0}Cd",
        ] {
            assert!(
                matches!(
                    Session::from_sessionid(input, UA, SessionOrigin::Paste),
                    Err(SessionError::MalformedSessionId)
                ),
                "{input:?} should be rejected"
            );
        }
    }

    /// The id is digits and nothing else: `u64`'s own parser takes a sign.
    #[test]
    fn the_account_id_is_digits_only() {
        assert_eq!(account_in("+71234567890%3AAbCd"), None);
        assert_eq!(account_in("%3A71234567890"), None);
        assert_eq!(account_in("7123456789012345678901234567890:AbCd"), None);
    }

    /// A buffer that grows leaves the outgrown one, a copy of the cookie,
    /// uncleared: the header is built in one of its exact size, however long
    /// the session's values are.
    #[test]
    fn the_cookie_header_never_outgrows_its_buffer() {
        let mut s = session();
        s.csrftoken = Some(Secret::new("c".repeat(700)));
        s.mid = Some("m".repeat(300));
        s.ig_did = Some(String::new());
        s.datr = Some("d".repeat(40));

        let header = s.cookie_header();
        assert_eq!(header.capacity(), header.len());
        assert!(header.ends_with(&format!("; datr={}", "d".repeat(40))));
        assert!(!header.contains("ig_did"));
    }
}
