//! The CDN client: fetches a picture or a video with no account behind it.
//!
//! **A [`CdnClient`] holds no session and no budget**: no cookie, no app id,
//! nothing charged to Instagram's buckets. Its redirect rule is the opposite of
//! the API's: an asset may move between CDN hosts, never to anything else.

use url::Url;

use crate::client_hints::ClientHints;
use crate::error::IgError;
use crate::pace::CancelToken;

use super::transport::{MAX_HOPS, build_cdn_client, read_capped_bytes, stream_capped};

/// Sized for a profile picture (1080x1080 lands far below); it exists so a
/// redirect to something else cannot make us read until memory runs out.
pub(super) const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;

/// The rule is [`crate::allowlist::refused_asset`]'s, which the tab holds a
/// fetch to as well.
pub(super) fn serves_pictures(base: &Url, url: &Url) -> bool {
    crate::allowlist::refused_asset(base.as_str(), url.as_str()).is_none()
}

/// [`CdnClient::check_downloadable`] without a client, for the tab's path.
pub(super) fn check_downloadable(base: &Url, url: &Url) -> Result<(), IgError> {
    if serves_pictures(base, url) {
        return Ok(());
    }
    Err(IgError::Unexpected {
        status: 0,
        body: format!(
            "the picture URL points somewhere pictures do not come from: {}",
            url.host_str().unwrap_or("nowhere")
        ),
    })
}

/// Every redirect hop is held to the same rule as the first: a redirect is
/// where a response gets to choose where the next request goes.
pub(super) fn cdn_policy(base: Url) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= MAX_HOPS {
            attempt.error("too many redirects")
        } else if serves_pictures(&base, attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("a redirect tried to leave the CDN")
        }
    })
}

/// Downloads from Instagram's CDN, carrying nothing that identifies an account.
pub struct CdnClient {
    http: reqwest::Client,
    /// The site itself is the one exception to "a CDN host", matched whole.
    base: Url,
    accept_encoding: &'static str,
    cancel: CancelToken,
}

impl CdnClient {
    /// `user_agent` is the account's browser's: it names nobody.
    pub fn new(user_agent: &str, base: Url, cancel: CancelToken) -> Result<Self, IgError> {
        Ok(Self {
            http: build_cdn_client(user_agent, cdn_policy(base.clone()))?,
            accept_encoding: ClientHints::from_user_agent(user_agent).accept_encoding,
            base,
            cancel,
        })
    }

    /// Downloads a public asset, such as a profile picture.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, IgError> {
        self.download_capped(url, MAX_ASSET_BYTES).await
    }

    /// The same, with the caller naming the ceiling (a story video does not
    /// fit under [`MAX_ASSET_BYTES`]).
    pub async fn download_capped(&self, url: &str, cap: usize) -> Result<Vec<u8>, IgError> {
        let response = self.fetch(url).await?;
        // Raced against the token: a CDN that sends headers and then stalls
        // would hold this process as long as it likes.
        tokio::select! {
            biased;
            () = self.cancel.canceled() => Err(IgError::Canceled),
            bytes = read_capped_bytes(response, cap as u64) => bytes,
        }
    }

    /// [`CdnClient::download_capped`], written to `sink` as it arrives, so the
    /// peak is one chunk whatever the size. On any error the sink holds a
    /// prefix of the file; whoever owns the file removes it.
    pub async fn download_to(
        &self,
        url: &str,
        cap: usize,
        sink: &mut (impl std::io::Write + Send),
    ) -> Result<Downloaded, IgError> {
        let mut head = Vec::new();
        let mut tee = Tee {
            head: &mut head,
            sink,
        };
        let response = self.fetch(url).await?;
        let len = tokio::select! {
            biased;
            () = self.cancel.canceled() => Err(IgError::Canceled),
            n = stream_capped(response, cap as u64, &mut tee) => n,
        }?;
        Ok(Downloaded { len, head })
    }

    /// Refuses a picture URL that does not go where a picture goes. The first
    /// hop comes out of Instagram's answer, so it is the one an attacker gets
    /// to choose (`http://127.0.0.1:9222/json`, a cloud metadata address).
    pub fn check_downloadable(&self, url: &Url) -> Result<(), IgError> {
        check_downloadable(&self.base, url)
    }

    async fn fetch(&self, url: &str) -> Result<reqwest::Response, IgError> {
        let url = Url::parse(url)?;
        self.check_downloadable(&url)?;
        tracing::debug!(%url, "GET asset");

        // Not paced: the CDN is a different host with its own limits.
        // `Accept-Encoding` is the browser's; left to reqwest it would be
        // `zstd,gzip,deflate,br`, which no browser sends.
        let request = self
            .http
            .get(url)
            .header("Accept-Encoding", self.accept_encoding);
        let response = tokio::select! {
            biased;
            () = self.cancel.canceled() => return Err(IgError::Canceled),
            response = request.send() => response?,
        };
        let status = response.status();

        // Not `classify`: the CDN's 403 means the signed link expired, not
        // that the session died.
        if !status.is_success() {
            return Err(IgError::Unexpected {
                status: status.as_u16(),
                body: "the picture could not be downloaded".into(),
            });
        }
        Ok(response)
    }
}

/// What a streamed download leaves the caller with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub len: u64,
    /// The first bytes of the file, up to [`HEAD_BYTES`], for the magic number
    /// that decides the extension.
    pub head: Vec<u8>,
}

/// Twelve is what the WebP check needs (`RIFF....WEBP`); sixteen leaves room.
pub const HEAD_BYTES: usize = 16;

struct Tee<'a, W: std::io::Write> {
    head: &'a mut Vec<u8>,
    sink: &'a mut W,
}

impl<W: std::io::Write> std::io::Write for Tee<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // The sink writes first and the head copies only what it accepted:
        // after a short write `write_all` retries with `&buf[n..]`.
        let written = self.sink.write(buf)?;
        let room = HEAD_BYTES.saturating_sub(self.head.len());
        if room > 0 {
            self.head.extend_from_slice(&buf[..written.min(room)]);
        }
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.sink.flush()
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::client::harness::UA;

    fn standalone(server: &MockServer) -> CdnClient {
        CdnClient::new(
            UA,
            Url::parse(&server.uri()).unwrap(),
            CancelToken::default(),
        )
        .unwrap()
    }

    /// The point of the type: it is built from a User-Agent and nothing else,
    /// and what it sends names no account.
    #[tokio::test]
    async fn a_cdn_client_downloads_with_no_account_at_all() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/pic.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xFF, 0xD8, 0xFF]))
            .mount(&server)
            .await;

        let bytes = standalone(&server)
            .download(&format!("{}/pic.jpg", server.uri()))
            .await
            .unwrap();
        assert_eq!(bytes, vec![0xFF, 0xD8, 0xFF]);

        let requests = server.received_requests().await.unwrap();
        let headers = &requests[0].headers;
        assert!(headers.get("cookie").is_none());
        assert!(headers.get("x-ig-app-id").is_none());
        assert_eq!(headers.get("user-agent").unwrap().to_str().unwrap(), UA);
    }

    /// A Ctrl+C already given sends nothing, and says so as a cancel rather
    /// than as a network error, which would be retried.
    #[tokio::test]
    async fn a_download_after_ctrl_c_sends_nothing() {
        let server = MockServer::start().await;
        let cancel = CancelToken::default();
        cancel.cancel();
        let cdn = CdnClient::new(UA, Url::parse(&server.uri()).unwrap(), cancel).unwrap();

        let error = cdn
            .download(&format!("{}/pic.jpg", server.uri()))
            .await
            .unwrap_err();
        assert!(matches!(error, IgError::Canceled), "{error:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// The rule holds with no client of Instagram's behind it.
    #[tokio::test]
    async fn a_cdn_client_refuses_an_address_off_the_cdn() {
        let server = MockServer::start().await;
        let cdn = standalone(&server);
        for refused in [
            "https://evil.test/p.jpg",
            "http://scontent.cdninstagram.com/p.jpg",
            "https://evilcdninstagram.com/p.jpg",
        ] {
            let error = cdn.download(refused).await.unwrap_err();
            assert!(matches!(error, IgError::Unexpected { .. }), "{refused}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
