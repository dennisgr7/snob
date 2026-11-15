//! Obtaining the Instagram session: a pasted cookie, or the cookies a browser
//! snob launched hands over. Never a programmatic login: the password is
//! never seen. (`sessionid` is `HttpOnly`, so no console snippet can read it.)

use snob_core::secret::Secret;
use snob_core::session::{Session, SessionError, SessionOrigin};
use snob_core::{EpochMs, Pk};
use thiserror::Error;

use crate::client::IgClient;
use crate::error::IgError;
use crate::pace::Pacer;
use crate::page_values::Face;

#[derive(Debug, Error)]
pub enum LoginError {
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Instagram(#[from] IgError),
    #[error(
        "the browser handed over cookies for two different accounts: the sessionid belongs to {from_sessionid} and ds_user_id says {from_cookie}"
    )]
    MismatchedAccount {
        from_sessionid: Pk,
        from_cookie: String,
    },
}

impl LoginError {
    /// The Instagram error behind this one: `#[error(transparent)]` hides it
    /// from a walk of the cause chain.
    pub fn as_instagram(&self) -> Option<&IgError> {
        match self {
            Self::Instagram(e) => Some(e),
            Self::Session(_) | Self::MismatchedAccount { .. } => None,
        }
    }
}

/// The cookies a browser hands over for `instagram.com`. Only `sessionid` is
/// required; the rest travel with every request the browser makes.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserCookies {
    pub sessionid: Secret,
    pub ds_user_id: Option<String>,
    pub csrftoken: Option<Secret>,
    pub mid: Option<String>,
    pub ig_did: Option<String>,
    pub datr: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    Confirmed,
    /// A transient problem kept it from being confirmed; stored anyway.
    Unconfirmed,
    /// Nothing was asked: the account is in cooldown, and an unchecked
    /// session beats spending the request the cooldown exists to prevent.
    Skipped {
        until_ms: EpochMs,
    },
}

pub fn session_from_paste(sessionid: &str, user_agent: &str) -> Result<Session, LoginError> {
    Ok(Session::from_sessionid(
        sessionid,
        user_agent,
        SessionOrigin::Paste,
    )?)
}

/// Builds a session from the cookies a browser handed over. The account id
/// comes from the `sessionid`, as with a paste; `ds_user_id` only
/// cross-checks it, since a browser signed into several accounts can disagree.
pub fn session_from_cookies(
    cookies: &BrowserCookies,
    user_agent: &str,
) -> Result<Session, LoginError> {
    let mut session = Session::from_sessionid(
        cookies.sessionid.expose(),
        user_agent,
        SessionOrigin::Browser,
    )?;

    if let Some(from_cookie) = &cookies.ds_user_id
        && from_cookie.trim() != session.ds_user_id.to_string()
    {
        return Err(LoginError::MismatchedAccount {
            from_sessionid: session.ds_user_id,
            // Printed in the refusal, and written by whatever set the cookie.
            from_cookie: snob_core::model::printable(from_cookie.trim()),
        });
    }

    session.csrftoken = cookies.csrftoken.clone();
    session.mid = cookies.mid.clone();
    session.ig_did = cookies.ig_did.clone();
    session.datr = cookies.datr.clone();
    Ok(session)
}

/// Checks against Instagram that the session works and fills in the username
/// if it can. On throttling or a network failure the session is taken as good
/// but unconfirmed; on an expired session, a mismatched User-Agent or a
/// pending check, the error propagates and nothing is stored.
pub async fn validate(
    session: &mut Session,
    pacer: Pacer,
) -> Result<ValidationOutcome, LoginError> {
    validate_seeing(session, pacer)
        .await
        .map(|(outcome, _)| outcome)
}

/// [`validate`], and what its answers showed of the account, when one did.
/// **It asks for nothing [`validate`] does not**: the tab's document is read
/// for free, and without a browser the face comes only with the request that
/// resolves a name not known yet.
pub async fn validate_seeing(
    session: &mut Session,
    pacer: Pacer,
) -> Result<(ValidationOutcome, Option<Face>), LoginError> {
    if let Some(until_ms) = pacer.cooldown()? {
        return Ok((ValidationOutcome::Skipped { until_ms }, None));
    }

    let client = IgClient::new(session.clone(), pacer)?;

    match client.validate().await {
        Ok(()) => {
            session.mark_validated();
        }
        // A cooldown another process wrote since the check above: the same
        // fact, and it must not cost the person the session just produced.
        Err(crate::error::IgError::InCooldown { until_ms }) => {
            return Ok((ValidationOutcome::Skipped { until_ms }, None));
        }
        Err(e) if e.invalidates_session() => return Err(e.into()),
        Err(e) if e.is_login_tolerable() => {
            tracing::warn!(error = %e, "could not confirm the session");
            return Ok((ValidationOutcome::Unconfirmed, None));
        }
        Err(e) => return Err(e.into()),
    }

    // The username is cosmetic. A push-back on it is not: a cooldown was just
    // written, and the person has to be told.
    let mut face = None;
    if session.username.is_none() || client.has_page() {
        match client.own_face().await {
            Ok(Some((name, seen))) => {
                if session.username.is_none() {
                    session.username = Some(name);
                }
                face = Some(seen);
            }
            Ok(None) => {}
            Err(e) if e.is_push_back() => {
                tracing::warn!(error = %e, "the session works, but Instagram pushed back on the follow-up request; the account is in cooldown");
            }
            Err(e) => tracing::debug!(error = %e, "could not resolve the username"),
        }
    }

    Ok((ValidationOutcome::Confirmed, face))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

    #[test]
    fn a_pasted_session_remembers_where_it_came_from() {
        let s = session_from_paste("42%3AAbCd%3A20", UA).unwrap();
        assert_eq!(s.origin, SessionOrigin::Paste);
        assert_eq!(s.ds_user_id, Pk::new(42));
        assert!(s.validated_at.is_none());
    }

    fn cookies() -> BrowserCookies {
        BrowserCookies {
            sessionid: "42%3AAbCd%3A20".into(),
            ds_user_id: Some("42".into()),
            csrftoken: Some("tok".into()),
            mid: Some("mid".into()),
            ig_did: Some("did".into()),
            datr: Some("dt".into()),
        }
    }

    #[test]
    fn a_browser_session_carries_every_cookie_it_was_given() {
        let s = session_from_cookies(&cookies(), UA).unwrap();
        assert_eq!(s.origin, SessionOrigin::Browser);
        assert_eq!(s.ds_user_id, Pk::new(42));
        assert_eq!(s.csrftoken.as_ref().map(Secret::expose), Some("tok"));
        assert_eq!(
            s.cookie_header().as_str(),
            "sessionid=42%3AAbCd%3A20; ds_user_id=42; csrftoken=tok; mid=mid; ig_did=did; datr=dt"
        );
    }

    /// A browser signed into two accounts can hand over a mismatched pair.
    /// Storing it would mean acting as somebody else without saying so.
    #[test]
    fn cookies_describing_two_accounts_are_refused() {
        let mismatched = BrowserCookies {
            ds_user_id: Some("99".into()),
            ..cookies()
        };
        assert!(matches!(
            session_from_cookies(&mismatched, UA),
            Err(LoginError::MismatchedAccount { .. })
        ));
    }

    #[test]
    fn a_mismatched_cookie_is_said_without_what_a_terminal_would_act_on() {
        let mismatched = BrowserCookies {
            ds_user_id: Some("99\u{1b}[2K\u{1b}[A\u{202e}".into()),
            ..cookies()
        };
        let refusal = session_from_cookies(&mismatched, UA).unwrap_err();
        let LoginError::MismatchedAccount { from_cookie, .. } = &refusal else {
            panic!("{refusal:?}");
        };
        assert_eq!(from_cookie, "99[2K[A");
        assert!(!refusal.to_string().contains('\u{1b}'), "{refusal}");
    }

    /// The cookie is a cross-check, not a source. Without it the sessionid
    /// still says whose session this is.
    #[test]
    fn a_missing_ds_user_id_is_not_a_problem() {
        let bare = BrowserCookies {
            sessionid: "42%3AAbCd%3A20".into(),
            ..BrowserCookies::default()
        };
        assert_eq!(
            session_from_cookies(&bare, UA).unwrap().ds_user_id,
            Pk::new(42)
        );
    }

    #[test]
    fn an_unreadable_sessionid_is_rejected_before_touching_the_network() {
        assert!(matches!(
            session_from_paste("not-a-sessionid", UA),
            Err(LoginError::Session(_))
        ));
    }
}
