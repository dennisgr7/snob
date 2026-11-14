//! How a request goes out to Instagram's API and how the answer comes back:
//! the origin rule, the redirect loop that pays for every hop, the ceilings on
//! what is read, and the races against the cancel token.

use serde::de::DeserializeOwned;
use url::Url;

use snob_core::budget::Via;

use crate::error::IgError;
use crate::http::Deadline;

use super::IgClient;
use super::headers::Surface;

/// A page of twelve accounts is a few kilobytes; the body is read into memory
/// whole, so the ceiling keeps a hostile answer from deciding how much.
pub(super) const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// Generous: the pacing keeps requests apart, these only keep anything from
/// waiting forever. For a file off the CDN, the longest wait without a byte.
pub(super) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub(super) const MAX_HOPS: usize = 3;

/// Same scheme, host and port: a different port or scheme on the same name is
/// a different server.
pub(super) fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// None: following a redirect is a request, and `IgClient::get_body` follows
/// them itself to pay for and check every hop.
///
/// **The write goes out on this client too, and depends on this staying
/// `none()`**: reqwest repeats a POST on a 307 or 308, which would be a follow
/// or an unfollow nobody confirmed.
pub(super) fn api_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::none()
}

/// A client under the trust the run chose (`http::use_trust`).
fn client_with(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
    deadline: Deadline,
) -> Result<reqwest::Client, IgError> {
    Ok(crate::http::builder(
        user_agent,
        redirect,
        CONNECT_TIMEOUT,
        deadline,
        &crate::http::chosen_trust(),
    )?
    .build()?)
}

pub(super) fn build_client(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
) -> Result<reqwest::Client, IgError> {
    client_with(user_agent, redirect, Deadline::Whole(REQUEST_TIMEOUT))
}

/// [`build_client`] for files off the CDN (see [`Deadline::BetweenReads`]).
pub(super) fn build_cdn_client(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
) -> Result<reqwest::Client, IgError> {
    client_with(
        user_agent,
        redirect,
        Deadline::BetweenReads(REQUEST_TIMEOUT),
    )
}

/// Reads a response body, refusing one that will not fit.
pub(super) async fn read_capped(response: reqwest::Response, cap: u64) -> Result<String, IgError> {
    Ok(utf8_or_lossy(read_capped_bytes(response, cap).await?))
}

/// One ceiling for the API body and the CDN download. `http::read_capped` is
/// a different rule: it truncates rather than refusing.
pub(super) async fn read_capped_bytes(
    response: reqwest::Response,
    cap: u64,
) -> Result<Vec<u8>, IgError> {
    // Sized from the declaration when there is one (in practice the CDN),
    // capped so a lying header cannot reserve more than would be accepted.
    let mut bytes: Vec<u8> = Vec::with_capacity(
        response
            .content_length()
            .map_or(0, |declared| declared.min(cap) as usize),
    );
    stream_capped(response, cap, &mut bytes).await?;
    Ok(bytes)
}

/// **The one copy of the chunked read**, into a `Vec` or a file, refusing past
/// `cap`. Returns the bytes written; on `TooLarge` the sink holds what arrived
/// before the ceiling.
pub(super) async fn stream_capped(
    mut response: reqwest::Response,
    cap: u64,
    sink: &mut (impl std::io::Write + Send),
) -> Result<u64, IgError> {
    let too_large = || IgError::TooLarge {
        limit: cap as usize,
    };

    if let Some(declared) = response.content_length()
        && declared > cap
    {
        return Err(too_large());
    }

    let mut written: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        if written + chunk.len() as u64 > cap {
            return Err(too_large());
        }
        sink.write_all(&chunk)
            .map_err(|e| IgError::Decode(format!("could not write the download: {e}")))?;
        written += chunk.len() as u64;
    }
    Ok(written)
}

/// Moves the buffer when it is valid UTF-8 rather than copying it.
fn utf8_or_lossy(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

pub(super) fn load_of(response: &reqwest::Response) -> Option<String> {
    // Read on every answer, whatever the log level: a test reads it from
    // `Answer` with no subscriber installed.
    let headers = response.headers();
    load_from(|name| headers.get(name).and_then(|value| value.to_str().ok()))
}

const LOAD_HEADERS: [&str; 2] = ["x-ig-capacity-level", "x-ig-peak-time"];

/// Which backend answered: only one of the two sends the load headers.
const SERVED_HEADERS: [&str; 2] = ["x-stack", "content-type"];

pub(super) fn load_from<'a>(lookup: impl Fn(&str) -> Option<&'a str>) -> Option<String> {
    joined(&LOAD_HEADERS, lookup)
}

pub(super) fn served_from<'a>(lookup: impl Fn(&str) -> Option<&'a str>) -> Option<String> {
    joined(&SERVED_HEADERS, lookup)
}

pub(super) fn served_of(response: &reqwest::Response) -> Option<String> {
    let headers = response.headers();
    served_from(|name| headers.get(name).and_then(|value| value.to_str().ok()))
}

fn joined<'a>(names: &[&str], lookup: impl Fn(&str) -> Option<&'a str>) -> Option<String> {
    let found: Vec<String> = names
        .iter()
        .filter_map(|name| lookup(name).map(|value| format!("{name}={value}")))
        .collect();
    (!found.is_empty()).then(|| found.join(" "))
}

/// Kept as a string: the header has two legal forms, and which one these
/// endpoints send, if either, is what the logging exists to find out.
pub(super) fn retry_after(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// One answer from Instagram. **Nothing decides anything from `retry_after`,
/// `load` or `served`**: see [`IgClient::note_push_back`].
pub(super) struct Answer {
    /// Only the path: the query can carry a name.
    pub(super) endpoint: String,
    pub(super) status: u16,
    pub(super) body: String,
    pub(super) retry_after: Option<String>,
    /// `x-ig-capacity-level` and `x-ig-peak-time`, joined.
    pub(super) load: Option<String>,
    /// `x-stack` and `content-type`, joined.
    pub(super) served: Option<String>,
}

/// The name an answer's time is written down under (`Pacer::answered`): the
/// call's, never its path, which carries the account. `None` for a document;
/// `other` for anything unknown.
pub(super) fn named(path: &str, style: &Surface<'_>) -> Option<String> {
    match style {
        Surface::Document => return None,
        Surface::Relay { friendly_name, .. } => return Some((*friendly_name).to_string()),
        Surface::App => {}
    }
    let name = match path {
        crate::web::BULK_ROUTE_DEFINITIONS => "pk",
        crate::web::NAVIGATION => "navigation",
        crate::web::SHOW_MANY => "show_many",
        "/api/graphql" | "/graphql/query" => "graphql",
        _ => {
            let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
            let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            match segments.as_slice() {
                ["api", "v1", "friendships", pk, read] if numeric(pk) => match *read {
                    "followers" => "followers",
                    "following" => "following",
                    "mutual_followers" => "mutual_followers",
                    _ => "other",
                },
                ["api", "v1", "media", pk, "info"] if numeric(pk) => "media_info",
                ["api", "v1", "media", pk, "comments"] if numeric(pk) => "comments",
                _ => "other",
            }
        }
    };
    Some(name.to_string())
}

impl Answer {
    /// Half the question: a 200 can still declare failure in the body
    /// ([`crate::error::refused`]).
    pub(super) fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

impl IgClient {
    /// `referer` is the path of the page the call would have come from in a
    /// browser, without the leading slash.
    pub(super) async fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
        referer: &str,
    ) -> Result<T, IgError> {
        let answer = self.get_body(path, query, referer, Surface::App).await?;
        self.decode(&answer)
    }

    /// Sends the request and follows any redirect itself, **paying for every
    /// hop** (`Pacer::clear_to_send` sits inside the loop).
    ///
    /// Every hop is held to the origin: `X-CSRFToken` is not on the list of
    /// headers reqwest drops across hosts. **A refusal has to reach
    /// [`Reaction::Abort`]**, hence `OffOrigin` and `TooManyRedirects` rather
    /// than `Network`, which is retried. The query goes on the first request
    /// only.
    pub(super) async fn get_body(
        &self,
        path: &str,
        query: &[(&str, &str)],
        referer: &str,
        style: Surface<'_>,
    ) -> Result<Answer, IgError> {
        if let Some(page) = &self.page {
            return self
                .get_body_in_page(page.as_ref(), path, query, referer, style)
                .await;
        }

        let mut url = self.base.join(path)?;
        let mut query: Option<&[(&str, &str)]> = Some(query);
        let mut hops: usize = 0;

        loop {
            tracing::debug!(%url, "GET");

            self.pacer.clear_to_send().await?;

            let sent = std::time::Instant::now();
            let response = self
                .send_or_cancel(self.api_request(&url, query, referer, style))
                .await?;
            let waited = sent.elapsed();
            self.remember_claim(&response);
            let status = response.status();

            if status.is_redirection() {
                let next = self.next_hop(&url, &response)?;
                // A hop to the challenge or the login form is already the
                // answer; it is not sent.
                if let Some(refused) = crate::error::landed_on(
                    &next[url::Position::BeforePath..url::Position::AfterQuery],
                ) {
                    return Err(self.record(refused));
                }
                if hops >= MAX_HOPS {
                    return Err(IgError::TooManyRedirects);
                }
                hops += 1;
                url = next;
                query = None;
                continue;
            }

            // The status is in hand, and a body that will not read must not
            // turn a 429 into `Network`, which is retried and records no
            // cooldown.
            let retry_after = retry_after(&response);
            let load = load_of(&response);
            let served = served_of(&response);
            if let Some(operation) = named(url.path(), &style) {
                self.timed(
                    operation,
                    status.as_u16(),
                    Some(waited),
                    load.clone(),
                    Via::Direct,
                )
                .await;
            }

            let body = match self.read_or_cancel(response, MAX_BODY_BYTES).await {
                Ok(body) => body,
                // A canceled read is the user, never a push-back to record.
                Err(IgError::Canceled) => return Err(IgError::Canceled),
                Err(_) if !status.is_success() => {
                    let answer = Answer {
                        endpoint: url.path().to_string(),
                        status: status.as_u16(),
                        body: String::new(),
                        retry_after,
                        load,
                        served,
                    };
                    return Err(self.refuse(&answer));
                }
                Err(e) => return Err(e),
            };
            return Ok(Answer {
                endpoint: url.path().to_string(),
                status: status.as_u16(),
                body,
                retry_after,
                load,
                served,
            });
        }
    }

    /// [`Self::get_body`], sent from the browser tab. Paid for first; the
    /// page follows no redirect on an API call, so one comes back as status 0
    /// and is refused; a document navigation's hops are charged and held to
    /// the origin in [`Self::answer_from_page`].
    async fn get_body_in_page(
        &self,
        page: &dyn super::page::Page,
        path: &str,
        query: &[(&str, &str)],
        referer: &str,
        style: Surface<'_>,
    ) -> Result<Answer, IgError> {
        let mut url = self.base.join(path)?;
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        tracing::debug!(%url, "GET from the page");

        // Before paying: a push-back the browser already heard stops this
        // with its cause, and is not written down again.
        if let Some(cause) = page.heard() {
            return Err(cause.into());
        }
        self.pacer.clear_to_send().await?;
        let request = self.page_request(&url, referer, style);
        let response = tokio::select! {
            biased;
            () = self.pacer.cancel_token().canceled() => return Err(IgError::Canceled),
            sent = page.send(request) => sent.map_err(IgError::from)?,
        };
        // An opaque redirect the page did not follow: an `Abort` everywhere.
        if response.status == 0 && !response.redirected {
            return Err(IgError::Unexpected {
                status: 0,
                body: "Instagram answered with a redirect, which is not followed".into(),
            });
        }
        if let Some(operation) = named(url.path(), &style) {
            self.timed(
                operation,
                response.status,
                response.waited(),
                response.load(),
                Via::Browser,
            )
            .await;
        }
        self.answer_from_page(&url, response).await
    }

    /// What the page said, read the way an answer off the wire is read.
    ///
    /// A redirect here is a document navigation's. Its hops went out before
    /// they could be paid for, so they are paid for now, and **what Instagram
    /// answered is kept even when paying is refused**: a 429 in hand is never
    /// left unclassified. The same holds for a body over the ceiling.
    pub(super) async fn answer_from_page(
        &self,
        asked: &Url,
        response: super::page::PageResponse,
    ) -> Result<Answer, IgError> {
        let mut landing = None;
        if response.redirected {
            let landed = Url::parse(&response.url).map_err(|_| IgError::OffOrigin {
                to: crate::error::body_excerpt(&response.url),
            })?;
            if !same_origin(asked, &landed) {
                return Err(IgError::OffOrigin {
                    to: crate::error::body_excerpt(landed.as_str()),
                });
            }
            landing = crate::error::landed_on(
                &landed[url::Position::BeforePath..url::Position::AfterQuery],
            );
        }
        if let Some(fresh) = response.header("x-ig-set-www-claim") {
            *self.claim.lock().unwrap_or_else(|e| e.into_inner()) = fresh.to_string();
        }
        // A change of address with no redirect counted still took a request.
        let hops = if response.redirected {
            response.hops.max(1)
        } else {
            0
        };
        if let Some(refused) = landing {
            let _ = self.pay_for_hops(hops).await;
            return Err(self.record(refused));
        }
        let too_large = response.too_large;
        let retry_after = response.header("retry-after").map(str::to_string);
        let load = response.load();
        let served = response.served();
        let answer = Answer {
            endpoint: asked.path().to_string(),
            status: response.status,
            retry_after,
            load,
            served,
            body: if too_large {
                String::new()
            } else {
                response.body
            },
        };
        let paid = self.pay_for_hops(hops).await.and_then(|()| {
            if hops as usize > MAX_HOPS {
                Err(IgError::TooManyRedirects)
            } else {
                Ok(())
            }
        });
        if let Err(refused) = paid {
            if !answer.is_success() {
                // Recorded for what it says; the caller still hears the refusal.
                let _ = self.refuse(&answer);
            }
            return Err(refused);
        }
        if too_large {
            if !answer.is_success() {
                return Err(self.refuse(&answer));
            }
            return Err(IgError::TooLarge {
                limit: MAX_BODY_BYTES as usize,
            });
        }
        Ok(answer)
    }

    /// Pays for hops a navigation already followed, stopping at the first
    /// charge refused.
    async fn pay_for_hops(&self, hops: u32) -> Result<(), IgError> {
        for _ in 0..hops {
            self.pacer.pay_for_sent().await?;
        }
        Ok(())
    }

    /// Sends the request, or gives up the moment the user asks.
    ///
    /// **The cancel branch must answer [`IgError::Canceled`]**: a dropped
    /// future's `reqwest::Error` would classify as `Network` and be retried.
    /// `biased`, so a token already set wins against a ready response.
    pub(super) async fn send_or_cancel(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, IgError> {
        tokio::select! {
            biased;
            () = self.pacer.cancel_token().canceled() => Err(IgError::Canceled),
            response = request.send() => Ok(response?),
        }
    }

    /// The other half of [`IgClient::send_or_cancel`]: a server can stall
    /// mid-body too.
    async fn read_or_cancel(
        &self,
        response: reqwest::Response,
        cap: u64,
    ) -> Result<String, IgError> {
        tokio::select! {
            biased;
            () = self.pacer.cancel_token().canceled() => Err(IgError::Canceled),
            body = read_capped(response, cap) => body,
        }
    }

    /// Where a redirect points, resolved against the URL that produced it and
    /// held to the origin. Both refusals are excerpted: Instagram wrote them.
    fn next_hop(&self, from: &Url, response: &reqwest::Response) -> Result<Url, IgError> {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| IgError::Unexpected {
                status: response.status().as_u16(),
                body: "a redirect arrived with nowhere to go".into(),
            })?;

        let next = from.join(location).map_err(|_| IgError::OffOrigin {
            to: crate::error::body_excerpt(location),
        })?;

        if !same_origin(&self.base, &next) {
            return Err(IgError::OffOrigin {
                to: crate::error::body_excerpt(next.as_str()),
            });
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snob_core::Pk;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::client::Direction;
    use crate::client::harness::{Scripted, client, page_said, watching};

    /// An answer from `reqwest` is timed to its headers, written down under
    /// what it read rather than its path, with the load it declared.
    #[tokio::test]
    async fn an_answer_is_timed_under_its_name_and_never_its_path() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_millis(300))
                    .insert_header("x-ig-capacity-level", "3")
                    .set_body_string("{}"),
            )
            .mount(&server)
            .await;
        let client = client(&server).await;

        client
            .get_body("/api/v1/friendships/4242/following/", &[], "", Surface::App)
            .await
            .unwrap();

        let answers = client.pacer().latest_answers();
        assert_eq!(answers.len(), 1, "{answers:?}");
        let answer = &answers[0];
        assert_eq!(answer.operation, "following");
        assert_eq!(answer.status, 200);
        assert_eq!(answer.via, Via::Direct);
        assert!(
            answer.waited >= std::time::Duration::from_millis(300),
            "{answer:?}"
        );
        assert_eq!(answer.load.as_deref(), Some("x-ig-capacity-level=3"));
    }

    /// A document is a page, not an answer of the API, and is not timed.
    #[tokio::test]
    async fn a_document_is_not_timed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .mount(&server)
            .await;
        let client = client(&server).await;

        client
            .get_body("/someone/", &[], "", Surface::Document)
            .await
            .unwrap();

        assert!(client.pacer().latest_answers().is_empty());
    }

    #[test]
    fn a_name_is_never_copied_out_of_a_path() {
        let app = Surface::App;
        let named = |path: &str| named(path, &app).unwrap();
        assert_eq!(named("/api/v1/friendships/42/followers/"), "followers");
        assert_eq!(
            named("/api/v1/friendships/42/mutual_followers/"),
            "mutual_followers"
        );
        assert_eq!(named("/api/v1/media/3141/comments/"), "comments");
        assert_eq!(named(crate::web::SHOW_MANY), "show_many");
        assert_eq!(named("/api/v1/friendships/someone/followers/"), "other");
        assert_eq!(named("/api/v1/users/web_profile_info/"), "other");
        assert_eq!(named("/someone/"), "other");
        assert_eq!(super::named("/someone/", &Surface::Document), None);
    }

    /// A 429 whose body dies mid-stream is still a 429.
    ///
    /// Read as `Network`, it would be retried three more times and no cooldown
    /// would be written: both halves of the rule at once.
    ///
    /// Served from a raw socket rather than from `wiremock`, because what has to
    /// happen is a body that stops arriving: the headers announce a length and
    /// the connection closes before it is sent.
    #[tokio::test]
    async fn a_throttled_answer_with_a_body_that_dies_is_still_throttled() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());

        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();

            // Drained first, or the close below races the request still being
            // written and the failure lands on the send rather than on the read.
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while socket.read(&mut byte).unwrap_or(0) == 1 {
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }

            // Announced as 4096 bytes, and then the connection goes away with
            // none of them sent. The pause is what lets the head be delivered
            // and the body read begin before that happens.
            let _ =
                socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nContent-Length: 4096\r\n\r\n");
            let _ = socket.flush();
            std::thread::sleep(std::time::Duration::from_millis(150));
        });

        let (client, budget) = watching(&base);
        let error = client.validate().await.unwrap_err();
        server.join().unwrap();

        assert!(
            matches!(error, IgError::RateLimited),
            "a body that would not read must not turn a 429 into a network error: {error:?}"
        );
        assert_eq!(error.reaction(), crate::error::Reaction::Cooldown);
        assert_eq!(
            budget.calls(),
            vec![(
                "rate_limit".to_string(),
                snob_core::budget::RATE_LIMIT_COOLDOWN
            )]
        );
    }

    /// `Cookie` is dropped by the HTTP client on a cross-host redirect, but
    /// `X-CSRFToken` is not on its list and would have traveled. Instagram's
    /// API does not redirect off its own host, so refusing costs nothing.
    #[tokio::test]
    async fn an_api_redirect_off_the_origin_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "https://evil.test/collected"),
            )
            .mount(&server)
            .await;

        let error = client(&server).await.validate().await.unwrap_err();
        // `OffOrigin` rather than `Network`: the refusal is this client's now
        // that `get_body` follows the chain itself, and it has to keep landing
        // on `Abort` — see `a_hop_off_the_origin_is_refused_and_not_retried`.
        assert!(matches!(error, IgError::OffOrigin { .. }), "{error:?}");

        // The one request that was made is the one we made on purpose.
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
    }

    /// A followed hop is a request, and every request is paid for.
    ///
    /// The budget's count is what the user is shown and what rate control is
    /// built on, so a chain of two has to count three requests, not one.
    #[tokio::test]
    async fn every_redirect_hop_is_charged() {
        let server = MockServer::start().await;
        let body = r#"{"users":[],"next_max_id":null}"#;

        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/1/followers/"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/api/v1/hop-one/"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hop-one/"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", "/api/v1/hop-two/"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/hop-two/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let client = client(&server).await;
        let page = client
            .friendships_page(Pk::new(1), "someone", Direction::Followers, 50, None)
            .await
            .expect("the chain ends in an answer");
        assert!(page.users.is_empty());

        assert_eq!(
            client.pacer().spent(),
            3,
            "one request and two hops is three requests, and the budget has to know"
        );
    }

    /// A hop off instagram.com is refused, and refused in a way that stops the
    /// walk rather than making it try again.
    ///
    /// Two of the headers on these requests are credentials. reqwest drops
    /// `Cookie` across hosts but has never heard of `X-CSRFToken`, so a
    /// followed hop would carry it wherever the response pointed.
    ///
    /// The reaction matters as much as the refusal: read as `Network`, whose
    /// reaction is `Retry`, the pager would send the same impossible request
    /// three more times.
    #[tokio::test]
    async fn a_hop_off_the_origin_is_refused_and_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/1/followers/"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "https://example.invalid/collect"),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        let error = client
            .friendships_page(Pk::new(1), "someone", Direction::Followers, 50, None)
            .await
            .expect_err("it must not follow that");

        assert!(
            matches!(&error, IgError::OffOrigin { to } if to.contains("example.invalid")),
            "{error:?}"
        );
        assert_eq!(
            error.reaction(),
            crate::error::Reaction::Abort,
            "retrying a redirect that will be refused again is what cost 13 requests"
        );
        assert_eq!(
            client.pacer().spent(),
            1,
            "the hop was never sent, so it is never charged"
        );
    }

    /// A chain that never ends stops at `MAX_HOPS`, having paid for exactly the
    /// requests it made.
    #[tokio::test]
    async fn a_redirect_loop_stops_and_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/1/followers/"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "/api/v1/friendships/1/followers/"),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        let error = client
            .friendships_page(Pk::new(1), "someone", Direction::Followers, 50, None)
            .await
            .expect_err("a loop is not an answer");

        assert!(matches!(error, IgError::TooManyRedirects), "{error:?}");
        assert_eq!(error.reaction(), crate::error::Reaction::Abort);
        assert_eq!(
            client.pacer().spent(),
            (MAX_HOPS + 1) as u32,
            "the first request and every hop it was allowed"
        );
    }

    /// The query goes on the first request and not on the hops.
    ///
    /// A `Location` carries whatever query it means to carry. Appending ours to
    /// it would send a parameter the server did not ask to see twice, and on an
    /// endpoint that takes a cursor that is a different request from the one
    /// the redirect described.
    #[tokio::test]
    async fn the_query_is_not_reattached_to_a_hop() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/1/followers/"))
            .and(query_param("count", "50"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "/api/v1/landed/?count=7"),
            )
            .mount(&server)
            .await;
        // Mounted with the hop's own count, so it only matches if ours was not
        // added alongside it.
        Mock::given(method("GET"))
            .and(path("/api/v1/landed/"))
            .and(query_param("count", "7"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"users":[],"next_max_id":null}"#),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        client
            .friendships_page(Pk::new(1), "someone", Direction::Followers, 50, None)
            .await
            .expect("the hop's own query is the one that travels");
    }

    /// A redirect with no `Location` is an answer nobody can act on, and it must
    /// not become a silent success or a retry.
    #[tokio::test]
    async fn a_redirect_with_nowhere_to_go_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/1/followers/"))
            .respond_with(ResponseTemplate::new(302))
            .mount(&server)
            .await;

        let client = client(&server).await;
        let error = client
            .friendships_page(Pk::new(1), "someone", Direction::Followers, 50, None)
            .await
            .expect_err("there is nowhere to go");
        assert!(
            matches!(error, IgError::Unexpected { status: 302, .. }),
            "{error:?}"
        );
    }

    /// A stop during a request in flight does not wait for the server.
    ///
    /// The interrupt that does not land in a budget wait lands here, and
    /// without the race it would track however long the far end holds the
    /// connection (see `send_or_cancel`).
    ///
    /// The delay here is thirty seconds so that a passing run cannot be one
    /// that simply waited it out: the assertion is that the call came back in a
    /// fraction of it.
    #[tokio::test]
    async fn canceling_does_not_wait_for_the_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(30))
                    .set_body_string(r#"{"users":[],"next_max_id":null}"#),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        let token = client.pacer().cancel_token().clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            token.cancel();
        });

        let started = std::time::Instant::now();
        let error = client
            .friendships_page(Pk::new(1), "someone", Direction::Followers, 50, None)
            .await
            .expect_err("the run was canceled");

        // `Canceled`, not `Network`. Letting the dropped request become a
        // network error would give it `Reaction::Retry`, so the pager would
        // answer a Ctrl+C by sending the request three more times.
        assert!(matches!(error, IgError::Canceled), "{error:?}");
        assert_eq!(error.reaction(), crate::error::Reaction::Abort);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "it waited {:?}, which is the server's patience rather than the user's",
            started.elapsed()
        );
    }

    /// A stop is the user's answer, not Instagram's, and nothing is written
    /// down as though it were.
    ///
    /// The endpoint here would classify as a push-back and earn a cooldown if
    /// its answer were ever read. Cancellation has to win first, and win
    /// without the interrupted request leaving a mark: a cooldown recorded
    /// because somebody pressed Ctrl+C would refuse the next run for half an
    /// hour over something Instagram never said.
    #[tokio::test]
    async fn canceling_records_no_cooldown() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_delay(std::time::Duration::from_secs(30))
                    .set_body_string(r#"{"message":"feedback_required"}"#),
            )
            .mount(&server)
            .await;

        let (client, budget) = watching(&server.uri());

        let token = client.pacer().cancel_token().clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            token.cancel();
        });

        let error = client.validate().await.expect_err("the run was canceled");
        assert!(matches!(error, IgError::Canceled), "{error:?}");
        assert!(
            budget.calls().is_empty(),
            "a canceled request was recorded as a push-back: {:?}",
            budget.calls()
        );
    }

    /// A page that has heard a push-back, or that answers with one it heard,
    /// and counts what it is asked to send.
    struct Pushed {
        heard: Option<crate::client::page::PushedBack>,
        sent: std::sync::atomic::AtomicUsize,
    }

    impl crate::client::page::Page for Pushed {
        fn send(&self, _: crate::client::page::PageRequest) -> crate::client::page::PageFuture<'_> {
            self.sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Err(crate::client::page::PageError::PushedBack(
                    crate::client::page::PushedBack::RateLimited,
                ))
            })
        }
        fn heard(&self) -> Option<crate::client::page::PushedBack> {
            self.heard.clone()
        }
    }

    /// A push-back the browser heard on the app's own calls stops the next
    /// request before it is paid for, with its own cause — a challenge keeps
    /// its link — and is not written down a second time.
    #[tokio::test]
    async fn a_push_back_the_page_heard_stops_before_anything_is_paid() {
        let (client, budget) = crate::client::harness::watching("http://127.0.0.1:9/");
        let page = Arc::new(Pushed {
            heard: Some(crate::client::page::PushedBack::Challenge {
                url: Some("https://www.instagram.com/challenge/x/".into()),
            }),
            sent: std::sync::atomic::AtomicUsize::new(0),
        });
        let client = client.through(page.clone());

        let error = client
            .get_body("/api/v1/friendships/42/following/", &[], "", Surface::App)
            .await
            .err()
            .expect("it was refused");
        assert!(
            matches!(error, IgError::Challenge { url: Some(_) }),
            "{error:?}"
        );
        assert_eq!(page.sent.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(client.pacer().spent(), 0, "nothing was paid for");
        assert!(
            budget.calls().is_empty(),
            "recorded again: {:?}",
            budget.calls()
        );
    }

    /// One the page reports as the answer to a request is its cause too, and
    /// it was recorded where it was heard: never here as well, which would
    /// double the cooldown.
    #[tokio::test]
    async fn a_push_back_the_page_reports_is_never_recorded_again() {
        let (client, budget) = crate::client::harness::watching("http://127.0.0.1:9/");
        let page = Arc::new(Pushed {
            heard: None,
            sent: std::sync::atomic::AtomicUsize::new(0),
        });
        let client = client.through(page.clone());

        let error = client
            .get_body("/api/v1/friendships/42/following/", &[], "", Surface::App)
            .await
            .err()
            .expect("it was refused");
        assert!(matches!(error, IgError::RateLimited), "{error:?}");
        assert_eq!(page.sent.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            budget.calls().is_empty(),
            "recorded again: {:?}",
            budget.calls()
        );
    }

    /// Every push-back says what `Retry-After` it carried, and nothing acts on
    /// it.
    ///
    /// `classify` takes a status and a body and never sees a header, so the
    /// question of whether these endpoints send this at all has never been
    /// answerable from a real run. It is now. What must **not** happen is the
    /// header changing anything before somebody has seen one: the cooldown
    /// recorded here is the same one that was recorded before, and a
    /// server-named thirty seconds must never shorten it.
    #[tokio::test]
    async fn a_push_back_says_what_retry_after_it_carried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "30")
                    // What Instagram volunteers about its own load on every
                    // API answer. Carried for the same reason and decided on
                    // for neither: see `note_push_back`.
                    .insert_header("x-ig-capacity-level", "2")
                    .insert_header("x-ig-peak-time", "1")
                    .insert_header("x-stack", "distillery")
                    .set_body_raw(
                        r#"{"message":"Please wait a few minutes"}"#,
                        "application/json",
                    ),
            )
            .mount(&server)
            .await;

        let (client, budget) = watching(&server.uri());

        // **The header on the answer, not the line in the log.** A capturing
        // subscriber is thread-local and an async path is polled wherever the
        // runtime likes, so a log assertion is flaky. What matters is that the
        // header is read off the response and carried, which `Answer` holds;
        // the logging is one line over this value.
        let answer = client
            .get_body(
                &format!(
                    "/api/v1/friendships/{}/following/",
                    client.session.ds_user_id
                ),
                &[("count", "1")],
                "",
                Surface::App,
            )
            .await
            .expect("a 429 is an answer, not a transport failure");
        assert_eq!(answer.status, 429);
        assert_eq!(answer.retry_after.as_deref(), Some("30"));
        assert_eq!(
            answer.load.as_deref(),
            Some("x-ig-capacity-level=2 x-ig-peak-time=1"),
            "nobody has ever recorded the load Instagram announced while refusing"
        );
        assert_eq!(
            answer.served.as_deref(),
            Some("x-stack=distillery content-type=application/json")
        );

        // And the cooldown is untouched by either of them. Thirty seconds is
        // far shorter than the rate-limit cooldown, so a header allowed to
        // shorten anything would show up right here.
        let error = client.validate().await.unwrap_err();
        assert!(matches!(error, IgError::RateLimited), "{error:?}");
        let recorded = budget.calls();
        assert_eq!(recorded.len(), 1, "{recorded:?}");
        assert_eq!(recorded[0].0, "rate_limit");
        assert_eq!(
            recorded[0].1,
            snob_core::budget::RATE_LIMIT_COOLDOWN,
            "the server's number reached the cooldown, and it must not"
        );
    }

    /// A push-back with no such header still says so, which is the answer the
    /// logging is really after: these endpoints may simply never send one.
    ///
    /// A 200 carrying `spam: true` is a push-back, and one that a check on the
    /// status alone would have walked straight past.
    #[tokio::test]
    async fn a_push_back_without_the_header_is_recorded_as_absent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"status":"fail","spam":true}"#),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        let answer = client
            .get_body("/api/v1/friendships/42/following/", &[], "", Surface::App)
            .await
            .expect("a 200 is an answer");
        assert_eq!(answer.status, 200);
        assert!(
            answer.retry_after.is_none(),
            "nothing sent one, so nothing may be invented"
        );

        let error = client.validate().await.unwrap_err();
        assert!(matches!(error, IgError::RateLimited), "{error:?}");
    }

    /// A hop to the challenge is the challenge, from `reqwest` as from the
    /// page: classified, written down, and not followed, so the one request
    /// charged is the one that was sent.
    #[tokio::test]
    async fn a_redirect_to_the_challenge_is_a_challenge_and_is_not_followed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "/challenge/x/?next=%2F"),
            )
            .mount(&server)
            .await;

        let (client, budget) = watching(&server.uri());
        let error = client.validate().await.unwrap_err();
        assert!(matches!(error, IgError::Challenge { .. }), "{error:?}");
        assert_eq!(
            budget.calls(),
            vec![(
                "challenge".to_string(),
                snob_core::budget::CHALLENGE_COOLDOWN
            )]
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_eq!(client.pacer().spent(), 1);
    }

    fn from_the_page(status: u16, too_large: bool) -> super::super::page::PageResponse {
        super::super::page::PageResponse {
            status,
            url: "http://127.0.0.1:9/api/v1/x/".into(),
            too_large,
            ..Default::default()
        }
    }

    /// A body over the ceiling does not take the status away from the page
    /// path either: a 429 too large to read is still a 429, with its cooldown,
    /// and only a success too large to read is `TooLarge`.
    #[tokio::test]
    async fn a_refusal_too_large_to_read_from_the_page_is_still_a_refusal() {
        let (client, budget) = watching("http://127.0.0.1:9");
        let asked = Url::parse("http://127.0.0.1:9/api/v1/x/").unwrap();

        let refused = client
            .answer_from_page(&asked, from_the_page(429, true))
            .await;
        assert!(matches!(refused, Err(IgError::RateLimited)));
        assert_eq!(budget.calls().len(), 1);

        let too_large = client
            .answer_from_page(&asked, from_the_page(200, true))
            .await;
        assert!(matches!(too_large, Err(IgError::TooLarge { .. })));
        assert_eq!(budget.calls().len(), 1, "a success records nothing");
    }

    /// A navigation that lands on a profile whose name starts with
    /// "challenge" landed on a profile.
    #[tokio::test]
    async fn a_profile_named_like_the_challenge_is_a_profile() {
        let (client, budget) = watching("http://127.0.0.1:9");
        let asked = Url::parse("http://127.0.0.1:9/challenger/").unwrap();
        let mut response = from_the_page(200, false);
        response.url = "http://127.0.0.1:9/challenger/?hl=en".into();
        response.redirected = true;

        let answer = client.answer_from_page(&asked, response).await;
        assert!(matches!(answer, Ok(Answer { status: 200, .. })));
        assert!(budget.calls().is_empty());
    }

    const PAGE_BASE: &str = "http://127.0.0.1:9";

    /// A redirect the page followed to `url`, `hops` of them, answering
    /// `status`.
    fn landed(url: &str, hops: u32, status: u16) -> super::super::page::PageResponse {
        super::super::page::PageResponse {
            status,
            body: "{}".into(),
            url: url.into(),
            redirected: true,
            hops,
            ..Default::default()
        }
    }

    /// One GET from `page`, the way every endpoint sends one: a friendship
    /// list, the one kind of GET the page sends.
    async fn get_from(client: &IgClient) -> Result<Answer, IgError> {
        client
            .get_body(
                "/api/v1/friendships/2345678901/following/",
                &[],
                "",
                Surface::App,
            )
            .await
    }

    /// An API call the page would not follow a redirect on comes back as an
    /// opaque redirect, and is refused: one request sent, one paid for,
    /// nothing recorded, nothing retried.
    #[tokio::test]
    async fn an_opaque_redirect_from_the_page_is_refused_with_nothing_more_sent() {
        let (client, budget) = watching(PAGE_BASE);
        let page = Scripted::new(|_| Ok(page_said(0, "")));
        let client = client.through(page.clone());

        let Err(error) = get_from(&client).await else {
            panic!("an opaque redirect is not an answer");
        };
        assert!(
            matches!(error, IgError::Unexpected { status: 0, .. }),
            "{error:?}"
        );
        assert_eq!(error.reaction(), crate::error::Reaction::Abort);
        assert_eq!(page.asked().len(), 1);
        assert_eq!(client.pacer().spent(), 1);
        assert!(budget.calls().is_empty(), "{:?}", budget.calls());
    }

    /// A navigation that ended on another origin is refused before its hop is
    /// paid for, since nothing is read from it.
    #[tokio::test]
    async fn a_page_answer_off_the_origin_is_refused() {
        let (client, _) = watching(PAGE_BASE);
        let page = Scripted::new(|_| Ok(landed("https://example.invalid/collect", 1, 200)));
        let client = client.through(page);

        let Err(error) = get_from(&client).await else {
            panic!("another origin is not an answer");
        };
        assert!(
            matches!(&error, IgError::OffOrigin { to } if to.contains("example.invalid")),
            "{error:?}"
        );
        assert_eq!(error.reaction(), crate::error::Reaction::Abort);
    }

    /// A navigation that landed on the challenge is the challenge: written
    /// down, and its hop paid for beside the request.
    #[tokio::test]
    async fn a_page_that_landed_on_the_challenge_is_a_challenge() {
        let (client, budget) = watching(PAGE_BASE);
        let page = Scripted::new(|_| Ok(landed("http://127.0.0.1:9/challenge/abc/", 1, 200)));
        let client = client.through(page);

        let Err(error) = get_from(&client).await else {
            panic!("the challenge is not an answer");
        };
        assert!(matches!(error, IgError::Challenge { .. }), "{error:?}");
        assert_eq!(
            budget.calls(),
            vec![(
                "challenge".to_string(),
                snob_core::budget::CHALLENGE_COOLDOWN
            )]
        );
        assert_eq!(client.pacer().spent(), 2, "the request and its hop");
    }

    /// Every hop the browser followed is paid for, and a chain longer than
    /// the `reqwest` loop would follow is refused the same way, once paid.
    #[tokio::test]
    async fn every_hop_the_page_followed_is_paid_for() {
        let (client, _) = watching(PAGE_BASE);
        let page = Scripted::new(|_| Ok(landed("http://127.0.0.1:9/api/v1/y/", 2, 200)));
        let client = client.through(page);
        assert!(get_from(&client).await.is_ok());
        assert_eq!(client.pacer().spent(), 3, "the request and its two hops");

        let (client, _) = watching(PAGE_BASE);
        let too_many = MAX_HOPS as u32 + 1;
        let page =
            Scripted::new(move |_| Ok(landed("http://127.0.0.1:9/api/v1/y/", too_many, 200)));
        let client = client.through(page);
        let Err(error) = get_from(&client).await else {
            panic!("a chain that long is a loop");
        };
        assert!(matches!(error, IgError::TooManyRedirects), "{error:?}");
        assert_eq!(error.reaction(), crate::error::Reaction::Abort);
        assert_eq!(client.pacer().spent(), 1 + too_many);
    }

    /// Paying for a hop can be refused — here by a Ctrl+C — and a 429 the
    /// page already holds is recorded all the same.
    #[tokio::test]
    async fn a_refusal_the_page_holds_is_recorded_when_its_hop_cannot_be_paid() {
        let (client, budget) = watching(PAGE_BASE);
        client.pacer().cancel_token().cancel();
        let asked = Url::parse("http://127.0.0.1:9/api/v1/x/").unwrap();

        let answer = client
            .answer_from_page(&asked, landed("http://127.0.0.1:9/api/v1/y/", 1, 429))
            .await;
        assert!(matches!(answer, Err(IgError::Canceled)));
        assert_eq!(
            budget.calls(),
            vec![(
                "rate_limit".to_string(),
                snob_core::budget::RATE_LIMIT_COOLDOWN
            )]
        );
    }

    /// The claim a page's answer hands out goes back on the next request.
    #[tokio::test]
    async fn the_claim_a_page_answer_sets_is_sent_next() {
        let (client, _) = watching(PAGE_BASE);
        let page = Scripted::new(|_| {
            let mut answer = page_said(200, "{}");
            answer.headers = vec![("x-ig-set-www-claim".into(), "hmac.fresh".into())];
            Ok(answer)
        });
        let client = client.through(page.clone());
        assert!(get_from(&client).await.is_ok());
        assert!(get_from(&client).await.is_ok());

        let claim = |request: &super::super::page::PageRequest| {
            request
                .headers
                .iter()
                .find(|(name, _)| name == "X-IG-WWW-Claim")
                .map(|(_, value)| value.clone())
        };
        let asked = page.asked();
        assert_eq!(claim(&asked[0]).as_deref(), Some("0"));
        assert_eq!(claim(&asked[1]).as_deref(), Some("hmac.fresh"));
    }
}
