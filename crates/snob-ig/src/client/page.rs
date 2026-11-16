//! Requests sent from inside a browser page, the default last hop to Instagram.
//!
//! A `fetch()` from an open instagram.com tab has the browser's own handshake,
//! cookie jar, client hints and `Referer`; `reqwest` has none of them. This
//! crate defines only the shape of such a request and of an intent the tab
//! builds one from ([`crate::web::Ask`]); driving the browser is `snob-cli`'s.
//! Pacing, budgets, cooldowns and classification are the same on both paths.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use snob_core::session::Session;

use crate::web::{Call, Told, Unbuilt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Method {
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PageRequest {
    pub method: Method,
    /// Absolute, on the page's own origin.
    pub url: String,
    /// Only the headers the site's own script adds; the browser adds the rest.
    pub headers: Vec<(String, String)>,
    /// The page the request is made from, absolute: sent as the `Referer`.
    pub referrer: String,
    /// A form-encoded body, for the POSTs built in the tab.
    pub body: Option<String>,
    /// A navigation rather than a fetch. Only the process holding the browser
    /// sends one; handed to it as a request, it is refused.
    pub navigate: bool,
    pub cap: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PageResponse {
    pub status: u16,
    /// Lowercase names. Never `Set-Cookie`: the browser keeps it.
    pub headers: Vec<(String, String)>,
    pub body: String,
    /// Where the request ended up, after any redirect the browser followed.
    pub url: String,
    pub redirected: bool,
    /// Redirects the browser followed. Only a navigation follows any, and it
    /// can also land elsewhere with none counted.
    pub hops: u32,
    /// The body was longer than [`PageRequest::cap`] and was not kept.
    pub too_large: bool,
    /// Milliseconds from the `fetch()` to its headers, timed inside the tab.
    /// `None` for a navigation, or from an owner of an older snob.
    #[serde(default)]
    pub waited_ms: Option<u32>,
}

impl PageResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn waited(&self) -> Option<std::time::Duration> {
        self.waited_ms
            .map(|ms| std::time::Duration::from_millis(u64::from(ms)))
    }

    pub fn load(&self) -> Option<String> {
        super::transport::load_from(|name| self.header(name))
    }

    pub fn served(&self) -> Option<String> {
        super::transport::served_from(|name| self.header(name))
    }
}

/// Why a page handed back no answer.
#[derive(Debug, Clone, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum PageError {
    /// No answer: the network, or the page's own timeout. Worth a retry.
    #[error("{0}")]
    Unreachable(String),
    /// A write, and the browser holds no CSRF token to send it with.
    #[error("the browser holds no CSRF token to write with")]
    NoCsrfToken,
    /// The browser would not start, went away, or stopped answering.
    #[error("{0}")]
    Browser(String),
    /// Instagram pushed back on this session and it was already written down.
    #[error("{}", crate::error::IgError::from(.0.clone()))]
    PushedBack(PushedBack),
    /// Not one of the calls snob may send ([`crate::allowlist`]): never sent.
    #[error("{}", crate::error::IgError::NotAllowed(.0.clone()))]
    NotAllowed(String),
    /// The page has not shown a value the call is built from; nothing sent.
    /// Never [`PageError::Browser`], which would close the browser.
    #[error("{}", crate::error::IgError::PageNotReady(.0.clone()))]
    NotReady(String),
    /// The tab's document is signed out, or served to another account.
    #[error("{}", crate::error::IgError::SessionExpired)]
    LoggedOut,
    /// The account was signed out of on this computer since the session was
    /// read: refused by the caller, never by Instagram.
    #[error("{}", crate::error::IgError::SignedOutHere)]
    SignedOutHere,
}

impl From<PageError> for crate::error::IgError {
    fn from(error: PageError) -> Self {
        match error {
            PageError::Unreachable(why) => Self::Unreachable(why),
            PageError::NoCsrfToken => Self::NoCsrfToken,
            PageError::Browser(why) => Self::Browser(why),
            PageError::PushedBack(cause) => cause.into(),
            PageError::NotAllowed(what) => Self::NotAllowed(what),
            PageError::NotReady(what) => Self::PageNotReady(what),
            PageError::LoggedOut => Self::SessionExpired,
            PageError::SignedOutHere => Self::SignedOutHere,
        }
    }
}

impl From<Unbuilt> for PageError {
    fn from(unbuilt: Unbuilt) -> Self {
        match unbuilt {
            Unbuilt::Missing(missing) => Self::NotReady(missing.0.into()),
            Unbuilt::LoggedOut => Self::LoggedOut,
            Unbuilt::NotAllowed(what) => Self::NotAllowed(what),
        }
    }
}

/// A push-back the browser heard, on the app's calls or on snob's, and has
/// already written down.
///
/// **Never recorded by the client**: it becomes the matching `IgError` through
/// `From`, which records no cooldown. A cooldown repeated within a day
/// doubles, so one push-back written down twice would cost twice.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PushedBack {
    RateLimited,
    FeedbackRequired,
    Challenge { url: Option<String> },
    Checkpoint { url: Option<String> },
}

impl PushedBack {
    pub fn of(error: &crate::error::IgError) -> Option<Self> {
        use crate::error::IgError;
        match error {
            IgError::RateLimited => Some(Self::RateLimited),
            IgError::FeedbackRequired => Some(Self::FeedbackRequired),
            IgError::Challenge { url } => Some(Self::Challenge { url: url.clone() }),
            IgError::Checkpoint { url } => Some(Self::Checkpoint { url: url.clone() }),
            _ => None,
        }
    }
}

impl From<PushedBack> for crate::error::IgError {
    fn from(cause: PushedBack) -> Self {
        match cause {
            PushedBack::RateLimited => Self::RateLimited,
            PushedBack::FeedbackRequired => Self::FeedbackRequired,
            PushedBack::Challenge { url } => Self::Challenge { url },
            PushedBack::Checkpoint { url } => Self::Checkpoint { url },
        }
    }
}

pub type PageFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PageResponse, PageError>> + Send + 'a>>;

pub type AskFuture<'a> = Pin<Box<dyn Future<Output = Result<Told, PageError>> + Send + 'a>>;

/// A browser tab on Instagram that sends what it is given.
pub trait Page: Send + Sync {
    fn send(&self, request: PageRequest) -> PageFuture<'_>;

    /// Answers an intent: the tab builds the request from its document's
    /// values right before sending it ([`crate::web::build_call`]).
    fn ask(&self, call: Call) -> AskFuture<'_> {
        let _ = call;
        Box::pin(std::future::ready(Err(PageError::Browser(
            "this page answers no intents".into(),
        ))))
    }

    /// A push-back the browser has already heard and written down. Asked
    /// before anything is paid for.
    fn heard(&self) -> Option<PushedBack> {
        None
    }
}

/// Called once per client; an implementation shares one browser between them.
pub type PageFactory = Arc<dyn Fn(&Session) -> Arc<dyn Page> + Send + Sync>;

/// Process-global, set once from `main` before any client exists.
static FACTORY: OnceLock<PageFactory> = OnceLock::new();

/// `Err` hands the factory back on a second call.
pub fn send_every_request_from(factory: PageFactory) -> Result<(), PageFactory> {
    FACTORY.set(factory)
}

/// Whether a page factory is set at all, wherever the clients point.
pub fn a_page_factory_is_set() -> bool {
    FACTORY.get().is_some()
}

/// Whether the process chose to send without a browser (`SNOB_NO_BROWSER`:
/// set, neither empty nor `0`).
pub(crate) fn plain_http_chosen() -> bool {
    std::env::var_os("SNOB_NO_BROWSER").is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether a client built now would send from a browser page, for a command
/// that builds no client (`--dry-run`).
pub fn a_client_would_use_a_page() -> bool {
    FACTORY.get().is_some() && (super::is_instagram(&super::site()) || used_off_instagram())
}

pub(crate) fn page_for(session: &Session) -> Option<Arc<dyn Page>> {
    FACTORY.get().map(|factory| allowlisted(factory(session)))
}

/// Every page a client sends through is one of these: whatever is not a call
/// snob may send is refused before it leaves.
pub(crate) fn allowlisted(page: Arc<dyn Page>) -> Arc<dyn Page> {
    Arc::new(Allowlisted(page))
}

struct Allowlisted(Arc<dyn Page>);

impl Page for Allowlisted {
    fn send(&self, request: PageRequest) -> PageFuture<'_> {
        if let Some(what) = crate::allowlist::refused(&request) {
            tracing::error!(request = %what, "refused a call snob may not send");
            return Box::pin(std::future::ready(Err(PageError::NotAllowed(what))));
        }
        self.0.send(request)
    }

    fn ask(&self, call: Call) -> AskFuture<'_> {
        if let Some(what) = crate::allowlist::refused_ask(&call.ask) {
            tracing::error!(request = %what, "refused a call snob may not send");
            return Box::pin(std::future::ready(Err(PageError::NotAllowed(what))));
        }
        self.0.ask(call)
    }

    fn heard(&self) -> Option<PushedBack> {
        self.0.heard()
    }
}

/// Whether a client pointed somewhere other than Instagram uses the page too:
/// a testing build drives the page path against a fake Instagram.
#[cfg(feature = "testing")]
static OFF_INSTAGRAM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(feature = "testing")]
pub fn use_the_page_off_instagram() {
    OFF_INSTAGRAM.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn used_off_instagram() -> bool {
    #[cfg(feature = "testing")]
    {
        OFF_INSTAGRAM.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(feature = "testing"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::super::harness::{Scripted, page_said};
    use super::*;

    fn request(method: Method, path: &str) -> PageRequest {
        PageRequest {
            method,
            url: format!("https://www.instagram.com{path}"),
            headers: Vec::new(),
            referrer: "https://www.instagram.com/".into(),
            body: Some("fb_dtsg=relay%3Atoken".into()),
            navigate: false,
            cap: 1 << 20,
            timeout_ms: 30_000,
        }
    }

    /// An answer from the backend that sends no load still says which one it
    /// was, so the absent load reads as that backend's ordinary answer.
    #[test]
    fn an_answer_says_which_backend_served_it() {
        let mut answer = page_said(500, "");
        answer.headers = vec![
            ("x-stack".into(), "www".into()),
            ("content-type".into(), "text/html; charset=utf-8".into()),
        ];
        assert_eq!(answer.load(), None);
        assert_eq!(
            answer.served().as_deref(),
            Some("x-stack=www content-type=text/html; charset=utf-8")
        );
        assert_eq!(page_said(200, "").served(), None);
    }

    #[tokio::test]
    async fn a_call_snob_may_not_send_never_reaches_the_browser() {
        let browser = Scripted::new(|_| Ok(page_said(200, "{}")));
        let page = allowlisted(browser.clone());

        let error = page
            .send(request(Method::Post, "/api/v1/web/something/"))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, PageError::NotAllowed(what) if what == "POST /api/v1/web/something/"),
            "{error:?}"
        );
        assert!(browser.asked().is_empty());
        let said = crate::error::IgError::from(error).to_string();
        assert!(said.contains("POST /api/v1/web/something/"), "{said}");
        assert!(!said.contains("relay:token"), "{said}");

        // A GET is a friendship list's, or it is not sent either.
        let error = page
            .send(request(Method::Get, "/api/v1/friendships/pending/"))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, PageError::NotAllowed(what) if what == "GET /api/v1/friendships/pending/"),
            "{error:?}"
        );
        assert!(browser.asked().is_empty());
        let answer = page
            .send(request(
                Method::Get,
                "/api/v1/friendships/2345678901/following/",
            ))
            .await
            .unwrap();
        assert_eq!(answer.status, 200);
        assert_eq!(browser.asked().len(), 1);
    }
}
