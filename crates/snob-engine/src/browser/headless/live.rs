//! One account's browser while it runs: its tab, the document the tab is on,
//! and the requests built beside that document's values, as the app's own
//! are.

use std::sync::Arc;

use snob_core::Pk;
use snob_core::session::Session;
use snob_ig::allowlist::Rediscovered;
use snob_ig::client::page::{PageError, PageRequest};
use snob_ig::web::{Ask, Call, Seen, SentToken, Unbuilt};
use snob_store::paths::AccountPaths;

use super::profile::ProfileMark;
use super::{APP_CALLS_PATIENCE, APP_CALLS_POLL, guard, listen, tab, write_back};
use crate::browser::cdp::Cdp;
use crate::system::sync::lock;

/// The browser of one account, started and connected.
pub(super) struct Live {
    pub(super) cdp: Cdp,
    /// The profile it runs on: the account's own.
    pub(super) profile: std::path::PathBuf,
    /// The DevTools session of the tab the requests are sent from.
    pub(super) tab: String,
    /// The origin the tab is on. `about:blank` until the first request.
    pub(super) origin: String,
    /// The isolated world the requests are sent from, with the loader of the
    /// document it was made in; `None` until one is made.
    pub(super) world: Option<(String, i64)>,
    /// The values the document that world was made in carries, read with it.
    pub(super) page: snob_ig::page_values::PageValues,
    /// Whether its cookies have been checked
    /// ([`super::profile::sync_cookies`]): from then on the mark names the
    /// session the browser holds ([`Live::held`]).
    pub(super) checked: bool,
    /// What is known about the profile; see [`ProfileMark`].
    pub(super) mark: ProfileMark,
    /// What the listener to the app's traffic shares with the requests sent
    /// from the same tab; see [`listen`].
    pub(super) listened: Arc<listen::Shared>,
    /// The write of snob's the tab's guard lets out, armed as it is sent
    /// (`answer::built_and_sent`); see [`guard::Armed`].
    pub(super) armed: Arc<guard::Armed>,
    /// Where a push-back heard on it is recorded: the account's own.
    pub(super) account: AccountPaths,
    /// The loader of the last document whose token the owner log named
    /// ([`Live::build`]), so that each document's is named once.
    pub(super) token_named: Option<String>,
    /// The document the last [`Ask::Document`] loaded, by its loader, and
    /// the path it was asked for: the only one a write may be built on
    /// ([`Live::build`]).
    pub(super) asked_document: Option<(String, String)>,
    /// What it was last moved to (`power::qos`), and when; `None` until it
    /// has been. See [`super::Headless::keep_mode`].
    pub(super) mode: Option<(crate::system::power::qos::Mode, std::time::Instant)>,
}

impl Live {
    /// The session the browser holds, by its fingerprint, once its cookies have
    /// been checked: the one it was handed, a later login it kept instead
    /// ([`super::profile::sync_cookies`]), or what a write-back stored from it.
    pub(super) fn held(&self) -> Option<&str> {
        self.checked.then_some(())?;
        self.mark.session.as_deref()
    }

    /// What the browser holds for the site it was sending to: read before it
    /// closes, for [`write_back`].
    pub(super) async fn cookies(&self) -> Option<snob_ig::login::BrowserCookies> {
        self.held()?;
        let host = lock(&self.listened.site).clone()?;
        let cookies = self
            .cdp
            .cookies()
            .await
            .map_err(|e| tracing::debug!(error = %e, "could not read the browser's cookies"))
            .ok()?;
        crate::browser::cdp::collect_for(&cookies, &host)
    }

    /// Whether the browser holds exactly `session`'s `sessionid`.
    pub(super) async fn holds(&self, session: &Session) -> bool {
        self.cookies()
            .await
            .is_some_and(|jar| jar.sessionid.expose() == session.sessionid.expose())
    }

    /// The same, with the session it was handed, for this process's own
    /// write-back.
    pub(super) async fn jar(&self, pk: Pk) -> Option<write_back::Rotated> {
        let handed = self.held()?.to_string();
        let jar = self.cookies().await?;
        Some(write_back::Rotated::new(pk, handed, jar))
    }

    /// The values the tab's current document carries, with the web session id
    /// its app last sent from that document: what a request built like the
    /// app's is built from.
    ///
    /// **The document is checked first**, as for a request sent: the app can
    /// load another one by itself, and values read from the old document would
    /// go out beside the new one's calls.
    pub(super) async fn page_values(
        &mut self,
    ) -> Result<snob_ig::page_values::PageValues, PageError> {
        tab::isolated_world(self).await?;
        let mut values = self.page.clone();
        values.web_session = self.world.as_ref().and_then(|(loader, _)| {
            lock(&self.listened.documents)
                .get(loader)
                .and_then(|document| document.web_session.clone())
        });
        Ok(values)
    }

    /// The loader of the document the tab is on, as the world was last made.
    pub(super) fn loader(&self) -> String {
        self.world
            .as_ref()
            .map(|(loader, _)| loader.clone())
            .unwrap_or_default()
    }

    /// Whether the document `loader` names is the one the last
    /// [`Ask::Document`] loaded at `path`.
    fn made_from(&self, loader: &str, path: &str) -> bool {
        self.asked_document
            .as_ref()
            .is_some_and(|(asked, at)| !loader.is_empty() && asked == loader && at == path)
    }

    /// The request `call` asks for, built as `session` from the tab's current
    /// document: its values, the app's calls on it, and what the app last sent
    /// with the operation asked, from any document
    /// ([`snob_ig::web::build_call`]): its variables, whose flags are copied,
    /// and the number it sent the read under.
    ///
    /// **A value only the app's calls carry is waited for**, up to
    /// [`APP_CALLS_PATIENCE`]: the app sends them as its document boots, a
    /// moment after the load.
    ///
    /// **A write is built only on the document it is made from**: the one the
    /// last [`Ask::Document`] loaded at `call`'s referrer, still the tab's.
    ///
    /// **A read is made from the page the tab is on**: the queries the app
    /// sends about an account it is not showing (a profile's, a hover card's,
    /// the story viewer's) go out as it sends them, from the page it is on, on
    /// that page's route, so the `Referer` and `__crn` agree with where the tab
    /// really is rather than with a page it never went to
    /// ([`Live::made_from_where_the_tab_is`]).
    ///
    /// **What the live check asks is said in the owner log**, at debug: once
    /// per document, which of its two tokens the app's Relay and Comet calls
    /// carry; and for a query built, whether it took the app's variables or its
    /// own, and which keys its variables and the app's for the same operation
    /// on this document each lack.
    pub(super) async fn build(
        &mut self,
        session: &Session,
        call: &Call,
    ) -> Result<PageRequest, PageError> {
        let deadline = tokio::time::Instant::now() + APP_CALLS_PATIENCE;
        loop {
            let page = self.page_values().await?;
            let loader = self.loader();
            if matches!(call.ask, Ask::Write { .. }) && !self.made_from(&loader, &call.referrer) {
                tracing::debug!("a write asked on a document it was not made from");
                return Err(PageError::NotReady(
                    "the profile this write is made from".into(),
                ));
            }
            let call = self.made_from_where_the_tab_is(call).await;
            // A std lock, released before anything is awaited.
            let (built, token, apps) = {
                let mut documents = lock(&self.listened.documents);
                let asked = match &call.ask {
                    Ask::Query { operation, .. } => Some(*operation),
                    _ => None,
                };
                let variables = asked
                    .and_then(|operation| documents.template(operation))
                    .map(str::to_string);
                let doc_id = asked
                    .and_then(|operation| documents.rediscovered().get(operation))
                    .map(str::to_string);
                let rest_token = documents.rest_token().map(str::to_string);
                let document = documents.entry(&loader);
                let token = document.calls.sent_token(&page);
                let built = snob_ig::web::build_call(
                    &call,
                    &page,
                    &mut document.calls,
                    session.ds_user_id,
                    &Seen {
                        variables: variables.as_deref(),
                        doc_id: doc_id.as_deref(),
                        rest_token: rest_token.as_deref(),
                    },
                );
                let apps = match &call.ask {
                    Ask::Query { operation, .. } => document
                        .calls
                        .variables_sent(*operation)
                        .map(str::to_string),
                    _ => None,
                };
                (built, token, apps)
            };
            if token != SentToken::NotYet && self.token_named.as_deref() != Some(&loader) {
                tracing::debug!(
                    ?token,
                    "the token the app's Relay and Comet calls carry on this document"
                );
                self.token_named = Some(loader);
            }
            match built {
                Err(Unbuilt::Missing(missing))
                    if missing.waits_on_the_app() && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(APP_CALLS_POLL).await;
                }
                Ok(request) => {
                    say_the_variables(&call, &request, apps.as_deref());
                    return Ok(request);
                }
                built => return built.map_err(PageError::from),
            }
        }
    }

    /// The numbers the app was seen to send the registry's reads under:
    /// what a request built from them is held to the allowlist with.
    pub(super) fn rediscovered(&self) -> Rediscovered {
        lock(&self.listened.documents).rediscovered().clone()
    }

    /// `call`, made from the page the tab is on ([`snob_ig::web::made_from`]),
    /// which is read from the tab itself: a page that could not be read
    /// leaves the call as it was asked.
    async fn made_from_where_the_tab_is(&mut self, call: &Call) -> Call {
        let Some(path) = tab::path_of(self).await else {
            return call.clone();
        };
        let made = snob_ig::web::made_from(call, &path);
        if made.referrer != call.referrer {
            tracing::debug!(
                "a read is made from the page the tab is on, not the one it was asked from"
            );
        }
        made
    }
}

/// Says in the owner log, for a query built as `request`, whether it took
/// the app's variables, with only what is asked made snob's, or its own; and,
/// when the app sent the same operation on the document (`apps`), which keys
/// each side's variables lack. Names only.
fn say_the_variables(call: &Call, request: &PageRequest, apps: Option<&str>) {
    let Ask::Query {
        operation,
        variables: asked,
    } = &call.ask
    else {
        return;
    };
    let Some(ours) = request.body.as_deref().and_then(|body| {
        url::form_urlencoded::parse(body.as_bytes())
            .find(|(name, _)| name == "variables")
            .map(|(_, value)| value.into_owned())
    }) else {
        return;
    };
    let operation = operation.friendly_name();
    let the_apps = ours != *asked;
    tracing::debug!(
        operation,
        the_apps,
        "a query's variables: the app's flags, or snob's own"
    );
    if let Some((ours_lack, apps_lack)) =
        apps.and_then(|apps| snob_ig::web::keys_each_lacks(&ours, apps))
    {
        tracing::debug!(
            operation,
            ?ours_lack,
            ?apps_lack,
            "the keys snob's variables and the app's each lack"
        );
    }
}
