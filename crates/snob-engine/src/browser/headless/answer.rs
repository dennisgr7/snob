//! What the tab answers a call with ([`answer_on`]): what its document says
//! itself, or the answer to the request the call asks for, built beside the
//! document's values ([`Live::build`]) and sent from the isolated world.
//!
//! **Every answer of snob's own is read for push-back on its way back**
//! ([`own_answer`]), and nothing the allowlist refuses is sent ([`refuse`]).

use serde_json::Value;
use snob_core::Pk;
use snob_core::session::Session;
use snob_ig::allowlist::{Operation, Rediscovered};
use snob_ig::client::page::{Method, PageError, PageRequest, PageResponse};
use snob_ig::model::web::{RouteAnswer, document_pk, route_answer};
use snob_ig::web::{Ask, Call, Told};

use super::live::Live;
use super::tab::{self, fetch, navigate_and_read};
use super::{APP_CALLS_PATIENCE, APP_CALLS_POLL, APP_CALLS_QUIET, listen};
use crate::system::sync::lock;

/// `request`, unless the allowlist refuses it, a read under a number the app
/// was seen to send it under (`found`) as well as under the registry's:
/// then nothing is sent, and it is said as the defect of snob's it is.
pub(super) fn refuse(request: &PageRequest, found: &Rediscovered) -> Result<(), PageError> {
    match snob_ig::allowlist::refused_with(request, found) {
        Some(what) => {
            tracing::error!(request = %what, "refused a call snob may not send");
            Err(PageError::NotAllowed(what))
        }
        None => Ok(()),
    }
}

/// The tab's answer to `call`, asked as `session`: what its document says, or
/// the answer to the request it asks for, built beside the document's values
/// ([`Live::build`]) and sent from the isolated world.
pub(super) async fn answer_on(
    live: &mut Live,
    session: &Session,
    call: &Call,
) -> Result<Told, PageError> {
    match &call.ask {
        Ask::Viewer => signed_in(live, session).await.map(Told::Viewer),
        Ask::Tray => {
            signed_in(live, session).await?;
            tab::tray(live).await.map(Told::Tray)
        }
        Ask::Pk { name } => {
            signed_in(live, session).await?;
            let route = format!("/{name}/");
            let envelope = {
                let loader = live.loader();
                lock(&live.listened.documents)
                    .get(&loader)
                    .is_some_and(|document| document.calls.made_a_route_call())
            };
            if !envelope {
                tracing::debug!(
                    "a name's pk is read from its profile document: the app made no route call to copy"
                );
                return pk_of_the_document(live, call, &route).await;
            }
            tracing::debug!("a name's pk is asked of the route definitions");
            let answer = built_and_sent(live, session, call).await?;
            let pk = route_answer(&answer.body, &route);
            Ok(Told::Pk { answer, pk })
        }
        Ask::Document { path } => {
            live.asked_document = None;
            let mut answer = loaded(live, call, path).await?;
            let bundles = snob_ig::graphql::bundles_in(&answer.body);
            answer.body.clear();
            if (200..300).contains(&answer.status) {
                signed_in(live, session).await?;
                if path != "/" {
                    let sent = the_apps_query(live, Operation::ProfilePage).await;
                    let settled = the_apps_calls_paused(live).await;
                    tracing::debug!(
                        sent = sent.is_some(),
                        settled,
                        "the app's profile query on the profile document, and its calls paused"
                    );
                }
                live.asked_document = Some((live.loader(), path.clone()));
            }
            Ok(Told::Document { answer, bundles })
        }
        Ask::Asset { url } => {
            if let Some(what) = snob_ig::allowlist::refused_asset(&call.origin, url) {
                tracing::error!(request = %what, "refused a download snob may not make");
                return Err(PageError::NotAllowed(what));
            }
            asset(live, call, url).await.map(Told::Asset)
        }
        Ask::Query { .. }
        | Ask::Rest { .. }
        | Ask::Navigation { .. }
        | Ask::Statuses { .. }
        | Ask::Write { .. } => built_and_sent(live, session, call).await.map(Told::Answer),
    }
}

/// The file at `url`, fetched from the isolated world as the app's own page
/// would fetch one from the CDN: the browser's TLS and HTTP/2, `Sec-Fetch-*`,
/// the page's `Referer` and none of the site's cookies, which are not the
/// CDN's.
async fn asset(live: &mut Live, call: &Call, url: &str) -> Result<PageResponse, PageError> {
    if let Some(cause) = live.listened.latch.get() {
        return Err(PageError::PushedBack(cause));
    }
    let world = tab::isolated_world(live).await?;
    match tab::fetch_asset(live, world, url, call.cap, call.timeout_ms).await {
        Ok(response) => Ok(response),
        Err(e) => Err(live.listened.latch.get().map_or(e, PageError::PushedBack)),
    }
}

/// Who the tab's document was served to, when that is `session`'s account
/// ([`snob_ig::web::viewer_of`], as a built call checks it).
async fn signed_in(
    live: &mut Live,
    session: &Session,
) -> Result<snob_ig::page_values::Viewer, PageError> {
    let page = live.page_values().await?;
    Ok(snob_ig::web::viewer_of(&page, session.ds_user_id)?.clone())
}

/// The request `call` asks for, built on the tab's document, held to the
/// allowlist, and sent from the isolated world.
async fn built_and_sent(
    live: &mut Live,
    session: &Session,
    call: &Call,
) -> Result<PageResponse, PageError> {
    let request = live.build(session, call).await?;
    refuse(&request, &live.rediscovered())?;
    let world = tab::isolated_world(live).await?;
    // The guard refuses every follow and unfollow but the one it expects,
    // until the fetch has answered or is given up.
    let expecting = matches!(call.ask, Ask::Write { .. }).then(|| live.armed.expect(&request));
    let response = fetch(live, world, &request).await;
    drop(expecting);
    own_answer(live, &request, response?).await
}

/// The tab goes to `path` on the call's origin, a navigation the allowlist
/// lets out, and the document it lands on is the answer.
async fn loaded(live: &mut Live, call: &Call, path: &str) -> Result<PageResponse, PageError> {
    let request = PageRequest {
        method: Method::Get,
        url: format!("{}{path}", call.origin),
        headers: Vec::new(),
        referrer: format!("{}{}", call.origin, call.referrer),
        body: None,
        navigate: true,
        cap: call.cap,
        timeout_ms: call.timeout_ms,
    };
    refuse(&request, &Rediscovered::default())?;
    let response = navigate_and_read(live, &request.url).await?;
    own_answer(live, &request, response).await
}

/// The pk behind `route`, read from the profile document the tab loads for it:
/// the id of the route props the document embeds.
///
/// **The app's own profile query is a second witness.**
async fn pk_of_the_document(live: &mut Live, call: &Call, route: &str) -> Result<Told, PageError> {
    let mut answer = loaded(live, call, route).await?;
    let from_the_document = document_pk(&answer.body);
    answer.body.clear();
    if !(200..300).contains(&answer.status) {
        tracing::debug!(
            status = answer.status,
            "the profile document is not a success"
        );
        return Ok(Told::Pk {
            answer,
            pk: RouteAnswer::Error,
        });
    }
    let from_the_app = the_apps_query(live, Operation::ProfilePage)
        .await
        .and_then(|sent| id_in(&sent));
    let witnesses = match (from_the_document, from_the_app) {
        (Some(document), Some(app)) if document == app => "both, and they agree",
        (Some(_), Some(_)) => "both, and they name two accounts",
        (Some(_), None) => "the document's props alone",
        (None, Some(_)) => "the app's query alone",
        (None, None) => "neither",
    };
    tracing::debug!(witnesses, "the pk the profile document named");
    let pk = match (from_the_document, from_the_app) {
        (Some(document), Some(app)) if document != app => RouteAnswer::Error,
        (Some(pk), _) | (None, Some(pk)) => RouteAnswer::Pk(pk),
        (None, None) => RouteAnswer::Error,
    };
    Ok(Told::Pk { answer, pk })
}

/// The variables the app sent with its own `operation` on the tab's
/// document, waited for up to [`APP_CALLS_PATIENCE`]; `None` when it sent
/// none by then.
async fn the_apps_query(live: &Live, operation: Operation) -> Option<String> {
    let loader = live.loader();
    let deadline = tokio::time::Instant::now() + APP_CALLS_PATIENCE;
    loop {
        let sent = lock(&live.listened.documents)
            .get(&loader)
            .and_then(|document| document.calls.variables_sent(operation))
            .map(str::to_string);
        if sent.is_some() || tokio::time::Instant::now() >= deadline {
            return sent;
        }
        tokio::time::sleep(APP_CALLS_POLL).await;
    }
}

/// Waits until the app has sent no call from the tab's document for
/// [`APP_CALLS_QUIET`], up to [`APP_CALLS_PATIENCE`]; says whether it paused by
/// then.
async fn the_apps_calls_paused(live: &Live) -> bool {
    let loader = live.loader();
    let heard = || {
        lock(&live.listened.documents)
            .get(&loader)
            .map_or(0, |document| document.heard)
    };
    let deadline = tokio::time::Instant::now() + APP_CALLS_PATIENCE;
    let mut last = heard();
    let mut quiet_since = tokio::time::Instant::now();
    loop {
        if quiet_since.elapsed() >= APP_CALLS_QUIET {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(APP_CALLS_POLL).await;
        let now = heard();
        if now != last {
            last = now;
            quiet_since = tokio::time::Instant::now();
        }
    }
}

/// The `id` of a query's variables, as the app sends it: a string of digits.
fn id_in(variables: &str) -> Option<Pk> {
    let variables: Value = serde_json::from_str(variables).ok()?;
    match variables.get("id")? {
        Value::String(id) => id.parse().ok(),
        Value::Number(id) => id.as_u64().map(Pk::new),
        _ => None,
    }
}

/// One of snob's own answers, on its way back to the client.
pub(super) async fn own_answer(
    live: &Live,
    request: &PageRequest,
    response: PageResponse,
) -> Result<PageResponse, PageError> {
    let latch = &live.listened.latch;
    if latch.get().is_none()
        && let Some(error) = listen::pushed_back(&request.url, &response)
    {
        listen::noted_own(&request.url, &response);
        let recording = listen::heard(
            live.cdp.connection(),
            &live.tab,
            &live.listened,
            &live.account,
            &error,
        );
        if let Some(recording) = recording {
            let _ = recording.await;
        }
    }
    match latch.get() {
        Some(cause) => Err(PageError::PushedBack(cause)),
        None => Ok(response),
    }
}
