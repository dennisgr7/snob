//! Intents sent to the browser tab, and what they are paid with.
//!
//! A command names what it wants with a [`web::Ask`]; the process holding the
//! browser builds the request from the page's values, and the client never
//! sees a token. **The ask decides what is paid, never the caller**: what the
//! tab answers from its document costs nothing, a write pays the write bucket
//! and is not abandoned in flight, everything else is one read.

use snob_core::Pk;

use crate::allowlist::{Operation, Rest};
use crate::error::IgError;
use crate::model::web::{
    GraphUser, HighlightsTrayPage, HoverCard, ProfilePage, ReelsPage, RouteAnswer,
};
use crate::model::{Counters, Highlight, Reel, WebProfileInfo};
use crate::web::{Ask, Call, Told};

use super::IgClient;
use super::page::PageResponse;
use super::transport::{Answer, MAX_BODY_BYTES, REQUEST_TIMEOUT};

impl IgClient {
    /// Asks the tab for `ask`, made from `referrer`, a path such as
    /// `/<user>/`. Needs a page. A request's answer comes back already read
    /// by `IgClient::answer_from_page`; what it says is the caller's to decode.
    pub(crate) async fn ask_page(&self, ask: Ask, referrer: &str) -> Result<Told, IgError> {
        let Some(page) = self.page.as_deref() else {
            return Err(IgError::Browser(
                "this client sends without a browser, which has no page to ask".into(),
            ));
        };
        // Before paying: a refused intent costs nothing, and a push-back the
        // browser heard stops this with its cause, not written down again.
        if let Some(what) = crate::allowlist::refused_ask(&ask) {
            tracing::error!(request = %what, "refused a call snob may not send");
            return Err(IgError::NotAllowed(what));
        }
        if let Some(cause) = page.heard() {
            return Err(cause.into());
        }
        let write = matches!(ask, Ask::Write { .. });
        match ask {
            Ask::Viewer | Ask::Tray => {
                if self.pacer.cancel_token().is_canceled() {
                    return Err(IgError::Canceled);
                }
                if let Some(until_ms) = self.pacer.cooldown_off_thread().await? {
                    return Err(IgError::InCooldown { until_ms });
                }
            }
            // Nothing is paid: the CDN is not Instagram's API.
            Ask::Asset { .. } => {
                if self.pacer.cancel_token().is_canceled() {
                    return Err(IgError::Canceled);
                }
            }
            Ask::Write { .. } => {
                // Before the budget is charged, as `post` refuses.
                if !self.can_write() {
                    return Err(IgError::NoCsrfToken);
                }
                self.pacer.clear_to_send_write().await?;
            }
            _ => self.pacer.clear_to_send().await?,
        }

        let asked = self.base.join(&ask.endpoint())?;
        let name = ask_name(&ask);
        // What is timed is an answer of the API.
        let timed = !write && !matches!(ask, Ask::Document { .. } | Ask::Asset { .. });
        tracing::debug!(url = %asked, ask = name, "asked of the page");
        let cap = match ask {
            Ask::Asset { .. } => super::media::PAGE_ASSET_BYTES as u64,
            _ => MAX_BODY_BYTES,
        };
        let call = Call {
            ask,
            origin: self.base.as_str().trim_end_matches('/').to_string(),
            referrer: referrer.to_string(),
            claim: self.claim.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            cap,
            timeout_ms: REQUEST_TIMEOUT.as_millis() as u64,
        };
        let told = if write {
            // A write in flight is the one request Ctrl+C does not abandon.
            page.ask(call).await.map_err(IgError::from)?
        } else {
            tokio::select! {
                biased;
                () = self.pacer.cancel_token().canceled() => return Err(IgError::Canceled),
                told = page.ask(call) => told.map_err(IgError::from)?,
            }
        };
        if timed && let Told::Answer(answer) | Told::Pk { answer, .. } = &told {
            self.timed(
                name.to_string(),
                answer.status,
                answer.waited(),
                answer.load(),
                snob_core::budget::Via::Browser,
            )
            .await;
        }

        Ok(match told {
            Told::Answer(answer) => Told::Answer(self.read_told(&asked, answer, write).await?),
            Told::Pk { answer, pk } => Told::Pk {
                answer: self.read_told(&asked, answer, write).await?,
                pk,
            },
            Told::Document { answer, bundles } => Told::Document {
                answer: self.read_told(&asked, answer, write).await?,
                bundles,
            },
            told @ (Told::Viewer(_) | Told::Tray(_) | Told::Asset(_)) => told,
        })
    }

    /// A request's answer, read as `IgClient::get` reads one from the page.
    async fn read_told(
        &self,
        asked: &url::Url,
        mut response: PageResponse,
        write: bool,
    ) -> Result<PageResponse, IgError> {
        // An opaque redirect the page did not follow: an `Abort` everywhere.
        if response.status == 0 && !response.redirected {
            let what = if write {
                "Instagram redirected a write, which is never followed"
            } else {
                "Instagram answered with a redirect, which is not followed"
            };
            return Err(IgError::Unexpected {
                status: 0,
                body: what.into(),
            });
        }
        let body = std::mem::take(&mut response.body);
        let mut read = response.clone();
        response.body = body;
        read.body = self.answer_from_page(asked, response).await?.body;
        Ok(read)
    }
}

impl IgClient {
    /// The pk behind `name`, as the tab finds it: the route definitions, or
    /// the profile document when the app has made no route call to copy. One
    /// read either way.
    pub(crate) async fn pk_of(&self, name: &str) -> Result<RouteAnswer, IgError> {
        let ask = Ask::Pk { name: name.into() };
        let endpoint = ask.endpoint();
        match self.ask_page(ask, "/").await? {
            Told::Pk { answer, pk } => {
                let answer = answered(&endpoint, answer);
                if !answer.is_success() {
                    return Err(self.refuse(&answer));
                }
                Ok(pk)
            }
            _ => Err(told_otherwise("a name")),
        }
    }

    /// The profile of `pk`: the web client's profile query, made from the
    /// page of `name` (the home page when the name is not known). The query
    /// alone; the caller that accepts the profile sends
    /// [`Self::profile_burst`]. An answer that names nobody is not found.
    pub(crate) async fn profile_by_pk(
        &self,
        pk: Pk,
        name: &str,
    ) -> Result<WebProfileInfo, IgError> {
        let operation = Operation::ProfilePage;
        let referrer = profile_path(name);
        let ask = Ask::Query {
            operation,
            variables: crate::web::profile_variables(pk),
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, &referrer).await? else {
            return Err(told_otherwise("a profile"));
        };
        let page: ProfilePage = self.decode_query(
            &answered(&endpoint, answer),
            Operation::ProfilePage.root_field(),
        )?;
        page.profile().ok_or_else(|| IgError::NotFound {
            what: (!name.is_empty()).then(|| name.to_string()),
        })
    }

    /// The reads the app sends beside its profile query: the highlights tray,
    /// the note over the picture and the school badge, each by pk. In the
    /// capture of 2026-10-01 all three went out within about a second of the
    /// query on each of the ten profiles opened.
    ///
    /// **Each is paid as the read it is**: opening a profile costs four reads.
    /// The tray is kept for [`Self::take_held_tray`]; the other two are read
    /// for a push-back only. One that fails is passed over unless it
    /// [`must_stop`].
    pub(crate) async fn profile_burst(&self, pk: Pk, name: &str) -> Result<(), IgError> {
        match self.highlights_tray_by_pk(pk, name).await {
            Ok(tray) => {
                *self.held_tray.lock().unwrap_or_else(|e| e.into_inner()) = Some(HeldTray {
                    pk,
                    action: self.pacer.actions(),
                    tray,
                });
            }
            Err(e) => passed_over(e, "the highlights tray of a profile opened")?,
        }
        let referrer = profile_path(name);
        for (operation, variables) in [
            (Operation::NoteBubble, format!(r#"{{"user_id":"{pk}"}}"#)),
            (
                Operation::SchoolPartnerBadge,
                format!(r#"{{"igid":"{pk}"}}"#),
            ),
        ] {
            let ask = Ask::Query {
                operation,
                variables,
            };
            let endpoint = ask.endpoint();
            let read = match self.ask_page(ask, &referrer).await {
                Ok(Told::Answer(answer)) => self
                    .decode::<serde_json::Value>(&answered(&endpoint, answer))
                    .map(drop),
                Ok(_) => Err(told_otherwise(operation.friendly_name())),
                Err(e) => Err(e),
            };
            if let Err(e) = read {
                passed_over(e, operation.friendly_name())?;
            }
        }
        Ok(())
    }

    /// The tray [`Self::profile_burst`] read for `pk`, taken once and only
    /// within the action it was read in (`Pacer::begin_action`).
    pub(crate) fn take_held_tray(&self, pk: Pk) -> Option<Vec<Highlight>> {
        let mut held = self.held_tray.lock().unwrap_or_else(|e| e.into_inner());
        match held.take() {
            Some(tray) if tray.pk == pk && tray.action == self.pacer.actions() => Some(tray.tray),
            _ => None,
        }
    }

    /// The app's router moving to `route` ([`Ask::Navigation`]): paid as one
    /// read, and read for a push-back only (its body is the app's `for (;;);`
    /// envelope, so only the status is read). One the tab cannot build yet,
    /// or that fails without a push-back, is passed over.
    pub(crate) async fn navigation(&self, route: &str) -> Result<(), IgError> {
        let ask = Ask::Navigation {
            route: route.to_string(),
        };
        let endpoint = ask.endpoint();
        let sent = match self.ask_page(ask, route).await {
            Ok(Told::Answer(answer)) => {
                let answer = answered(&endpoint, answer);
                if answer.is_success() {
                    Ok(())
                } else {
                    Err(self.refuse(&answer))
                }
            }
            Ok(_) => Err(told_otherwise("a navigation")),
            Err(e) => Err(e),
        };
        sent.or_else(|e| passed_over(e, "a navigation"))
    }

    /// The app's `friendships/show_many` for the accounts of a list page just
    /// read ([`Ask::Statuses`]): one read, kept for a push-back only.
    ///
    /// **Sent only with the token the app's REST POSTs carry**
    /// (`web::AppCalls::rest_token_sent`). When the tab has seen none, this
    /// client asks for no other, so one reservation is the only one lost.
    pub(crate) async fn statuses(&self, users: &[Pk], referrer: &str) -> Result<(), IgError> {
        use std::sync::atomic::Ordering;

        if users.is_empty() || self.statuses_unbuilt.load(Ordering::Relaxed) {
            return Ok(());
        }
        let pks = users
            .iter()
            .copied()
            .take(crate::allowlist::MOST_STATUSES)
            .collect();
        let ask = Ask::Statuses { pks };
        let endpoint = ask.endpoint();
        let sent = match self.ask_page(ask, referrer).await {
            Ok(Told::Answer(answer)) => self
                .decode::<serde_json::Value>(&answered(&endpoint, answer))
                .map(drop),
            Ok(_) => Err(told_otherwise("the statuses of a list page")),
            Err(IgError::PageNotReady(what)) => {
                tracing::debug!(
                    missing = %what,
                    "show_many is not sent: the app has sent no REST token to send it with"
                );
                self.statuses_unbuilt.store(true, Ordering::Relaxed);
                return Ok(());
            }
            Err(e) => Err(e),
        };
        sent.or_else(|e| passed_over(e, "show_many"))
    }
}

#[derive(Debug)]
pub(super) struct HeldTray {
    pk: Pk,
    /// `Pacer::actions` when it was read.
    action: u32,
    tray: Vec<Highlight>,
}

/// Whether an error on a call sent beside the one asked for has to end the
/// run: a push-back, a dead session, a stop, or a call snob may not send.
fn must_stop(error: &IgError) -> bool {
    error.is_push_back()
        || error.invalidates_session()
        || matches!(
            error,
            IgError::Canceled
                | IgError::WaitTooLong { .. }
                | IgError::NotAllowed(_)
                | IgError::Budget(_)
        )
}

fn passed_over(error: IgError, what: &str) -> Result<(), IgError> {
    if must_stop(&error) {
        return Err(error);
    }
    tracing::debug!(error = %error, call = what, "a call sent alongside failed; passed over");
    Ok(())
}

impl IgClient {
    /// The two counters of `pk`, from the web client's hover-card query: one
    /// read, which answers for a private account too. `None` when the answer
    /// names nobody.
    pub(crate) async fn hover_card(&self, pk: Pk) -> Result<Option<Counters>, IgError> {
        let ask = Ask::Query {
            operation: Operation::HoverCard,
            variables: format!(r#"{{"userID":"{pk}"}}"#),
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, "/").await? else {
            return Err(told_otherwise("a hover card"));
        };
        let card: HoverCard = self.decode_query(
            &answered(&endpoint, answer),
            Operation::HoverCard.root_field(),
        )?;
        Ok(card.user().map(GraphUser::counters))
    }
}

impl IgClient {
    /// The highlights of `pk` without their items: the tray query, one read.
    /// It carries no item count and no dates.
    pub(crate) async fn highlights_tray_by_pk(
        &self,
        pk: Pk,
        name: &str,
    ) -> Result<Vec<Highlight>, IgError> {
        let ask = Ask::Query {
            operation: Operation::HighlightsTray,
            variables: format!(r#"{{"user_id":"{pk}"}}"#),
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, &profile_path(name)).await? else {
            return Err(told_otherwise("a highlights tray"));
        };
        let page: HighlightsTrayPage = self.decode_query(
            &answered(&endpoint, answer),
            Operation::HighlightsTray.root_field(),
        )?;
        Ok(page.tray().tray)
    }

    /// The items of highlight `id`, as the highlights viewer reads them: every
    /// highlight of `tray` in its order. One read; only the reel `id` is kept.
    ///
    /// **Nothing is marked as seen**: the viewer does that with a mutation of
    /// its own, and the tab never opens it.
    pub(crate) async fn highlight_window(
        &self,
        id: &str,
        tray: &[String],
        name: &str,
    ) -> Result<Option<Reel>, IgError> {
        let ask = Ask::Query {
            operation: Operation::HighlightsPage,
            variables: crate::web::reel_variables(id, tray),
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, &profile_path(name)).await? else {
            return Err(told_otherwise("a highlight"));
        };
        let page: ReelsPage = self.decode_query(
            &answered(&endpoint, answer),
            Operation::HighlightsPage.root_field(),
        )?;
        Ok(page
            .highlights()
            .into_iter()
            .find(|reel| reel.id.as_deref() == Some(id)))
    }
}

impl IgClient {
    /// The reel ids of the stories tray the tab's document shows, read off the
    /// document for nothing. `None` when it holds no tray.
    pub(crate) async fn tray(&self) -> Result<Option<Vec<String>>, IgError> {
        match self.ask_page(Ask::Tray, "/").await? {
            Told::Tray(ids) => Ok(ids),
            _ => Err(told_otherwise("the stories tray")),
        }
    }

    /// The stories of `pk`, as the story viewer reads them: its gallery query
    /// with `reel_ids` the reels of the tray. One read; the reel comes whole
    /// in the first answer, so the next page is never asked.
    ///
    /// **Nothing is marked as seen**, as for [`Self::highlight_window`].
    pub(crate) async fn reel_gallery(
        &self,
        pk: Pk,
        reel_ids: &[String],
    ) -> Result<Option<Reel>, IgError> {
        let id = pk.to_string();
        let ask = Ask::Query {
            operation: Operation::ReelGallery,
            variables: crate::web::reel_variables(&id, reel_ids),
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, "/").await? else {
            return Err(told_otherwise("a reel"));
        };
        let page: ReelsPage = self.decode_query(
            &answered(&endpoint, answer),
            Operation::ReelGallery.root_field(),
        )?;
        Ok(page
            .reels()
            .into_iter()
            .find(|reel| reel.id.as_deref() == Some(id.as_str())))
    }
}

impl IgClient {
    /// A REST read of the registry's, `query` in the order the app sends it,
    /// made from `referrer`: built by the tab when there is one, and sent
    /// with `reqwest` otherwise, to the same path with the same query.
    pub(crate) async fn rest_read<T: serde::de::DeserializeOwned>(
        &self,
        read: Rest,
        query: &[(&str, &str)],
        referrer: &str,
    ) -> Result<T, IgError> {
        if !self.has_page() {
            return self.get(&read.path(), query, relative(referrer)).await;
        }
        let ask = Ask::Rest {
            read,
            query: query
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, referrer).await? else {
            return Err(told_otherwise("a list"));
        };
        self.decode(&answered(&endpoint, answer))
    }
}

/// The referrer of a request sent without a browser has no leading slash.
pub(super) fn relative(path: &str) -> &str {
    path.strip_prefix('/').unwrap_or(path)
}

/// `/<name>/`, where the web client asks about an account from; the home
/// page when the name is not known.
pub(super) fn profile_path(name: &str) -> String {
    if name.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", snob_core::model::in_a_path(name))
    }
}

pub(super) fn answered(endpoint: &str, response: PageResponse) -> Answer {
    Answer {
        endpoint: endpoint.to_string(),
        status: response.status,
        retry_after: response.header("retry-after").map(str::to_string),
        load: response.load(),
        served: response.served(),
        body: response.body,
    }
}

pub(super) fn told_otherwise(what: &str) -> IgError {
    IgError::Unexpected {
        status: 0,
        body: format!("the page was asked for {what} and answered with something else"),
    }
}

/// For a log line: never what the ask carries.
fn ask_name(ask: &Ask) -> &'static str {
    match ask {
        Ask::Viewer => "viewer",
        Ask::Tray => "tray",
        Ask::Pk { .. } => "pk",
        Ask::Document { .. } => "document",
        Ask::Query { operation, .. } => operation.friendly_name(),
        Ask::Rest { read, .. } => match read {
            crate::allowlist::Rest::Followers(_) => "followers",
            crate::allowlist::Rest::Following(_) => "following",
            crate::allowlist::Rest::MutualFollowers(_) => "mutual_followers",
            crate::allowlist::Rest::MediaInfo(_) => "media_info",
            crate::allowlist::Rest::Comments(_) => "comments",
            crate::allowlist::Rest::Likers(_) => "likers",
            crate::allowlist::Rest::Replies(..) => "replies",
        },
        Ask::Navigation { .. } => "navigation",
        Ask::Statuses { .. } => "show_many",
        Ask::Write { mutation, .. } => mutation.friendly_name(),
        Ask::Asset { .. } => "asset",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snob_core::Pk;

    use super::*;
    use crate::allowlist::{Operation, Rest, refused_ask};
    use crate::client::harness::{Scripted, page_said, spending_client};
    use crate::client::page::{AskFuture, Page, PageError, PageFuture, PageRequest};
    use crate::graphql::Mutation;
    use crate::model::web::RouteAnswer;
    use crate::pace::Pacer;
    use crate::page_values::Viewer;
    use crate::web::{Missing, Unbuilt};

    fn viewer() -> Told {
        Told::Viewer(Viewer {
            pk: Pk::new(42),
            username: "me".into(),
            fbid: "17841400000000042".into(),
        })
    }

    fn query() -> Ask {
        Ask::Query {
            operation: Operation::HoverCard,
            variables: r#"{"userID":"9001"}"#.into(),
        }
    }

    fn follow() -> Ask {
        Ask::Write {
            mutation: Mutation::Follow,
            variables: r#"{"target_user_id":"9001"}"#.into(),
            doc_id: "1000000000000003".into(),
            route: "comet.igweb.PolarisProfilePostsTabRoute".into(),
        }
    }

    /// What the tab answers itself costs nothing, and still waits out a
    /// cooldown without asking.
    #[tokio::test]
    async fn the_viewer_is_free_and_waits_out_a_cooldown() {
        let page = Scripted::telling(|_| Ok(viewer()));
        let (client, budget) = spending_client(page.clone());
        let told = client.ask_page(Ask::Viewer, "/").await.unwrap();
        assert!(matches!(told, Told::Viewer(v) if v.pk == Pk::new(42)));
        assert_eq!(client.pacer().spent(), 0);
        assert_eq!(budget.reserved(), (0, 0));
        assert_eq!(page.asks().len(), 1);

        budget.cool_down();
        let error = client.ask_page(Ask::Viewer, "/").await.unwrap_err();
        assert!(matches!(error, IgError::InCooldown { .. }), "{error:?}");
        let error = client.ask_page(Ask::Tray, "/").await.unwrap_err();
        assert!(matches!(error, IgError::InCooldown { .. }), "{error:?}");
        assert_eq!(page.asks().len(), 1, "nothing asked in a cooldown");
    }

    /// A read is one read, and a Ctrl+C gives it up.
    #[tokio::test]
    async fn a_query_spends_one_read_and_stops_on_ctrl_c() {
        let page = Scripted::telling(|_| Ok(Told::Answer(page_said(200, "{}"))));
        let (client, budget) = spending_client(page.clone());
        let told = client.ask_page(query(), "/someone/").await.unwrap();
        assert!(matches!(told, Told::Answer(ref a) if a.body == "{}"));
        assert_eq!(budget.reserved(), (1, 0));
        let call = &page.asks()[0];
        assert_eq!(call.ask, query());
        assert_eq!(call.origin, "http://127.0.0.1:9");
        assert_eq!(call.referrer, "/someone/");
        assert_eq!(call.claim, "0");
        assert_eq!(call.cap, MAX_BODY_BYTES);
        assert_eq!(call.timeout_ms, 60_000);

        // Canceled before: nothing is paid for or asked.
        client.pacer().cancel_token().cancel();
        let error = client.ask_page(query(), "/").await.unwrap_err();
        assert!(matches!(error, IgError::Canceled), "{error:?}");
        assert_eq!(budget.reserved(), (1, 0));
        assert_eq!(page.asks().len(), 1);
    }

    /// Ctrl+C while a read is in flight gives it up at once.
    #[tokio::test]
    async fn a_read_in_flight_is_given_up() {
        let (client, _) = spending_client(Arc::new(Stalled));
        let cancel = client.pacer().cancel_token().clone();
        let asked = tokio::spawn(async move { client.ask_page(query(), "/").await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
        let error = asked.await.unwrap().unwrap_err();
        assert!(matches!(error, IgError::Canceled), "{error:?}");
    }

    /// A write pays the write bucket, and a Ctrl+C once it is in flight does
    /// not abandon it.
    #[tokio::test]
    async fn a_write_spends_one_write_and_is_not_abandoned() {
        let (client, budget) = spending_client(Arc::new(Slow));
        let cancel = client.pacer().cancel_token().clone();
        let asked = tokio::spawn(async move { client.ask_page(follow(), "/someone/").await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();
        let told = asked.await.unwrap().unwrap();
        assert!(matches!(told, Told::Answer(ref a) if a.status == 200));
        assert_eq!(budget.reserved(), (0, 1));
    }

    /// An intent the allowlist refuses never reaches the page, and costs
    /// nothing.
    #[tokio::test]
    async fn a_refused_ask_never_reaches_the_page() {
        let page = Scripted::telling(|_| Ok(Told::Answer(page_said(200, "{}"))));
        let (client, budget) = spending_client(page.clone());
        let pk = Pk::new(9001);
        for ask in [
            Ask::Query {
                operation: Operation::Follow,
                variables: "{}".into(),
            },
            Ask::Pk {
                name: "stories/someone".into(),
            },
            Ask::Document {
                path: "/explore/".into(),
            },
            Ask::Rest {
                read: Rest::Following(pk),
                query: vec![("search_surface".into(), "follow_list_page".into())],
            },
        ] {
            assert!(refused_ask(&ask).is_some(), "{ask:?}");
            let error = client.ask_page(ask, "/").await.unwrap_err();
            assert!(matches!(error, IgError::NotAllowed(_)), "{error:?}");
        }
        assert!(page.asks().is_empty());
        assert_eq!(budget.reserved(), (0, 0));
    }

    /// What the tab could not build maps to what the command tells: a page
    /// not ready is not a dead browser, and a page logged out is a dead
    /// session.
    #[tokio::test]
    async fn a_page_not_ready_or_logged_out_is_told_as_such() {
        let page = Scripted::telling(|call| match call.ask {
            Ask::Query { .. } => Err(PageError::NotReady("a Relay call of the app's".into())),
            _ => Err(PageError::LoggedOut),
        });
        let (client, _) = spending_client(page);
        let error = client.ask_page(query(), "/").await.unwrap_err();
        assert!(
            matches!(&error, IgError::PageNotReady(what) if what == "a Relay call of the app's"),
            "{error:?}"
        );
        assert_eq!(error.reaction(), crate::error::Reaction::Abort);
        assert!(error.to_string().contains("nothing was sent"), "{error}");
        let error = client.ask_page(Ask::Viewer, "/").await.unwrap_err();
        assert!(matches!(error, IgError::SessionExpired), "{error:?}");

        assert!(matches!(
            PageError::from(Unbuilt::Missing(Missing::RELAY_CALL)),
            PageError::NotReady(what) if what == "a Relay call of the app's"
        ));
        assert!(matches!(
            PageError::from(Unbuilt::LoggedOut),
            PageError::LoggedOut
        ));
        assert!(matches!(
            PageError::from(Unbuilt::NotAllowed("POST /x".into())),
            PageError::NotAllowed(what) if what == "POST /x"
        ));
    }

    /// The claim an answer hands out is the one the next ask carries.
    #[tokio::test]
    async fn the_claim_a_told_answer_sets_is_sent_next() {
        let page = Scripted::telling(|call| {
            let mut answer = page_said(200, "{}");
            answer.headers = vec![("x-ig-set-www-claim".into(), "hmac.fresh".into())];
            Ok(match call.ask {
                Ask::Pk { .. } => Told::Pk {
                    answer,
                    pk: RouteAnswer::Pk(Pk::new(9001)),
                },
                _ => Told::Answer(answer),
            })
        });
        let (client, _) = spending_client(page.clone());
        let told = client
            .ask_page(
                Ask::Pk {
                    name: "someone".into(),
                },
                "/",
            )
            .await
            .unwrap();
        assert!(matches!(told, Told::Pk { pk: RouteAnswer::Pk(pk), .. } if pk == Pk::new(9001)));
        client.ask_page(query(), "/").await.unwrap();
        let claims: Vec<String> = page.asks().into_iter().map(|c| c.claim).collect();
        assert_eq!(claims, ["0", "hmac.fresh"]);
    }

    /// An opaque redirect is refused, with nothing more sent.
    #[tokio::test]
    async fn an_opaque_redirect_is_refused() {
        let page = Scripted::telling(|_| Ok(Told::Answer(page_said(0, ""))));
        let (client, budget) = spending_client(page);
        let error = client.ask_page(query(), "/").await.unwrap_err();
        assert!(
            matches!(error, IgError::Unexpected { status: 0, .. }),
            "{error:?}"
        );
        assert_eq!(budget.reserved(), (1, 0));
    }

    /// Each ask, built by the tab's own step on an invented page: the
    /// request it sends, and what the tab answers itself.
    #[tokio::test]
    async fn the_tab_builds_what_is_asked_of_it() {
        let page = Scripted::building(|request| {
            Ok(
                if request.url.ends_with(crate::web::BULK_ROUTE_DEFINITIONS) {
                    page_said(
                        200,
                        r#"for (;;);{"payload":{"payloads":{"/someone/":{"error":false,
                        "result":{"exports":{"hostableView":{"props":{"id":"9001"}}}}}}}}"#,
                    )
                } else {
                    page_said(200, "<html>a document</html>")
                },
            )
        });
        let (client, budget) = spending_client(page.clone());

        let told = client.ask_page(Ask::Viewer, "/").await.unwrap();
        assert!(matches!(told, Told::Viewer(v) if v.pk == Pk::new(42)));
        let told = client
            .ask_page(
                Ask::Pk {
                    name: "someone".into(),
                },
                "/",
            )
            .await
            .unwrap();
        assert!(matches!(told, Told::Pk { pk: RouteAnswer::Pk(pk), .. } if pk == Pk::new(9001)));
        client.ask_page(query(), "/someone/").await.unwrap();
        let told = client
            .ask_page(
                Ask::Document {
                    path: "/someone/".into(),
                },
                "/",
            )
            .await
            .unwrap();
        let Told::Document { answer, bundles } = told else {
            panic!("a document is answered as one");
        };
        assert!(answer.body.is_empty() && bundles.is_empty());
        client.ask_page(follow(), "/someone/").await.unwrap();
        assert_eq!(budget.reserved(), (3, 1));

        let sent: Vec<(String, bool)> = page
            .asked()
            .into_iter()
            .map(|r| (r.url, r.navigate))
            .collect();
        assert_eq!(
            sent,
            [
                (
                    "http://127.0.0.1:9/ajax/bulk-route-definitions/".into(),
                    false
                ),
                ("http://127.0.0.1:9/api/graphql".into(), false),
                ("http://127.0.0.1:9/someone/".into(), true),
                ("http://127.0.0.1:9/api/graphql".into(), false),
            ]
        );
        let relay = &page.asked()[1];
        assert_eq!(
            relay.referrer, "http://127.0.0.1:9/someone/",
            "made from the page it names"
        );
        let body = relay.body.as_deref().unwrap();
        assert!(body.starts_with("av=17841400000000001&"), "{body}");
        assert!(
            body.contains("fb_api_req_friendly_name=PolarisUserHoverCardContentV2Query"),
            "{body}"
        );
    }

    /// Without a browser there is no page to ask.
    #[tokio::test]
    async fn a_client_without_a_page_asks_nothing() {
        let server = wiremock::MockServer::start().await;
        let client = crate::client::harness::client_with(&server, Pacer::unlimited());
        assert!(!client.has_page());
        let error = client.ask_page(Ask::Viewer, "/").await.unwrap_err();
        assert!(matches!(error, IgError::Browser(_)), "{error:?}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A page that never answers.
    struct Stalled;

    impl Page for Stalled {
        fn send(&self, _: PageRequest) -> PageFuture<'_> {
            Box::pin(std::future::pending())
        }

        fn ask(&self, _: Call) -> AskFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    /// A page that answers a moment later.
    struct Slow;

    impl Page for Slow {
        fn send(&self, _: PageRequest) -> PageFuture<'_> {
            Box::pin(std::future::pending())
        }

        fn ask(&self, _: Call) -> AskFuture<'_> {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                Ok(Told::Answer(page_said(200, "{}")))
            })
        }
    }
}
