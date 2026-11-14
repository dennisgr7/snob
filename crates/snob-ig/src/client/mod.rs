//! HTTP client against Instagram's web API (`www.instagram.com`, never the
//! mobile hosts: cookie, User-Agent and endpoint have to agree).
//!
//! One `IgClient`, its `impl` split by concern: [`transport`] (how a request
//! goes out, redirects, ceilings), [`headers`], [`read`], [`write`] (the only
//! file that sends a method other than GET), [`cdn`] and [`media`], [`page`]
//! (the browser tab and its allowlist), [`ask`], [`posts`].

mod ask;
mod cdn;
mod headers;
mod media;
pub mod page;
mod posts;
mod read;
mod transport;
mod write;

#[cfg(test)]
pub(crate) mod harness;

pub use self::cdn::CdnClient;
pub use self::posts::{PostsPage, ProfileTab};
pub use self::read::Direction;

use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use snob_core::session::Session;
use url::Url;

use crate::BASE_URL;
use crate::client_hints::ClientHints;
use crate::error::{IgError, classify, refused};
use crate::pace::{Pace, Pacer};

use self::transport::{Answer, api_policy, build_client, same_origin};

pub struct IgClient {
    /// Carries the session, and follows no redirect on its own: see [`api_policy`].
    api: reqwest::Client,
    /// Carries nothing that identifies the account. A second client because
    /// its redirect rule is the opposite of the API's. Built on first use:
    /// assembling one reads the platform trust store.
    cdn: OnceLock<CdnClient>,
    base: Url,
    session: Session,
    /// Reserving budget lives here, so a request never paid for cannot be written.
    pacer: Pacer,
    hints: ClientHints,
    /// Instagram's session-continuity token. See [`IgClient::claim`].
    claim: Mutex<String>,
    /// The browser tab requests are sent from. `None` sends with `reqwest`:
    /// under `SNOB_NO_BROWSER`, and to a test server.
    page: Option<std::sync::Arc<dyn page::Page>>,
    /// The `X-Web-Session-ID` of the client that sends without a browser.
    web_session: String,
    /// The highlights tray the last profile opened was read with. See
    /// `IgClient::profile_by_pk`.
    held_tray: Mutex<Option<ask::HeldTray>>,
    /// Whether a `show_many` came back unbuilt for want of the app's token:
    /// the next ones are not paid for. See `IgClient::statuses`.
    statuses_unbuilt: AtomicBool,
}

/// What a browser sends before the server has told it anything.
const INITIAL_CLAIM: &str = "0";

fn is_instagram(url: &Url) -> bool {
    static LIVE: std::sync::LazyLock<Url> =
        std::sync::LazyLock::new(|| Url::parse(BASE_URL).expect("BASE_URL parses"));
    same_origin(url, &LIVE)
}

/// Where every client built after [`point_every_client_at`] points.
///
/// **Compiled out of a release build**, which is why it is a Cargo feature
/// and not a hidden flag. It sets the base URL, not a "skip the pace" switch:
/// [`IgClient::is_live`] answers by address. Set once from `main`; the binary
/// refuses `--ig-base-url` without `--sandbox-root`.
#[cfg(feature = "testing")]
static SANDBOX_BASE: std::sync::OnceLock<Url> = std::sync::OnceLock::new();

/// `Err` carries the base already set, on a second call.
#[cfg(feature = "testing")]
pub fn point_every_client_at(base: Url) -> Result<(), Url> {
    SANDBOX_BASE.set(base)
}

/// The site a client built now points at: Instagram, or in a testing build
/// the server [`point_every_client_at`] named.
pub fn site() -> Url {
    #[cfg(feature = "testing")]
    if let Some(base) = SANDBOX_BASE.get() {
        return base.clone();
    }
    Url::parse(BASE_URL).expect("BASE_URL parses")
}

impl IgClient {
    pub fn new(session: Session, pacer: Pacer) -> Result<Self, IgError> {
        #[cfg(feature = "testing")]
        if let Some(base) = SANDBOX_BASE.get() {
            return Self::pointed_at(session, pacer, base.clone());
        }
        Self::pointed_at(session, pacer, Url::parse(BASE_URL)?)
    }

    fn pointed_at(session: Session, pacer: Pacer, base: Url) -> Result<Self, IgError> {
        let page = if is_instagram(&base) || page::used_off_instagram() {
            page::page_for(&session)
        } else {
            None
        };
        Ok(Self {
            page,
            web_session: headers::new_web_session(),
            hints: ClientHints::from_user_agent(&session.user_agent),
            api: build_client(&session.user_agent, api_policy())?,
            cdn: OnceLock::new(),
            base,
            session,
            pacer,
            claim: Mutex::new(INITIAL_CLAIM.to_string()),
            held_tray: Mutex::new(None),
            statuses_unbuilt: AtomicBool::new(false),
        })
    }

    pub(in crate::client) fn cdn(&self) -> Result<&CdnClient, IgError> {
        if let Some(cdn) = self.cdn.get() {
            return Ok(cdn);
        }
        let built = CdnClient::new(
            &self.session.user_agent,
            self.base.clone(),
            self.pacer.cancel_token().clone(),
        )?;
        Ok(self.cdn.get_or_init(|| built))
    }

    /// Classifies Instagram's answer and, if it was a push-back, records the
    /// cooldown before handing the error on.
    pub(in crate::client) fn classify_and_record(&self, status: u16, body: &str) -> IgError {
        self.record(classify(status, body))
    }

    /// For an answer classified some other way (a navigation's landing page).
    pub(in crate::client) fn record(&self, error: IgError) -> IgError {
        if let Some((reason, minimum)) = crate::error::cooldown_for(&error) {
            // A cooldown that cannot be written must not hide the real error.
            if let Err(e) = self.pacer.start_cooldown(reason, minimum) {
                tracing::warn!(error = %e, "could not record the cooldown");
            }
        }
        error
    }

    /// Points the client at a different server. Tests only.
    ///
    /// **This is also what turns the pace off**, through [`IgClient::is_live`].
    /// Rebuilds rather than assigns: the redirect policies are decided from
    /// the base URL when the client is made.
    #[doc(hidden)]
    #[must_use]
    pub fn with_base_url(self, base: Url) -> Self {
        Self::pointed_at(self.session, self.pacer, base)
            .expect("rebuilding a client that already exists cannot fail")
    }

    #[cfg(test)]
    pub(crate) fn through(mut self, page: std::sync::Arc<dyn page::Page>) -> Self {
        self.page = Some(page::allowlisted(page));
        self
    }

    /// Sends through `page`, allowlist and all. After [`Self::with_base_url`],
    /// which builds the client again without one.
    #[cfg(feature = "testing")]
    #[doc(hidden)]
    #[must_use]
    pub fn with_page(mut self, page: std::sync::Arc<dyn page::Page>) -> Self {
        self.page = Some(page::allowlisted(page));
        self
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Whether this client sends from a browser page.
    pub fn has_page(&self) -> bool {
        self.page.is_some()
    }

    /// Whether this client can send a write: it needs a CSRF token, the
    /// browser's own from the tab, or the stored one with `reqwest`.
    pub fn can_write(&self) -> bool {
        self.page.is_some() || self.session.csrftoken.is_some()
    }

    pub fn pacer(&self) -> &Pacer {
        &self.pacer
    }

    /// Whether this client is pointed at Instagram itself, which decides
    /// whether the pace is real. **Asked of the base URL, never set by a
    /// caller**: nobody can ask to walk Instagram with no waits.
    pub fn is_live(&self) -> bool {
        is_instagram(&self.base)
    }

    /// Waits a step between two pages of one action ([`Pace::step`]) and
    /// answers whether it was canceled. Only against Instagram; lengthened
    /// when the latest answers were slow (`Pacer::slowed`).
    pub async fn step_between(&self, pace: &Pace) -> bool {
        if !self.is_live() {
            return false;
        }
        let step = self
            .pacer
            .slowed(pace, pace.step(), snob_core::clock::now_ms())
            .await;
        self.pacer.cancel_token().sleep_or_cancel(step).await
    }

    /// Waits as a person looks at a profile before opening one of its lists
    /// ([`Pace::dwell`]); otherwise as [`Self::step_between`].
    pub async fn dwell_before_a_list(&self, pace: &Pace) -> bool {
        if !self.is_live() {
            return false;
        }
        let dwell = self
            .pacer
            .slowed(pace, pace.dwell(), snob_core::clock::now_ms())
            .await;
        self.pacer.cancel_token().sleep_or_cancel(dwell).await
    }

    /// Writes down how long an answer took to arrive, under `operation`
    /// (`transport::named`): what the pace slows down on.
    pub(in crate::client) async fn timed(
        &self,
        operation: String,
        status: u16,
        waited: Option<Duration>,
        load: Option<String>,
        via: snob_core::budget::Via,
    ) {
        // Status 0 is a redirect the page did not follow: nothing to time.
        let Some(waited) = waited.filter(|_| status != 0) else {
            return;
        };
        self.pacer
            .answered(snob_core::budget::AnswerTime {
                at: snob_core::clock::now_ms(),
                operation,
                status,
                waited,
                load,
                via,
            })
            .await;
    }

    /// Charges the day's accounts for the `accounts` a read named. A failure
    /// to record it is logged, not raised: the read was already paid for.
    pub(in crate::client) async fn charge_accounts(&self, accounts: usize) {
        let accounts = u32::try_from(accounts).unwrap_or(u32::MAX);
        if let Err(e) = self.pacer.spend_accounts(accounts).await {
            tracing::warn!(error = %e, "could not record the accounts a read carried");
        }
    }

    /// Pays the rest a sitting owes, if any (`Pacer::rest_owed`), before a
    /// page read by hand; `Ok(true)` when canceled. `tell` hears how long the
    /// rest is before it begins. Against a test server it is counted paid at
    /// once.
    pub async fn rest_before_a_page(
        &self,
        pace: &Pace,
        tell: impl FnOnce(Duration),
    ) -> Result<bool, IgError> {
        let Some((until, left)) = self
            .pacer
            .rest_owed(pace, snob_core::clock::now_ms())
            .await?
        else {
            return Ok(false);
        };
        tell(left);
        if self.is_live() && self.pacer.cancel_token().sleep_or_cancel(left).await {
            return Ok(true);
        }
        self.pacer.rested(until).await?;
        Ok(false)
    }

    /// Turns an answer into the value asked for or an error, recording a
    /// cooldown if it earned one. [`refused`] is the one rule for "a 200 can
    /// still be a failure".
    pub(in crate::client) fn decode<T: DeserializeOwned>(
        &self,
        answer: &Answer,
    ) -> Result<T, IgError> {
        let body = answer.body.as_str();
        if refused(answer.status, body) {
            return Err(self.refuse(answer));
        }

        serde_json::from_str(body).map_err(|e| {
            IgError::Decode(format!(
                "{e} - response: {}",
                crate::error::body_excerpt(body)
            ))
        })
    }

    /// [`IgClient::decode`] for a registry query asked of the page, whose
    /// `data` is under `root` ([`crate::allowlist::Operation::root_field`]).
    ///
    /// **An answer whose `data` lacks `root` does not decode**: every field of
    /// the models is optional, so it would read as an empty account.
    pub(in crate::client) fn decode_query<T: DeserializeOwned>(
        &self,
        answer: &Answer,
        root: &str,
    ) -> Result<T, IgError> {
        self.decode(answer)
            .and_then(|decoded| match under_another_field(&answer.body, root) {
                Some(fields) => Err(IgError::Decode(format!(
                    "the answer's data is under {fields:?}, not {root:?}"
                ))),
                None => Ok(decoded),
            })
            .inspect_err(|error| {
                if matches!(error, IgError::Decode(_)) {
                    note_undecoded(answer);
                }
            })
    }

    /// The one way a push-back leaves the client: classified, recorded, noted.
    pub(in crate::client) fn refuse(&self, answer: &Answer) -> IgError {
        let error = self.classify_and_record(answer.status, &answer.body);
        self.note_push_back(answer, &error);
        error
    }

    /// Logs what a push-back looked like and changes nothing: `Retry-After`
    /// is measured, not acted on. **If it is ever acted on, it is a floor and
    /// never a ceiling** for the local cooldown. `warn` for a push-back,
    /// `debug` for any other refusal.
    pub(in crate::client) fn note_push_back(&self, answer: &Answer, error: &IgError) {
        let endpoint = answer.endpoint.as_str();
        let status = answer.status;
        let retry_after = answer.retry_after.as_deref().unwrap_or("<absent>");
        let load = answer.load.as_deref().unwrap_or("<absent>");
        let served = answer.served.as_deref().unwrap_or("<absent>");
        let body = crate::error::body_excerpt(&answer.body);
        if crate::error::cooldown_for(error).is_some() {
            let before = answered_before(&self.pacer.latest_answers());
            tracing::warn!(endpoint, status, retry_after, load, served, %before, %body, "Instagram pushed back");
        } else {
            tracing::debug!(endpoint, status, retry_after, load, served, %body, "Instagram refused");
        }
    }
}

/// `followers 200 420ms, followers 200 2300ms`, oldest first.
fn answered_before(answers: &[snob_core::budget::AnswerTime]) -> String {
    if answers.is_empty() {
        return "<none timed>".to_string();
    }
    answers
        .iter()
        .map(|a| format!("{} {} {}ms", a.operation, a.status, a.waited.as_millis()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The names of the fields under `data` when it is an object without `root`.
fn under_another_field(body: &str, root: &str) -> Option<Vec<String>> {
    let answer: serde_json::Value = serde_json::from_str(body).ok()?;
    let data = answer.get("data")?.as_object()?;
    (!data.contains_key(root)).then(|| data.keys().cloned().collect())
}

/// Logs the shape of a query's answer that did not decode: key names, never
/// a value.
fn note_undecoded(answer: &Answer) {
    let endpoint = answer.endpoint.as_str();
    let served = answer.served.as_deref().unwrap_or("<absent>");
    let guarded = answer.body.starts_with("for (;;);");
    let json = answer
        .body
        .strip_prefix("for (;;);")
        .unwrap_or(&answer.body);
    let data: Option<Vec<String>> = serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|answer| Some(answer.get("data")?.as_object()?.keys().cloned().collect()));
    tracing::debug!(
        endpoint,
        served,
        guarded,
        ?data,
        "a query's answer did not decode"
    );
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::client::harness::{answering, client, watching};

    #[test]
    fn a_push_back_says_the_answers_before_it() {
        let answer = |status, ms| snob_core::budget::AnswerTime {
            at: snob_core::EpochMs::new(0),
            operation: "followers".to_string(),
            status,
            waited: Duration::from_millis(ms),
            load: None,
            via: snob_core::budget::Via::Browser,
        };
        assert_eq!(answered_before(&[]), "<none timed>");
        assert_eq!(
            answered_before(&[answer(200, 420), answer(429, 2_300)]),
            "followers 200 420ms, followers 429 2300ms"
        );
    }

    /// Every push-back is written down, with the length its own cause earns.
    ///
    /// `cooldown_for` is the table and it is tested on its own, but a table
    /// nobody reads is not a rule. This drives the four causes through a real
    /// request and asks the budget what arrived.
    #[tokio::test]
    async fn every_push_back_puts_the_account_in_cooldown() {
        use crate::error::cooldown_for;

        let cases = [
            (429, "", IgError::RateLimited),
            (
                400,
                r#"{"message":"","spam":true,"status":"fail"}"#,
                IgError::RateLimited,
            ),
            (
                400,
                r#"{"message":"feedback_required","status":"fail"}"#,
                IgError::FeedbackRequired,
            ),
            (
                400,
                r#"{"message":"challenge_required","status":"fail"}"#,
                IgError::Challenge { url: None },
            ),
        ];

        for (status, body, expected) in cases {
            let server = answering(status, body).await;
            let (client, budget) = watching(&server.uri());
            client.validate().await.unwrap_err();

            let (reason, minimum) = cooldown_for(&expected).expect("this cause earns a cooldown");
            assert_eq!(
                budget.calls(),
                vec![(reason.to_string(), minimum)],
                "status {status} with body {body:?} should record one cooldown"
            );
        }
    }

    /// A dead session is not push-back, and must not put the account in
    /// cooldown: logging in again is what fixes it, and a cooldown would refuse
    /// the very command that fixes it.
    #[tokio::test]
    async fn a_dead_session_records_nothing() {
        let server = answering(403, r#"{"message":"login_required","status":"fail"}"#).await;
        let (client, budget) = watching(&server.uri());

        let error = client.validate().await.unwrap_err();
        assert!(matches!(error, IgError::SessionExpired));
        assert!(budget.calls().is_empty());
    }

    /// Only a `data` object that lacks the root field is another field's
    /// answer; a `data` that is `null` or absent is left to the model.
    #[test]
    fn an_answer_is_under_another_field_only_when_data_lacks_the_root() {
        let root = "user";
        assert_eq!(under_another_field(r#"{"data":{"user":null}}"#, root), None);
        assert_eq!(
            under_another_field(r#"{"data":{"user":{},"viewer":{}}}"#, root),
            None
        );
        assert_eq!(under_another_field(r#"{"data":null}"#, root), None);
        assert_eq!(under_another_field(r#"{"errors":[]}"#, root), None);
        assert_eq!(
            under_another_field(r#"{"data":{"xig_user":{"pk":"1"},"viewer":{}}}"#, root),
            Some(vec!["viewer".to_string(), "xig_user".to_string()])
        );
        assert_eq!(
            under_another_field(r#"{"data":{}}"#, root),
            Some(Vec::new())
        );
    }

    /// A 200 whose body will not parse is a `Decode` error with the same
    /// excerpt as every other error: filtered, and named as an HTML page. A
    /// captive portal answering 200 with a login page is the ordinary way to
    /// reach this.
    #[tokio::test]
    async fn a_body_that_will_not_parse_is_excerpted_like_every_other_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/42/following/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body>{esc}[2K{esc}[A Sign in to the network</body></html>",
                esc = '\x1b'
            )))
            .mount(&server)
            .await;

        let error = client(&server)
            .await
            .validate()
            .await
            .expect_err("that is not the API's JSON");
        assert!(matches!(error, IgError::Decode(_)), "{error:?}");
        let message = error.to_string();

        assert!(!message.contains('\x1b'), "{message:?}");
        assert!(message.contains("an HTML page"), "{message}");
    }
}
