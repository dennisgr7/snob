//! Client errors and the classification of Instagram's answers: its status
//! codes lie, and the body says why. Advice about what to do is `snob-cli`'s
//! (`report::advice_for`); the messages here are the diagnosis only.

use serde::Deserialize;
use serde::de::IgnoredAny;
use snob_core::EpochMs;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IgError {
    #[error("the session has expired")]
    SessionExpired,

    /// The account was signed out of on this computer since the session was
    /// read. Refused before anything was sent; Instagram said nothing.
    /// Otherwise read as [`IgError::SessionExpired`].
    #[error(
        "this session was signed out on this computer (a logout or a purge, or its data was \
         removed), so nothing more was sent as it"
    )]
    SignedOutHere,

    #[error(
        "Instagram rejected the session because the User-Agent does not match the browser that created it"
    )]
    UserAgentMismatch,

    #[error("Instagram is asking for a security check{}", url_suffix(.url))]
    Challenge { url: Option<String> },

    #[error("Instagram has flagged the account with a checkpoint{}", url_suffix(.url))]
    Checkpoint { url: Option<String> },

    #[error("Instagram is throttling requests; this needs a wait before retrying")]
    RateLimited,

    /// The account is already in cooldown, so this request was never sent.
    ///
    /// **Not [`IgError::RateLimited`]**: that one comes back from Instagram
    /// and opens a cooldown; this is read out of the store, and recording it
    /// would open a second one for a push-back that never happened. `Abort`.
    #[error("the account is in cooldown, so nothing may be spent until it lifts")]
    InCooldown { until_ms: EpochMs },

    #[error("Instagram has temporarily blocked this action")]
    FeedbackRequired,

    #[error("{}", missing_message(.what))]
    NotFound { what: Option<String> },

    #[error("Instagram answered {status}: {body}")]
    Unexpected { status: u16, body: String },

    /// A GraphQL answer whose errors say the call is not worth repeating:
    /// `allow_user_retry` false, or a variable of the wrong type. Nothing is
    /// sent again, under this `doc_id` or a rediscovered one.
    #[error("Instagram refused the call and said repeating it would not help: {body}")]
    QueryRefused { body: String },

    /// A redirect pointed off the origin. Its own variant so that it is an
    /// `Abort` and never a retry with the session on it.
    #[error("a redirect tried to take an API call off instagram.com: {to}")]
    OffOrigin { to: String },

    /// As [`IgError::OffOrigin`], for a chain that is a loop by another name.
    #[error("too many redirects")]
    TooManyRedirects,

    #[error("the response exceeds {limit} bytes, which is more than this could be")]
    TooLarge { limit: usize },

    #[error("could not parse Instagram's response: {0}")]
    Decode(String),

    #[error("could not consult the request budget: {0}")]
    Budget(String),

    /// The browser would not start, went away, or would not answer. `Abort`:
    /// a retry would relaunch a browser.
    #[error("the browser snob sends its requests from failed: {0}")]
    Browser(String),

    /// The browser is fine and the request got no answer: what
    /// [`IgError::Network`] is for `reqwest`, and read the same way.
    #[error("network error in the browser: {0}")]
    Unreachable(String),

    /// The mutation's identifier is not in Instagram's code any more: the
    /// shape changed, not the value, and the user can do nothing about it.
    #[error(
        "Instagram's page no longer carries the identifier for {name}, so this cannot be sent. \
         That is a change on their side, and nothing here can work around it."
    )]
    MutationNotFound { name: &'static str },

    /// The stored session carries no `csrftoken`, so it cannot write. Still a
    /// good session: reads need none. Only without a browser, on a session
    /// pasted without `--csrftoken`.
    #[error("this session has no CSRF token, so it can read but not follow or unfollow")]
    NoCsrfToken,

    /// Not one of the calls snob may send ([`crate::allowlist`]), refused
    /// before it left. A defect in snob, never Instagram's answer.
    #[error("snob refused to send {0}: it is not one of the calls it may make")]
    NotAllowed(String),

    /// The page has not shown a value a call is built from, so nothing was
    /// sent. The browser stays open. `Abort`: a retry would only wait again.
    #[error("Instagram's page has not shown {0} yet, so nothing was sent; try again in a moment")]
    PageNotReady(String),

    #[error("canceled")]
    Canceled,

    /// The budget would have made this wait longer than the caller allowed
    /// (`Pacer::waiting_at_most`): nothing sent, nothing charged.
    /// **Stopping, not throttling**: a walk it stops ends `Canceled`.
    #[error(
        "the request budget would wait {}, longer than the {} allowed, so nothing was sent",
        snob_core::duration::format(*.wait),
        snob_core::duration::format(*.limit)
    )]
    WaitTooLong {
        wait: std::time::Duration,
        limit: std::time::Duration,
    },

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
}

fn url_suffix(url: &Option<String>) -> String {
    match url {
        Some(u) => format!(". Open it in a browser to clear it: {u}"),
        None => ". Open Instagram in a browser or the app to clear it".into(),
    }
}

/// Names the account when the caller knows it. Filtered: the name is whatever
/// was typed on the command line and is printed to a terminal.
fn missing_message(what: &Option<String>) -> String {
    match what {
        Some(name) => format!(
            "the account \"{}\" does not exist",
            snob_core::model::printable(name)
        ),
        None => "Instagram answered 404: what was asked for does not exist".into(),
    }
}

/// How long the account is left alone after this error, and the name it is
/// recorded under. The one table, for the walker and single requests alike.
pub fn cooldown_for(error: &IgError) -> Option<(&'static str, std::time::Duration)> {
    use snob_core::budget::{ACTION_BLOCK_COOLDOWN, CHALLENGE_COOLDOWN, RATE_LIMIT_COOLDOWN};

    match error {
        IgError::FeedbackRequired => Some(("feedback_required", ACTION_BLOCK_COOLDOWN)),
        IgError::RateLimited => Some(("rate_limit", RATE_LIMIT_COOLDOWN)),
        // A hard stop and a cooldown: the user clears it by opening the link,
        // and an unattended rerun must not walk back in meanwhile.
        IgError::Challenge { .. } | IgError::Checkpoint { .. } => {
            Some(("challenge", CHALLENGE_COOLDOWN))
        }
        _ => None,
    }
}

/// Whether an answer is a refusal: anything but a success, or a success that
/// declares a failure in its body (`{"status":"fail"}`, a GraphQL `errors`).
pub fn refused(status: u16, body: &str) -> bool {
    !(200..300).contains(&status) || declares_failure(body)
}

/// The push-back an answer is, if it earns a cooldown. For the browser, which
/// hears answers the client never reads; both use [`refused`], [`classify`]
/// and [`cooldown_for`], or one push-back would be recorded twice.
pub fn push_back(status: u16, body: &str) -> Option<IgError> {
    if !refused(status, body) {
        return None;
    }
    let error = classify(status, body);
    cooldown_for(&error).is_some().then_some(error)
}

/// What the list engine should do about an error. **The single authority**:
/// retry decisions are never made by combining the predicates below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    Retry,
    /// Stop and put the account in cooldown.
    Cooldown,
    /// Stop: retrying within this run cannot help. Whether a cooldown follows
    /// is [`cooldown_for`]'s to say (a challenge says yes to both).
    Abort,
}

impl IgError {
    /// The address that clears a security check, when Instagram gave one.
    pub fn challenge_url(&self) -> Option<&str> {
        match self {
            Self::Challenge { url } | Self::Checkpoint { url } => url.as_deref(),
            _ => None,
        }
    }

    pub fn reaction(&self) -> Reaction {
        match self {
            // A refused redirect is not a network failure (the CDN client's
            // policy refusal arrives this way): each retry would carry the
            // session. Nor is a request that would not build (a header value
            // `reqwest` refuses): it was never sent, and never will be.
            Self::Network(e) if e.is_redirect() || e.is_builder() => Reaction::Abort,
            Self::Network(_) | Self::Unreachable(_) => Reaction::Retry,
            Self::Unexpected { status, .. } if *status >= 500 => Reaction::Retry,
            Self::RateLimited | Self::FeedbackRequired => Reaction::Cooldown,
            // `Decode` lands here on purpose: a body that is not the expected
            // JSON is usually a firewall page.
            _ => Reaction::Abort,
        }
    }

    /// Whether, **while logging in**, the session is worth storing anyway.
    /// Not a retry criterion.
    pub fn is_login_tolerable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited | Self::Network(_) | Self::Unreachable(_)
        )
    }

    /// Whether a second, different request may be sent to answer the same
    /// question another way. Only for a 4xx that is no push-back, no dead
    /// session and no 404: Instagram answered, and its answer was broken (see
    /// `IgClient::web_profile_info`).
    pub fn worth_a_second_route(&self) -> bool {
        matches!(self, Self::Unexpected { status, .. } if (400..500).contains(status))
    }

    /// Whether Instagram objected to the account rather than the request: the
    /// four answers that earn a cooldown, and one already standing.
    pub(crate) fn is_push_back(&self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::FeedbackRequired
                | Self::Challenge { .. }
                | Self::Checkpoint { .. }
                | Self::InCooldown { .. }
        )
    }

    /// Whether it invalidates the stored session, and so must not be persisted.
    pub fn invalidates_session(&self) -> bool {
        matches!(
            self,
            Self::SessionExpired
                | Self::SignedOutHere
                | Self::UserAgentMismatch
                | Self::Challenge { .. }
                | Self::Checkpoint { .. }
        )
    }
}

/// Instagram's error bodies. Every field is optional: the shape varies by
/// endpoint and version.
#[derive(Debug, Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
    #[serde(default)]
    checkpoint_url: Option<String>,
    #[serde(default)]
    challenge: Option<ChallengeBody>,
    #[serde(default)]
    spam: Option<bool>,
    /// When true the problem is the session, even though the message talks
    /// about waiting a few minutes (verified live with an invalid cookie).
    #[serde(default)]
    require_login: Option<bool>,
    #[serde(default)]
    errors: Option<Vec<GraphqlError>>,
}

impl ErrorBody {
    /// Every piece of text a refusal might carry its reason in, joined and
    /// lowercased: the two endpoint families put it in different fields.
    fn searchable(&self) -> String {
        let mut text = self.message.clone().unwrap_or_default();
        for error in self.errors.iter().flatten() {
            for part in [&error.message, &error.summary, &error.description]
                .into_iter()
                .flatten()
            {
                text.push(' ');
                text.push_str(part);
            }
        }
        text.to_ascii_lowercase()
    }
}

/// One entry of a GraphQL `errors` array. A refusal from `/api/graphql` is a
/// 200 with `{"errors":[{...}],"data":null}` and no top-level `message`.
#[derive(Debug, Deserialize)]
struct GraphqlError {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    allow_user_retry: Option<bool>,
}

impl GraphqlError {
    fn not_worth_repeating(&self) -> bool {
        self.allow_user_retry == Some(false)
            || [&self.message, &self.summary, &self.description]
                .into_iter()
                .flatten()
                .any(|text| text.contains("invalid_variable_type"))
    }
}

#[derive(Debug, Deserialize)]
struct ChallengeBody {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    api_path: Option<String>,
}

/// Whether a body that arrived with a successful status is really a failure.
///
/// **Parsed, not searched**: a follower whose full name is `fail` must not
/// stop a walk. This is the gate in front of [`classify`] and so of the
/// cooldown. A body that is not JSON is not a declared failure.
pub fn declares_failure(body: &str) -> bool {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        spam: Option<bool>,
        #[serde(default)]
        require_login: Option<bool>,
        /// A GraphQL refusal: a 200 with `data: null` and the reason in here.
        /// Without it an action block read as a successful unfollow.
        #[serde(default)]
        errors: Option<Vec<IgnoredAny>>,
        /// Only to qualify `errors`: an answer with both is a write that was
        /// performed, and must be neither failed nor replayed. `None` for a
        /// `null`. Skipped, not kept: it is the whole of a GraphQL answer,
        /// and this runs on every one.
        #[serde(default)]
        data: Option<IgnoredAny>,
    }

    let Ok(envelope) = serde_json::from_str::<Envelope>(body) else {
        return false;
    };
    envelope.status.as_deref() == Some("fail")
        || envelope.spam == Some(true)
        || envelope.require_login == Some(true)
        || (envelope.errors.is_some_and(|errors| !errors.is_empty()) && envelope.data.is_none())
}

/// What a navigation redirected to `target` (path and query) means: the
/// challenge, the login form (a session that has gone), or nothing. Whole
/// path segments are matched: `/challenger/` is somebody's profile.
pub fn landed_on(target: &str) -> Option<IgError> {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    if path == "/challenge" || path.starts_with("/challenge/") {
        return Some(IgError::Challenge {
            url: checked_url(target.to_string()),
        });
    }
    if path == "/accounts/login" || path.starts_with("/accounts/login/") {
        return Some(IgError::SessionExpired);
    }
    None
}

/// Translates an Instagram error response into the matching error. Pure.
pub fn classify(status: u16, body: &str) -> IgError {
    let parsed: ErrorBody = serde_json::from_str(body).unwrap_or_default();
    let message = parsed.searchable();

    // Before the message: an invalid cookie answers "Please wait a few
    // minutes" with `require_login: true`, and must not read as throttling.
    if parsed.require_login == Some(true) {
        return IgError::SessionExpired;
    }

    if parsed.spam == Some(true) {
        return IgError::RateLimited;
    }

    if message.contains("useragent mismatch") {
        return IgError::UserAgentMismatch;
    }
    if message.contains("login_required") {
        return IgError::SessionExpired;
    }
    if message.contains("checkpoint_required") {
        return IgError::Checkpoint {
            url: parsed.checkpoint_url.and_then(checked_url),
        };
    }
    if message.contains("challenge_required")
        || parsed.error_type.as_deref() == Some("checkpoint_challenge_required")
    {
        let url = parsed.challenge.as_ref().and_then(|c| {
            c.url
                .clone()
                .and_then(checked_url)
                .or_else(|| c.api_path.clone().and_then(checked_url))
        });
        return IgError::Challenge { url };
    }
    if message.contains("feedback_required") {
        return IgError::FeedbackRequired;
    }
    if message.contains("please wait a few minutes") || message.contains("wait a few minutes") {
        return IgError::RateLimited;
    }
    if (200..300).contains(&status)
        && parsed
            .errors
            .iter()
            .flatten()
            .any(GraphqlError::not_worth_repeating)
    {
        return IgError::QueryRefused {
            body: body_excerpt(body),
        };
    }

    match status {
        429 => IgError::RateLimited,
        401 | 403 => IgError::SessionExpired,
        // A name nobody owns is served as a whole HTML page.
        404 => IgError::NotFound { what: None },
        _ => IgError::Unexpected {
            status,
            body: body_excerpt(body),
        },
    }
}

/// What an unexpected body contributes to a message printed to a terminal:
/// an HTML page is only named, and anything else goes through `printable`.
pub fn body_excerpt(body: &str) -> String {
    if body.trim_start().starts_with('<') {
        return "(an HTML page, not the API's JSON)".into();
    }
    snob_core::model::printable(&body.chars().take(300).collect::<String>())
}

const CHALLENGE_HOSTS: [&str; 3] = ["instagram.com", "www.instagram.com", "i.instagram.com"];

/// The address the user is told to open, if it is Instagram's own. Any other
/// gets the generic wording rather than this program's authority.
fn checked_url(value: String) -> Option<String> {
    let absolute = if value.starts_with("http") {
        value
    } else if value.starts_with('/') {
        format!("https://www.instagram.com{value}")
    } else {
        return None;
    };

    let parsed = url::Url::parse(&absolute).ok()?;
    let host = parsed.host_str()?;
    // The **parsed** address is returned, never the string checked: the
    // parser drops interior tab, CR and LF and encodes C0 controls, so the
    // raw text could pass the host check and still carry control bytes.
    (parsed.scheme() == "https" && CHALLENGE_HOSTS.contains(&host)).then(|| parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one thing a caller can act on: the address that clears the check.
    /// It is held exactly once ever — nothing stores it — so dropping it means
    /// provoking the error again to see it.
    #[test]
    fn a_challenge_hands_over_its_address() {
        let url = "https://www.instagram.com/challenge/123/abc/";
        for e in [
            IgError::Challenge {
                url: Some(url.into()),
            },
            IgError::Checkpoint {
                url: Some(url.into()),
            },
        ] {
            assert_eq!(e.challenge_url(), Some(url));
        }

        // Everything else has no address, and must not invent one.
        assert_eq!(IgError::SessionExpired.challenge_url(), None);
        assert_eq!(IgError::Challenge { url: None }.challenge_url(), None);
    }

    /// What counts as a push-back is exactly what earns a cooldown, whoever
    /// heard it.
    #[test]
    fn a_push_back_is_exactly_what_earns_a_cooldown() {
        assert!(matches!(push_back(429, ""), Some(IgError::RateLimited)));
        assert!(matches!(
            push_back(200, r#"{"status":"fail","spam":true}"#),
            Some(IgError::RateLimited)
        ));
        assert!(matches!(
            push_back(400, r#"{"message":"feedback_required"}"#),
            Some(IgError::FeedbackRequired)
        ));
        assert!(matches!(
            push_back(400, r#"{"message":"challenge_required"}"#),
            Some(IgError::Challenge { .. })
        ));
        assert!(push_back(200, r#"{"status":"ok"}"#).is_none());
        assert!(push_back(200, "<html>a page</html>").is_none());
        // Refusals with no cooldown are not push-backs: a dead session, a
        // missing account, a server having a bad day.
        assert!(push_back(401, "").is_none());
        assert!(push_back(404, "").is_none());
        assert!(push_back(500, "").is_none());
        assert!(push_back(200, r#"{"require_login":true}"#).is_none());
    }

    /// A 5xx the edge proxy answered in text, as it does when the backend
    /// failed (made-up values in the capture's shape): the server's problem,
    /// with no cooldown.
    #[test]
    fn a_text_server_error_from_the_proxy_is_not_a_push_back() {
        for body in ["", "<html>server error</html>"] {
            assert!(push_back(500, body).is_none(), "{body:?}");
            let e = classify(500, body);
            assert!(
                matches!(e, IgError::Unexpected { status: 500, .. }),
                "{e:?}"
            );
            assert!(cooldown_for(&e).is_none());
            assert!(!e.is_push_back());
        }
    }

    /// A GraphQL refusal of the request itself, in the capture's shape with
    /// made-up values: an error, not a push-back, and not repeated.
    const REFUSED_QUERY: &str = r#"{"errors":[{"message":"invalid_variable_type: a variable has the wrong type",
        "severity":"CRITICAL","code":1000001,"is_silent":false,"is_transient":false,
        "is_not_critical":false,"requires_reauth":false,"allow_user_retry":false}],
        "extensions":{"is_final":true}}"#;

    #[test]
    fn a_graphql_error_that_says_not_to_retry_is_refused_and_not_repeated() {
        assert!(push_back(200, REFUSED_QUERY).is_none());
        let e = classify(200, REFUSED_QUERY);
        assert!(matches!(e, IgError::QueryRefused { .. }), "{e:?}");
        assert_eq!(e.reaction(), Reaction::Abort);
        assert!(!e.worth_a_second_route());

        // Either sign is enough.
        let flag_only = r#"{"errors":[{"message":"x","allow_user_retry":false}]}"#;
        let class_only = r#"{"errors":[{"summary":"invalid_variable_type"}]}"#;
        for body in [flag_only, class_only] {
            assert!(
                matches!(classify(200, body), IgError::QueryRefused { .. }),
                "{body}"
            );
        }
        // A push-back in the same shape is still the push-back.
        let blocked = r#"{"errors":[{"message":"feedback_required","allow_user_retry":false}]}"#;
        assert!(matches!(classify(200, blocked), IgError::FeedbackRequired));
        // And an error that says neither is what it was.
        assert!(matches!(
            classify(200, r#"{"errors":[{"message":"x"}]}"#),
            IgError::Unexpected { status: 200, .. }
        ));
    }

    /// A navigation that ends on the challenge is one, and earns its cooldown;
    /// one that ends on the login form is a session that has gone.
    #[test]
    fn where_a_navigation_lands_is_an_answer() {
        assert!(matches!(
            landed_on("/challenge/action/AbC/"),
            Some(IgError::Challenge { url: Some(_) })
        ));
        assert!(cooldown_for(&landed_on("/challenge/").unwrap()).is_some());
        assert!(matches!(
            landed_on("/accounts/login/"),
            Some(IgError::SessionExpired)
        ));
        assert!(landed_on("/someone/").is_none());
        assert!(landed_on("/challenger/").is_none());
        assert!(landed_on("/challenges_x/?next=/").is_none());
        let Some(IgError::Challenge { url: Some(url) }) = landed_on("/challenge/x/?next=%2F")
        else {
            panic!("a challenge keeps its query");
        };
        assert!(url.ends_with("/challenge/x/?next=%2F"), "{url}");
    }

    #[test]
    fn an_expired_session() {
        let e = classify(403, r#"{"message":"login_required","status":"fail"}"#);
        assert!(matches!(e, IgError::SessionExpired));
        assert!(e.invalidates_session());
        assert!(!e.is_login_tolerable());
    }

    #[test]
    fn an_expired_session_with_the_extended_body() {
        let body = r#"{"message":"login_required","error_title":"You've Been Logged Out","error_body":"Please log back in.","logout_reason":8,"status":"fail"}"#;
        assert!(matches!(classify(403, body), IgError::SessionExpired));
    }

    #[test]
    fn a_mismatched_user_agent() {
        let e = classify(401, r#"{"message":"useragent mismatch","status":"fail"}"#);
        assert!(matches!(e, IgError::UserAgentMismatch));
        // Through the function that decides it, rather than a predicate nothing
        // consulted: `reaction` is what the retry loop asks.
        assert_eq!(e.reaction(), Reaction::Abort);
    }

    /// Regression: during a walk, throttling is **never** retried, however
    /// tolerable it may be while logging in. Two different questions, and
    /// confusing them is how you end up hammering Instagram.
    #[test]
    fn throttling_is_never_retried() {
        assert_eq!(IgError::RateLimited.reaction(), Reaction::Cooldown);
        assert!(IgError::RateLimited.is_login_tolerable());
    }

    #[test]
    fn server_errors_are_retried() {
        let e = IgError::Unexpected {
            status: 503,
            body: String::new(),
        };
        assert_eq!(e.reaction(), Reaction::Retry);
    }

    #[test]
    fn an_unreadable_response_is_not_retried() {
        // It is usually a firewall page; pushing on is how you enter a loop.
        assert_eq!(IgError::Decode("oops".into()).reaction(), Reaction::Abort);
    }

    #[test]
    fn a_dead_session_is_not_retried() {
        assert_eq!(IgError::SessionExpired.reaction(), Reaction::Abort);
        assert_eq!(IgError::Challenge { url: None }.reaction(), Reaction::Abort);
    }

    #[test]
    fn a_challenge_with_a_url() {
        let body = r#"{"message":"challenge_required","error_type":"checkpoint_challenge_required","challenge":{"api_path":"/challenge/424242/Ch4LLeNGe1/","url":"https://i.instagram.com/challenge/424242/Ch4LLeNGe1/","native_flow":true,"lock":true,"logout":false},"status":"fail"}"#;
        match classify(400, body) {
            IgError::Challenge { url } => {
                assert_eq!(
                    url.as_deref(),
                    Some("https://i.instagram.com/challenge/424242/Ch4LLeNGe1/")
                );
            }
            other => panic!("expected a challenge, got {other:?}"),
        }
    }

    #[test]
    fn a_checkpoint_with_a_relative_path_is_made_absolute() {
        let body = r#"{"message":"checkpoint_required","checkpoint_url":"/challenge/424242/Ch4LLeNGe2/","lock":false,"status":"fail"}"#;
        match classify(400, body) {
            IgError::Checkpoint { url } => {
                assert_eq!(
                    url.as_deref(),
                    Some("https://www.instagram.com/challenge/424242/Ch4LLeNGe2/")
                );
            }
            other => panic!("expected a checkpoint, got {other:?}"),
        }
    }

    /// Instagram's real answer (July 2026) to an invalid cookie. The message
    /// misleads: it talks about waiting, but `require_login` gives away that
    /// the session is the problem.
    #[test]
    fn an_invalid_cookie_is_not_mistaken_for_throttling() {
        let body = r#"{"message":"Please wait a few minutes before you try again.","require_login":true,"igweb_rollout":true,"status":"fail"}"#;
        let e = classify(401, body);
        assert!(
            matches!(e, IgError::SessionExpired),
            "it was classified as {e:?}"
        );
        assert!(e.invalidates_session());
    }

    /// Two `contains` over the raw text would stop the walk on any account
    /// named "fail": every good answer carries `"status":"ok"`, so the other
    /// half is always true. The body is read typed instead.
    #[test]
    fn an_account_named_fail_does_not_look_like_one() {
        let page = r#"{"users":[{"pk":1,"username":"fail","full_name":"fail"}],"status":"ok"}"#;
        assert!(!declares_failure(page));

        let quoted =
            r#"{"users":[{"pk":1,"username":"a","full_name":"say \"fail\" again"}],"status":"ok"}"#;
        assert!(!declares_failure(quoted));
    }

    #[test]
    fn a_declared_failure_is_recognized() {
        assert!(declares_failure(
            r#"{"message":"","spam":true,"status":"fail"}"#
        ));
        // Order does not matter: it is a field, not a position.
        assert!(declares_failure(r#"{"status":"fail","message":"x"}"#));
    }

    /// This gate is what stands in front of the cooldown. A throttling answer
    /// that skips it stops the run — which is safe — without writing anything
    /// down, so the next run walks straight back into the same wall.
    #[test]
    fn throttling_without_the_status_field_is_still_a_failure() {
        assert!(declares_failure(r#"{"spam":true}"#));
        assert!(declares_failure(r#"{"require_login":true,"message":"x"}"#));

        // And the flags being false is not a failure.
        assert!(!declares_failure(r#"{"spam":false,"status":"ok"}"#));
    }

    /// A body with no status, or one that is not JSON at all, is not a
    /// declared failure. What it is instead is the deserializer's to say.
    #[test]
    fn a_body_without_a_status_is_not_a_failure() {
        assert!(!declares_failure(r#"{"users":[]}"#));
        assert!(!declares_failure("<html>a firewall page</html>"));
        assert!(!declares_failure(""));
    }

    #[test]
    fn the_spam_flag_means_throttling() {
        let e = classify(400, r#"{"message":"","spam":true,"status":"fail"}"#);
        assert!(matches!(e, IgError::RateLimited));
    }

    #[test]
    fn wait_a_few_minutes_means_throttling() {
        let body =
            r#"{"message":"Please wait a few minutes before you try again.","status":"fail"}"#;
        let e = classify(401, body);
        assert!(matches!(e, IgError::RateLimited));
        assert!(e.is_login_tolerable());
        assert!(!e.invalidates_session());
    }

    /// The three causes that leave a mark, and the shape of each. A challenge
    /// is the one waiting does not fix, so it gets its own much shorter
    /// length rather than the action block's.
    #[test]
    fn every_pushback_records_a_cooldown_of_its_own_size() {
        let length = |e: &IgError| cooldown_for(e).map(|(_, d)| d);

        let throttle = length(&IgError::RateLimited).unwrap();
        let block = length(&IgError::FeedbackRequired).unwrap();
        let challenge = length(&IgError::Challenge { url: None }).unwrap();
        let checkpoint = length(&IgError::Checkpoint { url: None }).unwrap();

        assert_eq!(challenge, checkpoint, "both mean the same thing");
        assert!(
            challenge < throttle,
            "waiting is not what clears a challenge"
        );
        assert!(throttle < block);

        // A dead session is not push-back and must not put the account in
        // cooldown: logging in again is what fixes it.
        assert_eq!(length(&IgError::SessionExpired), None);
        assert_eq!(length(&IgError::Decode("x".into())), None);
    }

    #[test]
    fn feedback_required_stops_the_run() {
        let e = classify(400, r#"{"message":"feedback_required","status":"fail"}"#);
        assert!(matches!(e, IgError::FeedbackRequired));
        assert_eq!(e.reaction(), Reaction::Cooldown);
    }

    #[test]
    fn a_429_means_throttling_even_with_no_body() {
        assert!(matches!(classify(429, ""), IgError::RateLimited));
    }

    #[test]
    fn a_body_that_is_not_json_does_not_blow_up() {
        let e = classify(500, "<html>internal error</html>");
        assert!(matches!(e, IgError::Unexpected { status: 500, .. }));
    }

    #[test]
    fn an_unexpected_body_is_truncated() {
        let long = "x".repeat(5000);
        match classify(500, &long) {
            IgError::Unexpected { body, .. } => assert!(body.chars().count() <= 300),
            other => panic!("expected Unexpected, got {other:?}"),
        }
    }

    /// A username nobody owns answers 404 with a whole web page. What the user
    /// needs to read is that the account is not there.
    #[test]
    fn a_404_means_the_thing_does_not_exist() {
        let e = classify(
            404,
            "<!DOCTYPE html><html><body>Page Not Found</body></html>",
        );
        assert!(matches!(e, IgError::NotFound { what: None }), "{e:?}");
        assert_eq!(e.reaction(), Reaction::Abort);
        let message = e.to_string();
        assert!(!message.contains("<html"), "{message}");
        assert!(message.contains("does not exist"), "{message}");
    }

    #[test]
    fn an_unexpected_html_body_is_named_rather_than_dumped() {
        match classify(500, "<!DOCTYPE html><html>a firewall page</html>") {
            IgError::Unexpected { body, .. } => {
                assert!(!body.contains('<'), "{body}");
                assert!(body.contains("HTML"), "{body}");
            }
            other => panic!("expected Unexpected, got {other:?}"),
        }
    }

    /// An address that passed the check cannot still be carrying what the check
    /// was for.
    ///
    /// The parsed URL and the raw string are not the same text: the parser
    /// skips interior tab, CR and LF and percent-encodes C0 controls in the
    /// path, so both of these reach `www.instagram.com` and satisfy the host
    /// check. What comes back is the parsed one, without their control bytes.
    #[test]
    fn an_offered_challenge_url_cannot_carry_control_characters() {
        for hostile in [
            // Written as the JSON escapes they arrive as: a raw control
            // character inside a JSON string is not JSON, so a body carrying
            // one never reaches this check at all. These do.
            "/challenge/424242/Ch4LLeNGe1/\\u001b[2K\\u001b[A",
            "https://www.instagram.com\\u000a/challenge/x",
            "https://www.\\u0009instagram.com/challenge/x",
        ] {
            let body = format!(
                r#"{{"message":"challenge_required","challenge":{{"url":"{hostile}"}},"status":"fail"}}"#
            );
            let IgError::Challenge { url } = classify(400, &body) else {
                panic!("expected a challenge for {hostile:?}");
            };
            let Some(url) = url else { continue };
            assert!(
                !url.chars().any(|c| c.is_control()),
                "{url:?} was offered with control characters in it"
            );
            // And the whole rendered sentence, which is what reaches the
            // terminal.
            let message = IgError::Challenge { url: Some(url) }.to_string();
            assert!(
                !message.chars().any(|c| c.is_control()),
                "{message:?} reaches a terminal"
            );
        }
    }

    /// The message says "Open it in a browser to clear it", so the address
    /// carries this tool's authority. A body that names somewhere else loses
    /// the convenience rather than sending someone to a login form that is not
    /// Instagram's.
    #[test]
    fn a_challenge_url_that_is_not_instagram_is_not_offered() {
        for elsewhere in [
            "https://evil.test/challenge/",
            "http://www.instagram.com/challenge/",
            "https://www.instagram.com.evil.test/challenge/",
            "https://evil.test/#www.instagram.com",
            "javascript:alert(1)",
        ] {
            let body = format!(
                r#"{{"message":"challenge_required","challenge":{{"url":"{elsewhere}"}},"status":"fail"}}"#
            );
            match classify(400, &body) {
                IgError::Challenge { url } => {
                    assert_eq!(url, None, "{elsewhere} should not have been offered")
                }
                other => panic!("expected a challenge, got {other:?}"),
            }
        }

        // The generic wording still tells the user what to do.
        let message = IgError::Challenge { url: None }.to_string();
        assert!(message.contains("Open Instagram in a browser"), "{message}");
    }

    /// A response body is no more trustworthy than a profile field, and this
    /// excerpt is printed to a terminal.
    #[test]
    fn an_unexpected_body_cannot_drive_the_terminal() {
        let hostile = format!("{esc}[2K{esc}[A gone", esc = '\x1b');
        match classify(500, &hostile) {
            IgError::Unexpected { body, .. } => assert!(!body.contains('\x1b'), "{body:?}"),
            other => panic!("expected Unexpected, got {other:?}"),
        }
    }

    #[test]
    fn a_404_names_the_account_when_the_caller_knows_it() {
        let e = IgError::NotFound {
            what: Some("someone".into()),
        };
        assert_eq!(e.to_string(), "the account \"someone\" does not exist");
    }

    /// A header value `reqwest` refuses fails the request before it is sent:
    /// retried, the walker would pay the budget three more times for nothing.
    #[test]
    fn a_request_that_does_not_build_is_not_retried() {
        let unbuilt = reqwest::Client::new()
            .get("http://127.0.0.1:9/")
            .header("X-FB-LSD", "a token\nwith a line break")
            .build()
            .expect_err("a header value with a line break does not build");
        assert!(unbuilt.is_builder());
        assert_eq!(IgError::Network(unbuilt).reaction(), Reaction::Abort);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) })]

        /// An HTTP status and body from Instagram, JSON or anything else, can
        /// classify into an error variant without panicking (release builds
        /// abort on panic).
        #[test]
        fn classifying_any_status_and_body_never_panics(
            status in any::<u16>(),
            body in prop_oneof![
                any::<String>(),
                (any::<String>(), any::<String>(), any::<String>()).prop_map(
                    |(msg, url, title)| {
                        serde_json::json!({
                            "message": msg,
                            "status": "fail",
                            "challenge": { "url": url },
                            "feedback_title": title,
                        })
                        .to_string()
                    }
                ),
            ],
        ) {
            let _ = classify(status, &body);
        }
    }
}
