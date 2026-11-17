//! Which site the account's tab may be on: Instagram's own, whatever a
//! request or a call names.

use anyhow::{Context, Result};
use snob_ig::client::page::PageError;

/// The host of an origin.
pub(super) fn host_of(origin: &str) -> Option<String> {
    url::Url::parse(origin)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// Scheme, host and port, with no trailing slash.
pub(super) fn origin_of(url: &str) -> Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("not an address: {url}"))?;
    Ok(parsed.origin().ascii_serialization())
}

/// The origin of `url`, when it is the site's own (`snob_ig::client::site`).
///
/// **The account's tab is never taken to another site**, whatever handed the
/// request over, the owner's socket included: the allowlist judges a method
/// and a path, never a host, and the tab goes to the origin it is told.
pub(super) fn on_the_site(url: &str) -> Result<String, PageError> {
    let site = snob_ig::client::site().origin().ascii_serialization();
    match origin_of(url) {
        Ok(origin) if origin == site || a_tests_own_server(&origin) => Ok(origin),
        _ => {
            let what = "a request to another site";
            tracing::error!(request = what, "refused a call snob may not send");
            Err(PageError::NotAllowed(what.into()))
        }
    }
}

/// A fake Instagram on a loopback port, in a test or a `testing` build, never
/// a release: the tests of one binary cannot share the one `site()` of a
/// process, and the owner of a sandbox's browsers is not told its site (it is
/// started with the sandbox's root alone).
#[cfg(any(test, feature = "testing"))]
fn a_tests_own_server(origin: &str) -> bool {
    url::Url::parse(origin).is_ok_and(|url| {
        url.scheme() == "http"
            && matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
    })
}

#[cfg(not(any(test, feature = "testing")))]
fn a_tests_own_server(_: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_origin_has_no_path_and_no_trailing_slash() {
        assert_eq!(
            origin_of("https://www.instagram.com/api/v1/x/?a=1").unwrap(),
            "https://www.instagram.com"
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8765/api/v1/x/").unwrap(),
            "http://127.0.0.1:8765"
        );
    }
}
