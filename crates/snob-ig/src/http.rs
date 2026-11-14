//! HTTP plumbing for callers that are not talking to Instagram.
//!
//! The only place `reqwest` is configured, and re-exported so no other
//! manifest declares it: the TLS backend is chosen per target in this crate's
//! `Cargo.toml`, and a second declaration could turn `cookies` on for
//! [`crate::client`]. No client built here can carry a credential: there is no
//! argument to pass one in.

use std::time::Duration;

pub use reqwest;

/// Not optional: a server that accepts the connection and then says nothing
/// would hang the process, and no cancel token reaches a waiting socket.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Which roots a client checks the server's certificate against. Narrowing is
/// opt-in and only ever applies to the Instagram client: it shuts out a
/// TLS-inspecting root in the platform store, and with it a corporate proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Trust {
    /// The platform's store, enterprise roots included.
    #[default]
    Platform,
    /// Mozilla's published roots plus the PEM bundles the user named.
    Narrow { extra: Vec<Vec<u8>> },
}

/// Whether narrowing works on this build. Windows on ARM64 is on schannel,
/// which cannot do it, so the flag is refused there rather than ignored.
pub const CAN_NARROW: bool = !cfg!(all(windows, target_arch = "aarch64"));

/// What every Instagram client trusts. A process-global set once from `main`,
/// before any client exists, since `login::validate` never sees the arguments.
static TRUST: std::sync::OnceLock<Trust> = std::sync::OnceLock::new();

/// `Err` carries the value already chosen back on a second call.
pub fn use_trust(trust: Trust) -> Result<(), Trust> {
    TRUST.set(trust).map_err(|_| chosen_trust())
}

pub(crate) fn chosen_trust() -> Trust {
    TRUST.get().cloned().unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Deadline {
    /// The whole request, body included.
    Whole(Duration),
    /// The longest wait for the next bytes, with no limit on the whole: a
    /// file's length is not known in advance. Its size cap is what bounds it.
    BetweenReads(Duration),
}

/// The builder every client in this program starts from.
///
/// **`trust` is an argument, never read from [`chosen_trust`] here**: that is
/// what keeps the narrowing off the webhook client, whose receiver may sit
/// behind a private CA.
pub(crate) fn builder(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
    connect_timeout: Duration,
    deadline: Deadline,
    trust: &Trust,
) -> reqwest::Result<reqwest::ClientBuilder> {
    let built = reqwest::Client::builder()
        .user_agent(user_agent.to_string())
        .redirect(redirect)
        .connect_timeout(connect_timeout);
    let built = match deadline {
        Deadline::Whole(limit) => built.timeout(limit),
        Deadline::BetweenReads(limit) => built.read_timeout(limit),
    };
    let built = built
        // reqwest drops an idle connection at 90 s (measured: 89 s reuses it,
        // 95 s opens a new one), and the budget's waits can pass a minute.
        // Five minutes is Chromium's own idle-socket timeout. The keepalive
        // lets the kernel notice a connection a NAT dropped silently.
        .pool_idle_timeout(Duration::from_secs(300))
        .tcp_keepalive(Duration::from_secs(60));

    let Trust::Narrow { extra } = trust else {
        return Ok(built);
    };

    // Unreachable on the schannel target (`CAN_NARROW` is false and `main`
    // refuses the flag), but it has to compile there.
    #[cfg(all(windows, target_arch = "aarch64"))]
    {
        let _ = extra;
        Ok(built)
    }

    // `tls_certs_only` turns the platform store off; `tls_certs_merge` would
    // leave the enterprise root where it was.
    #[cfg(not(all(windows, target_arch = "aarch64")))]
    {
        let mut roots: Vec<reqwest::Certificate> = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .map(|der| reqwest::Certificate::from_der(der))
            .collect::<reqwest::Result<_>>()?;
        for bundle in extra {
            roots.extend(reqwest::Certificate::from_pem_bundle(bundle)?);
        }
        Ok(built.tls_certs_only(roots))
    }
}

/// A client that carries no credential. `redirect` has no default: the CDN
/// follows redirects within itself, a webhook POST follows none.
pub fn plain(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
) -> reqwest::Result<reqwest::Client> {
    builder(
        user_agent,
        redirect,
        CONNECT_TIMEOUT,
        Deadline::Whole(REQUEST_TIMEOUT),
        &Trust::Platform,
    )?
    .build()
}

/// [`plain`] for a file: no limit on the whole request, only on the wait for
/// the next bytes. The caller bounds its size. For snob's own update.
pub fn plain_download(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
) -> reqwest::Result<reqwest::Client> {
    builder(
        user_agent,
        redirect,
        CONNECT_TIMEOUT,
        Deadline::BetweenReads(REQUEST_TIMEOUT),
        &Trust::Platform,
    )?
    .build()
}

/// [`plain`] for a destination allowed to be `http://` because it is on the
/// user's own network, and which therefore must bypass any proxy: with
/// `HTTP_PROXY` set, a POST to loopback would otherwise go to the proxy in the
/// clear (reproduced against a recording proxy).
pub fn plain_direct(
    user_agent: &str,
    redirect: reqwest::redirect::Policy,
) -> reqwest::Result<reqwest::Client> {
    builder(
        user_agent,
        redirect,
        CONNECT_TIMEOUT,
        Deadline::Whole(REQUEST_TIMEOUT),
        &Trust::Platform,
    )?
    .no_proxy()
    .build()
}

/// Reads a response body, stopping at `cap`, so the far end cannot decide how
/// much memory this process uses. Lossy: it is for showing a person the bytes.
pub async fn read_capped(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<String, reqwest::Error> {
    // Said, not silently emptied: an empty excerpt would read as an empty answer.
    if let Some(declared) = response.content_length()
        && declared > cap as u64
    {
        return Ok(format!("<{declared} bytes, more than the {cap} shown>"));
    }

    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > cap {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The guard this module exists to keep. A client built here has no way to
    /// be given the session, and the header it does send names this tool rather
    /// than a browser.
    #[tokio::test]
    async fn a_plain_client_sends_no_cookie_and_its_own_user_agent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = plain("snob/test", reqwest::redirect::Policy::none()).unwrap();
        client.get(server.uri()).send().await.unwrap();

        let received = &server.received_requests().await.unwrap()[0];
        assert!(
            received.headers.get("cookie").is_none(),
            "a client built here must not be able to carry a session"
        );
        assert_eq!(received.headers.get("user-agent").unwrap(), "snob/test");
    }

    /// A body larger than the cap does not decide how much memory this uses.
    #[tokio::test]
    async fn a_body_past_the_cap_is_cut_rather_than_buffered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(10_000)))
            .mount(&server)
            .await;

        let client = plain("snob/test", reqwest::redirect::Policy::none()).unwrap();
        let response = client.get(server.uri()).send().await.unwrap();
        assert!(read_capped(response, 100).await.unwrap().len() <= 100);
    }

    /// The default is the platform's store, and it has to stay that way.
    ///
    /// Narrowing is the safer setting for somebody being inspected and the
    /// broken one for somebody behind a corporate proxy, and only the person
    /// running it knows which they are. A default that flipped would take the
    /// tool away from the second group with no message.
    #[test]
    fn the_platform_store_is_what_is_trusted_unless_asked() {
        assert!(matches!(Trust::default(), Trust::Platform));
    }

    /// A client that carries no credential is never narrowed, and there is no
    /// argument with which to narrow it.
    ///
    /// This is the rule the audit attached to the whole item: a private CA in
    /// front of the user's own webhook receiver is legitimate. It is held down
    /// structurally rather than by remembering — `plain` and `plain_direct`
    /// take a user agent and a redirect policy and nothing else — so this test
    /// exists to fail the day somebody adds the parameter.
    #[test]
    fn the_webhook_client_cannot_be_narrowed() {
        let source = include_str!("http.rs");
        for name in ["pub fn plain(", "pub fn plain_direct("] {
            let at = source.find(name).expect(name);
            let signature = &source[at..source[at..].find(')').unwrap() + at];
            assert!(
                !signature.contains("Trust"),
                "{name} has grown a way to be told what to trust: {signature}"
            );
        }
    }

    /// Narrowing replaces the roots rather than adding to them.
    ///
    /// `tls_certs_merge` compiles just as well and leaves the enterprise root
    /// exactly where it was, which is the one thing this option exists to
    /// remove. The difference is one word in one call and nothing else would
    /// notice.
    #[test]
    fn narrowing_uses_only_the_roots_it_was_given() {
        // Only the code. The tests below this line talk *about* the wrong call
        // by name, and a search over the whole file finds its own prose.
        let source = include_str!("http.rs");
        let code = &source[..source.find("#[cfg(test)]").expect("a test module")];
        assert!(
            code.contains("tls_certs_only("),
            "the roots are not replaced"
        );
        assert!(
            !code.contains("tls_certs_merge("),
            "merging leaves the platform's roots in place, which is the exposure"
        );
    }

    /// Mozilla's list is not empty, and every entry in it is a certificate the
    /// client can be handed.
    ///
    /// Cheap, and it covers the mistake that would otherwise show up as a
    /// handshake failure against Instagram with `--strict-roots` and nowhere
    /// else: `webpki-roots` publishes trust anchors rather than certificates,
    /// and swapping the crate back would compile until this ran.
    #[cfg(not(all(windows, target_arch = "aarch64")))]
    #[test]
    fn the_bundled_roots_are_certificates() {
        // `CAN_NARROW` is a constant, so asserting it here would be asserting
        // that this `cfg` is spelled the same way twice -- which it is, and
        // which the compiler already knows. What is worth checking is the list.
        let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS;
        assert!(roots.len() > 50, "only {} roots", roots.len());
        for der in roots {
            reqwest::Certificate::from_der(der).expect("a root that is not a certificate");
        }
    }

    /// And on the target where it cannot work, it says so rather than being
    /// accepted.
    #[cfg(all(windows, target_arch = "aarch64"))]
    #[test]
    fn the_schannel_build_does_not_pretend_it_can_narrow() {
        const { assert!(!CAN_NARROW) };
    }

    /// A run cannot change what it trusts halfway through.
    ///
    /// `IgClient` is built in three places and one of them is inside this
    /// crate, so the value is a process-global; the thing that makes that safe
    /// is that the second write is refused rather than applied. Without it a
    /// client built early and one built late could disagree about what they
    /// check a certificate against.
    #[test]
    fn what_is_trusted_is_settled_once() {
        // Whichever of these runs first in this process wins; both orders have
        // to leave one value in place, which is what is asserted.
        let first = use_trust(Trust::Platform);
        let second = use_trust(Trust::Narrow { extra: Vec::new() });
        assert!(
            first.is_err() || second.is_err(),
            "two different answers were both accepted"
        );
    }
}
