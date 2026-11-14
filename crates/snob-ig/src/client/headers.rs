//! What a request to Instagram says about itself.
//!
//! One list per transport: [`IgClient::dressed`] for what `reqwest` sends,
//! whatever its method, and [`IgClient::page_request`] for a read the browser
//! sends, which names only what the app's own script adds. The `reqwest` list
//! is the divergent one (`SNOB_NO_BROWSER`): it reaches endpoints the web app
//! never calls. Every value is the page's or computed from the User-Agent; see
//! "Talking to Instagram" in `AGENTS.md`.

use url::Url;

use crate::client_hints;
use crate::{BASE_URL, IG_APP_ID};

use super::IgClient;
use super::page::Method;

/// Which of the two things a browser does on instagram.com a request is.
///
/// **The difference changes which handler answers**: the API routes answer
/// 403 without `X-IG-App-ID`, and a route sent with the wrong set answers 404.
/// A navigation asks for HTML and announces none of the app headers, because
/// the page is what loads the app. One superset sent everywhere is a shape no
/// browser produces.
#[derive(Debug, Clone, Copy)]
pub(super) enum Surface<'a> {
    App,
    Document,
    /// The app talking to Relay. Both values already travel in the body of
    /// the same request.
    Relay {
        friendly_name: &'a str,
        lsd: &'a str,
    },
}

impl Surface<'_> {
    fn accept(self) -> &'static str {
        match self {
            // What `fetch()` sends when the page sets none.
            Self::App | Self::Relay { .. } => "*/*",
            // Byte for byte what Chrome 151 sent on all six navigations of the
            // August 2026 capture. `concat!` because `cargo fmt` would put a
            // continued literal's indentation inside the value.
            Self::Document => concat!(
                "text/html,application/xhtml+xml,application/xml;q=0.9,",
                "image/avif,image/webp,image/apng,*/*;q=0.8,",
                "application/signed-exchange;v=b3;q=0.7"
            ),
        }
    }

    fn fetch_mode(self) -> (&'static str, &'static str) {
        match self {
            Self::App | Self::Relay { .. } => ("cors", "empty"),
            Self::Document => ("navigate", "document"),
        }
    }

    /// False for a navigation: the page is what loads the app.
    fn announces_the_app(self) -> bool {
        !matches!(self, Self::Document)
    }

    /// A navigation is `u=0`; everything else is a fetch at the default urgency.
    fn priority(self) -> &'static str {
        match self {
            Self::Document => client_hints::NAVIGATION_PRIORITY,
            _ => client_hints::FETCH_PRIORITY,
        }
    }
}

/// A web session id in the app's shape, three segments of six letters and
/// digits, for the client that sends without a browser.
///
/// **Made up, which nothing else in this client is**: without a page there is
/// no app to copy it from. Made once for a client, as a page keeps its own.
pub(super) fn new_web_session() -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let segment = || -> String {
        (0..6)
            .map(|_| char::from(DIGITS[fastrand::usize(..DIGITS.len())]))
            .collect()
    };
    format!("{}:{}:{}", segment(), segment(), segment())
}

impl IgClient {
    /// The current `X-IG-WWW-Claim`: the server hands one back in
    /// `x-ig-set-www-claim` and expects it echoed; `0` is what a client sends
    /// once, before it has been given one.
    fn claim(&self) -> String {
        self.claim.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub(super) fn remember_claim(&self, response: &reqwest::Response) {
        let Some(fresh) = response
            .headers()
            .get("x-ig-set-www-claim")
            .and_then(|v| v.to_str().ok())
        else {
            return;
        };
        *self.claim.lock().unwrap_or_else(|e| e.into_inner()) = fresh.to_string();
    }

    /// Split from the sending so that a redirect hop is built as the first
    /// request was.
    pub(super) fn api_request(
        &self,
        url: &Url,
        query: Option<&[(&str, &str)]>,
        referer: &str,
        style: Surface<'_>,
    ) -> reqwest::RequestBuilder {
        let request = self.api.get(url.clone());
        let request = match query {
            Some(q) => request.query(q),
            None => request,
        };
        self.dressed(request, referer, style)
    }

    /// The GET [`Self::dressed`] would build, for the browser to send. Only
    /// what the site's own script adds goes in the list; the browser adds the
    /// rest itself. `X-CSRFToken` goes in empty and the page writes in the
    /// cookie it holds at that moment, since it rotates. A
    /// [`Surface::Document`] comes out as a navigation, which the process
    /// holding the browser refuses.
    pub(super) fn page_request(
        &self,
        url: &Url,
        referer: &str,
        style: Surface<'_>,
    ) -> super::page::PageRequest {
        debug_assert!(
            !matches!(style, Surface::Relay { .. }),
            "a Relay call from the page is asked of the tab"
        );
        let mut headers: Vec<(String, String)> = Vec::new();
        if style.announces_the_app() {
            headers.push(("X-IG-App-ID".into(), IG_APP_ID.into()));
            headers.push(("X-ASBD-ID".into(), client_hints::ASBD_ID.into()));
            headers.push(("Accept".into(), style.accept().into()));
            headers.push(("X-CSRFToken".into(), String::new()));
        }
        if matches!(style, Surface::App) {
            headers.push(("X-IG-WWW-Claim".into(), self.claim()));
            headers.push(("X-Requested-With".into(), "XMLHttpRequest".into()));
        }
        super::page::PageRequest {
            method: Method::Get,
            url: url.to_string(),
            headers,
            referrer: format!("{}{referer}", self.base),
            body: None,
            navigate: matches!(style, Surface::Document),
            cap: super::transport::MAX_BODY_BYTES,
            timeout_ms: super::transport::REQUEST_TIMEOUT.as_millis() as u64,
        }
    }

    /// The headers every request `reqwest` sends to Instagram carries, the
    /// read's GET and the write's POST alike.
    pub(super) fn dressed(
        &self,
        request: reqwest::RequestBuilder,
        referer: &str,
        style: Surface<'_>,
    ) -> reqwest::RequestBuilder {
        let mut request = request;
        if style.announces_the_app() {
            request = request
                // Without these the API routes answer 403 even with a good session.
                .header("X-IG-App-ID", IG_APP_ID)
                .header("X-ASBD-ID", client_hints::ASBD_ID)
                // On every API call of the capture.
                .header("X-IG-Max-Touch-Points", "0");
        }
        // Only on `/api/v1/`, measured in the August 2026 browser capture:
        //     x-ig-www-claim     0/171 on /api/graphql, 97/97 on /api/v1/*
        //     x-requested-with   0/171 on /api/graphql, 97/97 on /api/v1/*
        if matches!(style, Surface::App) {
            request = request
                .header("X-IG-WWW-Claim", self.claim())
                .header("X-Requested-With", "XMLHttpRequest")
                .header("X-Web-Session-ID", self.web_session.as_str());
        }
        // Every `/api/graphql` call of the August 2026 capture carried both.
        if let Surface::Relay { friendly_name, lsd } = style {
            request = request
                .header("X-FB-Friendly-Name", friendly_name)
                .header("X-FB-LSD", lsd);
        }
        let mut request = request
            // Whether Instagram varies on `Sec-Fetch-*` is unverified (the
            // capture saw `Vary` on none); a browser sends them regardless.
            .header("Accept", style.accept())
            // From the User-Agent: `zstd` and `Priority` arrived in the same
            // Chromium release and have to agree.
            .header("Accept-Encoding", self.hints.accept_encoding)
            .header("Accept-Language", client_hints::accept_language())
            // The capture confirmed `Vary: Accept-Language, Cookie`.
            .header("Sec-Fetch-Site", "same-origin")
            .header("Sec-Fetch-Mode", style.fetch_mode().0)
            .header("Sec-Fetch-Dest", style.fetch_mode().1)
            // Built rather than interpolated: a header value that will not
            // build surfaces at `send()` as `Network`, which is retried. The
            // site's own address is a truthful fallback.
            .header(
                "Referer",
                reqwest::header::HeaderValue::try_from(format!("{BASE_URL}/{referer}"))
                    .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static(BASE_URL)),
            )
            .header("Cookie", self.session.cookie_header().as_str());

        // No `Origin` here: the Fetch standard omits it on same-origin GETs.
        // It requires one on a POST, so `post` adds it, and only `post`.

        // Only where an app would send one: a navigation carries no
        // `X-CSRFToken` (`a_navigation_does_not_claim_to_be_the_app`).
        if let Some(csrf) = &self.session.csrftoken
            && style.announces_the_app()
        {
            request = request.header("X-CSRFToken", csrf.expose());
        }

        // Only for browsers that send client hints at all.
        if let Some(brands) = &self.hints.ua_brands {
            request = request
                .header("Sec-CH-UA", brands)
                .header("Sec-CH-UA-Mobile", self.hints.mobile)
                .header("Sec-CH-UA-Platform", self.hints.platform);
        }

        // Whether this Chrome sends a `Priority` is the hints' to say; how
        // urgent the request is, the surface's.
        if self.hints.priority.is_some() {
            request = request.header("Priority", style.priority());
        }

        request
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use snob_core::Pk;

    use super::*;
    use crate::client::harness::{
        FOLLOWED, Known, UA, client, instagram_that_takes_a_write, writer,
    };

    #[tokio::test]
    async fn it_sends_the_headers_instagram_requires() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/42/following/"))
            .and(query_param("count", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"users":[]}"#))
            .mount(&server)
            .await;

        client(&server).await.validate().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let headers = &requests[0].headers;
        let read = |name: &str| {
            headers
                .get(name)
                .unwrap_or_else(|| panic!("missing header {name}"))
                .to_str()
                .unwrap()
                .to_string()
        };

        assert_eq!(read("x-ig-app-id"), IG_APP_ID);
        assert_eq!(read("user-agent"), UA);
        assert_eq!(
            read("cookie"),
            "sessionid=42%3AAbCdEfGh%3A20; ds_user_id=42"
        );
        assert_eq!(read("x-requested-with"), "XMLHttpRequest");
        // What the app's script sets on every API call, with the page's own
        // web session id on `/api/v1/`: one per client, in the app's shape.
        assert_eq!(read("x-ig-max-touch-points"), "0");
        let session = read("x-web-session-id");
        assert_eq!(
            crate::page_values::web_session_id(&session),
            Some(session.as_str())
        );

        // A browser sends these, so a request without them is the anomaly.
        assert_eq!(read("sec-fetch-site"), "same-origin");
        assert_eq!(read("sec-fetch-mode"), "cors");
        assert_eq!(read("sec-fetch-dest"), "empty");

        // What `fetch()` sends when the page sets nothing. `application/json`
        // is a value no browser produces here.
        assert_eq!(read("accept"), "*/*");

        // Chrome's own string, not the one the HTTP client assembles from
        // whichever decoders happen to be compiled in.
        assert_eq!(read("accept-encoding"), "gzip, deflate, br, zstd");

        // Present, and shaped like a language preference rather than like a
        // locale. What it says depends on the machine, so that is all this can
        // assert.
        let language = read("accept-language");
        assert!(!language.is_empty());
        assert!(!language.contains('_'), "{language}");

        // Chrome 138 is past the version that started sending this.
        assert_eq!(read("priority"), client_hints::FETCH_PRIORITY);

        // The client hints have to agree with the User-Agent above, which says
        // Chrome 138 on Windows.
        assert!(read("sec-ch-ua").contains(r#""Google Chrome";v="138""#));
        assert_eq!(read("sec-ch-ua-platform"), "\"Windows\"");
        assert_eq!(read("sec-ch-ua-mobile"), "?0");

        // A browser omits `Origin` on a same-origin GET. Sending one next to
        // `Sec-Fetch-Site: same-origin` is a pairing Chrome cannot produce.
        assert!(
            headers.get("origin").is_none(),
            "Origin must not travel on a same-origin GET"
        );
    }

    /// The generated web session id is the app's shape, and it is made once
    /// per client rather than per request.
    #[test]
    fn a_made_up_web_session_id_has_the_apps_shape() {
        for _ in 0..50 {
            let id = new_web_session();
            assert_eq!(
                crate::page_values::web_session_id(&id),
                Some(id.as_str()),
                "{id}"
            );
            assert_eq!(id.len(), 20);
            assert!(!id.bytes().any(|b| b.is_ascii_uppercase()), "{id}");
        }
    }

    /// The token is announced as `0` once and is the server's answer after
    /// that. Staying on `0` forever would claim, on every request, to be
    /// making the first request of the session.
    #[tokio::test]
    async fn the_www_claim_is_echoed_back_after_the_first_answer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"users":[]}"#)
                    .insert_header("x-ig-set-www-claim", "hmac.AR2nhXYZ"),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        client.validate().await.unwrap();
        client.validate().await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let claim = |i: usize| {
            requests[i]
                .headers
                .get("x-ig-www-claim")
                .unwrap()
                .to_str()
                .unwrap()
        };
        assert_eq!(claim(0), "0", "the first one has nothing to echo yet");
        assert_eq!(claim(1), "hmac.AR2nhXYZ");
    }

    /// The net under the encoding above, reached the only way it can be: by
    /// handing `get` a referer no caller builds any more.
    ///
    /// It matters because of what the alternative costs. A header that will not
    /// build is not refused where it is written — reqwest carries it to
    /// `send()`, `?` turns it into `Network`, and `Network`'s reaction is
    /// `Retry`. Four requests charged to the budget, none of them sent, and the
    /// answer says the network is at fault.
    #[tokio::test]
    async fn a_referer_that_will_not_build_falls_back_to_the_site() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let _: serde_json::Value = client(&server)
            .await
            .get("/api/v1/users/web_profile_info/", &[], "gh\u{1b}[2K/")
            .await
            .expect("a referer is not worth failing a request over");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get("referer")
                .unwrap()
                .to_str()
                .unwrap(),
            BASE_URL,
            "less specific, and still true"
        );
    }

    /// **A navigation is not an XHR, and the headers say so.**
    ///
    /// The page that hands out the tokens is fetched the way a browser fetches
    /// a page: asking for HTML, saying `navigate`/`document`, and announcing
    /// none of the four headers that mean "I am the single-page app" — because
    /// at that moment there is no app yet, the page is what loads it. The API
    /// requests must keep all four, so both halves are asserted together; a
    /// change that made one set serve both would pass on its own and be a shape
    /// no browser produces on either.
    #[tokio::test]
    async fn a_navigation_does_not_claim_to_be_the_app() {
        // A browser navigating to a profile sends none of these:
        // `X-CSRFToken` is what a running page adds to its own XHRs, and
        // `Priority` on a document is `u=0`, not the default fetch urgency.
        const APP_ONLY: [&str; 5] = [
            "x-ig-app-id",
            "x-asbd-id",
            "x-ig-www-claim",
            "x-requested-with",
            "x-csrftoken",
        ];

        let server = instagram_that_takes_a_write(FOLLOWED).await;
        writer(&server)
            .await
            .follow(Pk::new(7), "someone", &Known)
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        let page = requests
            .iter()
            .find(|r| r.method == wiremock::http::Method::GET)
            .expect("the page");
        for header in APP_ONLY {
            assert!(
                !page.headers.contains_key(header),
                "a navigation carried {header}"
            );
        }
        assert_eq!(page.headers.get("sec-fetch-mode").unwrap(), "navigate");
        assert_eq!(page.headers.get("sec-fetch-dest").unwrap(), "document");
        // **The whole value, not its first word**: `starts_with` would not
        // see a run of spaces in the middle of it.
        assert_eq!(
            page.headers.get("accept").unwrap(),
            concat!(
                "text/html,application/xhtml+xml,application/xml;q=0.9,",
                "image/avif,image/webp,image/apng,*/*;q=0.8,",
                "application/signed-exchange;v=b3;q=0.7"
            ),
            "a navigation asks for what Chrome asks for"
        );
        assert_eq!(
            page.headers.get("priority").unwrap(),
            client_hints::NAVIGATION_PRIORITY,
            "a document is the most urgent thing on the page, not a default fetch"
        );

        // And the mutation, which is the other half of the same distinction.
        let write = requests
            .iter()
            .find(|r| r.method == wiremock::http::Method::POST)
            .expect("the mutation");
        assert_eq!(
            write.headers.get("x-fb-friendly-name").unwrap(),
            "usePolarisFollowMutation",
            "a Relay request names its operation in the headers as well as the body"
        );
        assert!(
            write.headers.contains_key("x-fb-lsd"),
            "a Relay request carries the page token it was given"
        );
        assert!(
            write.headers.contains_key("x-csrftoken"),
            "a write still carries the token the write rule requires"
        );
        // **And does not wear the XHR's clothes.** Measured, not reasoned:
        // across 171 `/api/graphql` requests in a real session, neither of
        // these appeared once, while both appeared on all 97 `/api/v1/` ones.
        for xhr_only in ["x-requested-with", "x-ig-www-claim", "x-web-session-id"] {
            assert!(
                !write.headers.contains_key(xhr_only),
                "a Relay request carried {xhr_only}, which Relay never sends"
            );
        }
        assert!(
            write.headers.contains_key("x-ig-app-id"),
            "the app id is on every surface that has an app behind it"
        );
        assert_eq!(write.headers.get("x-ig-max-touch-points").unwrap(), "0");
        assert!(!page.headers.contains_key("x-ig-max-touch-points"));
        assert_eq!(
            write.headers.get("priority").unwrap(),
            client_hints::FETCH_PRIORITY
        );
    }
}
