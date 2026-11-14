//! The endpoints this tool reads, one method per URL: the address, the query,
//! the page a browser would have called from, and how the answer is read.
//! The budget is spent and the headers built in [`IgClient::get`].

use snob_core::Pk;

use crate::allowlist::Rest;
use crate::error::IgError;
use crate::model::web::RouteAnswer;
use crate::model::{
    Counters, FriendshipsPage, Highlight, HighlightsTray, Identity, Reel, ReelsMedia, SearchUser,
    TopSearch, UserInfo, UserInfoEnvelope, WebProfileInfo, WebProfileInfoEnvelope,
};
use crate::page_values::Viewer;
use crate::web::{Ask, Told};

use super::IgClient;
use super::ask::{answered, profile_path, relative, told_otherwise};
use super::headers::Surface;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Followers,
    Following,
}

impl Direction {
    fn segment(self) -> &'static str {
        match self {
            Self::Followers => "followers",
            Self::Following => "following",
        }
    }
}

fn pks_of(page: &FriendshipsPage) -> Vec<Pk> {
    page.users.iter().map(|user| user.pk).collect()
}

impl IgClient {
    /// Checks the session works, with the cheapest request on the endpoint the
    /// walk depends on. **From the browser it costs nothing**: the tab's
    /// document says who it was served to ([`Self::viewer`]).
    pub async fn validate(&self) -> Result<(), IgError> {
        if self.has_page() {
            self.viewer().await?;
            return Ok(());
        }
        let _: FriendshipsPage = self
            .get(
                &format!(
                    "/api/v1/friendships/{}/{}/",
                    self.session.ds_user_id,
                    Direction::Following.segment()
                ),
                &[("count", "1")],
                // The account's own page: the only one nameable before the
                // username is resolved.
                "",
            )
            .await?;
        Ok(())
    }

    /// Identity of the authenticated account. From the browser the name is
    /// the one the tab's document was served under, so a rename shows.
    pub async fn whoami(&self) -> Result<Identity, IgError> {
        if self.has_page() {
            let viewer = self.viewer().await?;
            return Ok(Identity {
                pk: viewer.pk,
                username: Some(viewer.username),
            });
        }
        let pk = self.session.ds_user_id;
        if let Some(username) = &self.session.username {
            self.validate().await?;
            return Ok(Identity {
                pk,
                username: Some(username.clone()),
            });
        }
        let username = self.resolve_username(pk).await?;
        Ok(Identity { pk, username })
    }

    /// `None` if Instagram answers without the field: the name is cosmetic
    /// and must never make a login fail.
    pub async fn resolve_username(&self, pk: Pk) -> Result<Option<String>, IgError> {
        if self.has_page() {
            if pk == self.session.ds_user_id {
                return Ok(Some(self.viewer().await?.username));
            }
            return Ok(Some(self.profile_by_pk(pk, "").await?.username));
        }
        Ok(self.user_info(pk).await?.map(|u| u.username))
    }

    /// The profile of the account called `name`, `known` its pk when an
    /// earlier answer said so. Without a browser, [`Self::web_profile_info`].
    /// From the browser: the pk (`known`, or one read), the profile query,
    /// and once accepted the app's three reads beside it
    /// ([`Self::profile_burst`]).
    ///
    /// **The answer must name `name`**, in any case: a known pk that now
    /// belongs to another name is looked up once more, and anything else is
    /// not found. A route that answers with an error is not "not found".
    pub async fn profile_named(
        &self,
        name: &str,
        known: Option<Pk>,
    ) -> Result<WebProfileInfo, IgError> {
        if !self.has_page() {
            return self.web_profile_info(name).await;
        }
        let missing = || IgError::NotFound {
            what: Some(name.to_string()),
        };
        if !crate::allowlist::profile_name(name) {
            return Err(missing());
        }
        let found = match known {
            Some(pk) => match self.profile_by_pk(pk, name).await {
                Ok(profile) if profile.username.eq_ignore_ascii_case(name) => {
                    self.profile_burst(pk, name).await?;
                    return Ok(profile);
                }
                Ok(_) | Err(IgError::NotFound { .. }) => {
                    tracing::debug!("the stored pk belongs to another name now; looking it up");
                    self.pk_named(name).await?
                }
                Err(e) => return Err(e),
            },
            None => self.pk_named(name).await?,
        };
        let profile = self.profile_by_pk(found, name).await?;
        if profile.username.eq_ignore_ascii_case(name) {
            self.profile_burst(found, name).await?;
            Ok(profile)
        } else {
            Err(missing())
        }
    }

    /// The two counters of account `pk`: the one request that tells whether a
    /// stored list still answers. The hover card from the browser; without
    /// one, [`Self::web_profile_info`], which needs a name, and `None` with
    /// none rather than a request for a certain 404.
    pub async fn counters(
        &self,
        pk: Pk,
        username: Option<&str>,
    ) -> Result<Option<Counters>, IgError> {
        if self.has_page() {
            return match self.hover_card(pk).await? {
                Some(counters) => Ok(Some(counters)),
                None => Err(IgError::NotFound {
                    what: username.map(str::to_string),
                }),
            };
        }
        let Some(username) = username else {
            return Ok(None);
        };
        Ok(Some(self.web_profile_info(username).await?.counters()))
    }

    /// The pk the tab finds behind `name`.
    async fn pk_named(&self, name: &str) -> Result<Pk, IgError> {
        let missing = || IgError::NotFound {
            what: Some(name.to_string()),
        };
        match self.pk_of(name).await {
            Ok(RouteAnswer::Pk(pk)) => Ok(pk),
            Ok(RouteAnswer::NoProfile) | Err(IgError::NotFound { .. }) => Err(missing()),
            Ok(RouteAnswer::Error) => Err(IgError::Unexpected {
                status: 0,
                body: "looking the name up named no account".into(),
            }),
            Err(e) => Err(e),
        }
    }

    /// Who the tab's document was served to: nothing sent, nothing paid. A
    /// document served to nobody, or to another account, is an expired session.
    async fn viewer(&self) -> Result<Viewer, IgError> {
        match self.ask_page(Ask::Viewer, "/").await? {
            Told::Viewer(viewer) => Ok(viewer),
            _ => Err(IgError::Unexpected {
                status: 0,
                body: "the page answered who it was served to with something else".into(),
            }),
        }
    }

    /// `/api/v1/users/{pk}/info/`: the only source of the full-size picture.
    pub async fn user_info(&self, pk: Pk) -> Result<Option<UserInfo>, IgError> {
        let envelope: UserInfoEnvelope = self
            .get(&format!("/api/v1/users/{pk}/info/"), &[], "")
            .await?;
        Ok(envelope.user)
    }

    /// Public profile data, with the counters and the high-resolution picture.
    ///
    /// **One request, and a second only when the first fails in one way**:
    /// the endpoint answers 400 for certain business accounts (`Asset
    /// asset://laser.provider/ig_business_category_subvertical has been
    /// deleted`; verified live in August 2026), and search is tried then.
    /// [`IgError::worth_a_second_route`] keeps this from being a retry: a
    /// push-back, a dead session, a cancel and a 404 all answer `false`.
    ///
    /// Search gives an identity and no counters
    /// ([`WebProfileInfo::counters_are_knowable`]); see "Known walls".
    pub async fn web_profile_info(&self, username: &str) -> Result<WebProfileInfo, IgError> {
        let failure = match self.profile_by_name(username).await {
            Ok(profile) => return Ok(profile),
            Err(e) => e,
        };
        if !failure.worth_a_second_route() {
            return Err(failure);
        }

        tracing::debug!(
            error = %failure,
            "the profile endpoint would not answer; trying search"
        );

        match self.search_user_id(username).await {
            Ok(Some(user)) => {
                tracing::debug!(
                    pk = user.pk.get(),
                    "search resolved the account the profile lost"
                );
                Ok(WebProfileInfo::from_search(user))
            }
            // The original failure is still the truthful one: search not
            // finding the name does not turn a 400 into "no such account".
            Ok(None) => {
                tracing::debug!("search knows no such account either");
                Err(failure)
            }
            // The fallback's failure replaces the real one only when the user
            // has to act on it. `is_push_back`, not `cooldown_for`, which says
            // `None` for an `InCooldown` another process just wrote.
            Err(second) => {
                if second.is_push_back() || second.invalidates_session() {
                    Err(second)
                } else {
                    Err(failure)
                }
            }
        }
    }

    async fn profile_by_name(&self, username: &str) -> Result<WebProfileInfo, IgError> {
        let missing = || IgError::NotFound {
            what: Some(username.to_string()),
        };
        // A 404, or a 200 whose envelope carries no user: one answer.
        let envelope: WebProfileInfoEnvelope = self
            .get(
                "/api/v1/users/web_profile_info/",
                &[("username", username)],
                &format!("{}/", snob_core::model::in_a_path(username)),
            )
            .await
            .map_err(|e| match e {
                IgError::NotFound { .. } => missing(),
                other => other,
            })?;
        envelope.data.user.ok_or_else(missing)
    }

    /// Resolves a username through the web client's search box. **A fallback,
    /// never a first choice** (see [`Self::web_profile_info`]).
    ///
    /// Search matches loosely, so the answer is held to an exact,
    /// case-insensitive match: otherwise a name that does not exist would walk
    /// a stranger's followers. `None` means search knows no such account.
    pub async fn search_user_id(&self, username: &str) -> Result<Option<SearchUser>, IgError> {
        let found: TopSearch = self
            .get(
                "/web/search/topsearch/",
                &[("context", "blended"), ("query", username), ("count", "1")],
                "",
            )
            .await?;

        Ok(found
            .users
            .into_iter()
            .map(|hit| hit.user)
            .find(|user| user.username.eq_ignore_ascii_case(username)))
    }

    /// One page of followers or following, asked as the web app asks it:
    /// `count` ([`crate::pace::ACCOUNTS_PER_PAGE`]), then the cursor, then
    /// `search_surface` on followers alone (the order of the capture's 36
    /// later pages). From the browser it is followed by the app's `show_many`
    /// ([`Self::statuses`]). `username` only names the referring page.
    pub async fn friendships_page(
        &self,
        pk: Pk,
        username: &str,
        direction: Direction,
        count: u32,
        cursor: Option<&str>,
    ) -> Result<FriendshipsPage, IgError> {
        let count = count.to_string();
        let mut query: Vec<(&str, &str)> = vec![("count", &count)];
        if let Some(c) = cursor {
            query.push(("max_id", c));
        }
        if direction == Direction::Followers {
            query.push(("search_surface", "follow_list_page"));
        }
        let referer = profile_path(username);
        let read = match direction {
            Direction::Followers => Rest::Followers(pk),
            Direction::Following => Rest::Following(pk),
        };
        let page: FriendshipsPage = self.rest_read(read, &query, &referer).await?;
        if self.has_page() {
            self.statuses(&pks_of(&page), &referer).await?;
        }
        self.charge_accounts(page.users.len()).await;
        Ok(page)
    }

    /// The app's navigation as one of `username`'s lists opens, before its
    /// first page: to `/<name>/`, or for the mutual followers to
    /// `/<name>/followers/mutualOnly` (every list read of the capture of
    /// 2026-10-01 was preceded by one). From the browser only, and only for a
    /// name that can be a profile's.
    ///
    /// The tab stays where it is: a history push the app's router did not make
    /// would leave the address disagreeing with the route every read is built
    /// after (`web::made_from`).
    pub async fn open_list(&self, username: &str, mutual: bool) -> Result<(), IgError> {
        if !self.has_page() || !crate::allowlist::profile_name(username) {
            return Ok(());
        }
        let route = if mutual {
            format!("/{username}/followers/mutualOnly")
        } else {
            format!("/{username}/")
        };
        self.navigation(&route).await
    }

    /// Loads the profile whose list is being read again, as the app does when
    /// the machine wakes: one paid read (`web::Ask::Document`), from the
    /// browser only. A name that cannot be a profile's loads the home page.
    pub async fn reload(&self, username: &str) -> Result<(), IgError> {
        if !self.has_page() {
            return Ok(());
        }
        let path = if crate::allowlist::profile_name(username) {
            profile_path(username)
        } else {
            "/".to_string()
        };
        let ask = Ask::Document { path };
        let endpoint = ask.endpoint();
        let Told::Document { answer, .. } = self.ask_page(ask, "").await? else {
            return Err(told_otherwise("a document"));
        };
        let answer = answered(&endpoint, answer);
        if !answer.is_success() {
            return Err(self.refuse(&answer));
        }
        Ok(())
    }

    /// A page as HTML, as a browser navigating to it would get it: where
    /// `fb_dtsg` and `lsd` are handed out ([`crate::graphql`]). A large
    /// request (about 600 KB for a profile), so callers cache what they find.
    ///
    /// **Only without a browser**: the tab hands no document's HTML out.
    pub async fn page(&self, path: &str) -> Result<String, IgError> {
        let answer = self.get_body(path, &[], "", Surface::Document).await?;
        // No `declares_failure`: this is HTML, not JSON.
        if !answer.is_success() {
            return Err(self.refuse(&answer));
        }
        Ok(answer.body)
    }

    /// The accounts the viewer follows that follow this one, a page at a time.
    ///
    /// **Twelve per page because that is what the web client asks for**
    /// (capture of August 2026); no other size has been sent. The cursor is
    /// the offset as a string; the reply has a followers page's shape.
    pub async fn mutual_followers_page(
        &self,
        pk: Pk,
        username: &str,
        cursor: Option<&str>,
    ) -> Result<FriendshipsPage, IgError> {
        const PAGE_SIZE: &str = "12";
        let mut query: Vec<(&str, &str)> = vec![("page_size", PAGE_SIZE)];
        if let Some(c) = cursor {
            query.push(("max_id", c));
        }
        // The "mutual" tab of the followers dialog has an address of its own.
        let referer = if username.is_empty() {
            profile_path(username)
        } else {
            format!("{}followers/mutualOnly", profile_path(username))
        };
        let page: FriendshipsPage = self
            .rest_read(Rest::MutualFollowers(pk), &query, &referer)
            .await?;
        if self.has_page() {
            self.statuses(&pks_of(&page), &referer).await?;
        }
        self.charge_accounts(page.users.len()).await;
        Ok(page)
    }

    /// The highlights under an account's bio, without their items. One request
    /// (verified live in August 2026). From the browser it is the web client's
    /// tray query, which carries **no size and no dates**.
    pub async fn highlights_tray(&self, pk: Pk, username: &str) -> Result<Vec<Highlight>, IgError> {
        if self.has_page() {
            if let Some(tray) = self.take_held_tray(pk) {
                return Ok(tray);
            }
            return self.highlights_tray_by_pk(pk, username).await;
        }
        let tray: HighlightsTray = self
            .get(
                &format!("/api/v1/highlights/{pk}/highlights_tray/"),
                &[],
                relative(&profile_path(username)),
            )
            .await?;
        Ok(tray.tray)
    }

    /// The items of one highlight: [`Self::stories`]'s endpoint asked with
    /// `highlight:<id>` (the tray's spelling, prefix included). From the
    /// browser, [`Self::highlight_window`], which needs `tray`. `None` for an
    /// id that no longer exists.
    ///
    /// **Reading it does not tell anybody you looked**: a view is registered
    /// by a separate mutation this project has no code to send (`no_seen.rs`).
    pub async fn highlight(
        &self,
        id: &str,
        tray: &[String],
        username: &str,
    ) -> Result<Option<Reel>, IgError> {
        if self.has_page() {
            return self.highlight_window(id, tray, username).await;
        }
        let envelope: ReelsMedia = self
            .get(
                "/api/v1/feed/reels_media/",
                &[("reel_ids", id)],
                // A highlight opens at an address of its own, with the bare id.
                &format!(
                    "stories/highlights/{}/",
                    id.strip_prefix("highlight:").unwrap_or(id)
                ),
            )
            .await?;
        Ok(envelope.reel())
    }

    /// The stories an account has up right now. One request. `None` means
    /// nothing is up: the endpoint answers 200 with an empty envelope.
    ///
    /// **This does not tell anybody you looked**, as for [`Self::highlight`].
    ///
    /// From the browser it is the story gallery ([`Self::reel_gallery`]),
    /// opened over the home page's tray when it holds `pk`, and over `pk`
    /// alone otherwise.
    pub async fn stories(&self, pk: Pk, username: &str) -> Result<Option<Reel>, IgError> {
        if self.has_page() {
            let id = pk.to_string();
            let reel_ids = match self.tray().await? {
                Some(tray) if tray.contains(&id) => tray,
                _ => vec![id],
            };
            return self.reel_gallery(pk, &reel_ids).await;
        }
        let ids = pk.to_string();
        let envelope: ReelsMedia = self
            .get(
                "/api/v1/feed/reels_media/",
                &[("reel_ids", ids.as_str())],
                &if username.is_empty() {
                    String::new()
                } else {
                    format!("{}/", snob_core::model::in_a_path(username))
                },
            )
            .await?;
        Ok(envelope.reel())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use snob_core::session::{Session, SessionOrigin};
    use url::Url;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::client::harness::{Recording, SID, UA, client};

    /// Without a browser, the REST reads the page refuses to fetch are still
    /// sent, each over `reqwest`: the allowlist judges what leaves a page,
    /// and this client has none.
    #[tokio::test]
    async fn without_a_page_the_rest_reads_are_still_sent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;
        let rest = client(&server).await;
        assert!(!rest.has_page());
        let pk = Pk::new(2_345_678_901);
        let _ = rest.web_profile_info("someone").await;
        let _ = rest.user_info(pk).await;
        let _ = rest.search_user_id("someone").await;
        let _ = rest.highlights_tray(pk, "someone").await;
        let _ = rest
            .highlight("highlight:17900000000000001", &[], "someone")
            .await;
        let _ = rest.stories(pk, "someone").await;

        let asked: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| r.url.path().to_string())
            .collect();
        for sent in [
            "/api/v1/users/web_profile_info/",
            "/api/v1/users/2345678901/info/",
            "/web/search/topsearch/",
            "/api/v1/highlights/2345678901/highlights_tray/",
            "/api/v1/feed/reels_media/",
        ] {
            assert!(asked.iter().any(|path| path == sent), "{sent} in {asked:?}");
        }
        assert_eq!(
            asked
                .iter()
                .filter(|path| *path == "/api/v1/feed/reels_media/")
                .count(),
            2,
            "a highlight's and the stories'"
        );
    }

    /// A list is asked the way the web app asks it, on both paths: `count`,
    /// the cursor, then `search_surface` on the followers list alone, from
    /// the account's own page.
    #[tokio::test]
    async fn a_list_is_asked_as_the_app_asks_it_on_both_paths() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"users":[]}"#))
            .mount(&server)
            .await;
        let rest = client(&server).await;
        let page = crate::client::harness::Scripted::building(|_| {
            Ok(crate::client::harness::page_said(200, r#"{"users":[]}"#))
        });
        let (tab, _) = crate::client::harness::spending_client(page.clone());

        for client in [&rest, &tab] {
            for direction in [Direction::Followers, Direction::Following] {
                client
                    .friendships_page(Pk::new(7), "someone", direction, 12, Some("24"))
                    .await
                    .unwrap();
            }
        }

        let sent: Vec<(String, String)> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let referer = r.headers.get("referer").unwrap().to_str().unwrap();
                (
                    format!("{}?{}", r.url.path(), r.url.query().unwrap_or_default()),
                    referer.to_string(),
                )
            })
            .collect();
        let asked: Vec<(String, String)> = page
            .asked()
            .into_iter()
            .map(|r| {
                let url = url::Url::parse(&r.url).unwrap();
                (
                    format!("{}?{}", url.path(), url.query().unwrap_or_default()),
                    r.referrer,
                )
            })
            .collect();
        let expected = |origin: &str| {
            vec![
                (
                    "/api/v1/friendships/7/followers/?count=12&max_id=24&search_surface=follow_list_page"
                        .to_string(),
                    format!("{origin}/someone/"),
                ),
                (
                    "/api/v1/friendships/7/following/?count=12&max_id=24".to_string(),
                    format!("{origin}/someone/"),
                ),
            ]
        };
        assert_eq!(sent, expected("https://www.instagram.com"));
        assert_eq!(asked, expected("http://127.0.0.1:9"));
    }

    /// A name that cannot go in a header verbatim must still get an answer
    /// about the name.
    ///
    /// `target::clean` strips a leading `@` and nothing else, and `watch.toml`
    /// does not validate a username at all, so the referer is the one
    /// name-in-a-URL in this crate that arrives as typed. Any byte below 0x20
    /// makes the header unbuildable; reqwest holds that failure until `send()`,
    /// where `?` reads it as `Network` — a *retryable* fault, so the pager
    /// sends the same doomed request three more times and the pacer charges
    /// for four requests that never left the machine. The name it was asking
    /// about is never mentioned.
    #[tokio::test]
    async fn a_hostile_name_still_reaches_the_not_found_arm() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":{"user":null}}"#))
            .mount(&server)
            .await;

        let hostile = "gh\u{1b}[2K";
        let error = client(&server)
            .await
            .web_profile_info(hostile)
            .await
            .unwrap_err();

        assert!(
            matches!(&error, IgError::NotFound { what: Some(name) } if name == hostile),
            "{error:?}"
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            1,
            "the request the budget paid for went out"
        );
        assert_eq!(
            requests[0]
                .headers
                .get("referer")
                .unwrap()
                .to_str()
                .unwrap(),
            "https://www.instagram.com/gh%1B%5B2K/",
            "encoded, not filtered: a name with a character removed is a different account"
        );
    }

    #[tokio::test]
    async fn it_reads_a_page_of_followers() {
        let server = MockServer::start().await;
        let body = r#"{"users":[
            {"pk":"1","username":"one","full_name":"One","is_verified":true,"is_private":false},
            {"pk":2,"username":"two"}
        ],"next_max_id":"QVFB","status":"ok"}"#;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/42/followers/"))
            .and(query_param("count", "50"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let page = client(&server)
            .await
            .friendships_page(Pk::new(42), "someone", Direction::Followers, 50, None)
            .await
            .unwrap();

        assert_eq!(page.users.len(), 2);
        assert_eq!(page.users[0].username, "one");
        assert_eq!(page.users[0].is_verified, Some(true));
        assert_eq!(page.users[1].full_name, None);
        assert_eq!(page.next_cursor(), Some("QVFB"));
    }

    #[tokio::test]
    async fn the_cursor_is_sent_as_max_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("max_id", "QVFB"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"users":[]}"#))
            .mount(&server)
            .await;

        client(&server)
            .await
            .friendships_page(
                Pk::new(42),
                "someone",
                Direction::Following,
                50,
                Some("QVFB"),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn it_resolves_a_username_from_an_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/42/info/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"user":{"pk":42,"username":"whoever"},"status":"ok"}"#),
            )
            .mount(&server)
            .await;

        let name = client(&server)
            .await
            .resolve_username(Pk::new(42))
            .await
            .unwrap();
        assert_eq!(name.as_deref(), Some("whoever"));
    }

    /// The live answer to a name nobody owns: a 404 carrying a web page. What
    /// reaches the terminal must be the account name, not the markup.
    #[tokio::test]
    async fn a_profile_that_does_not_exist_is_named_not_dumped() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/web_profile_info/"))
            .respond_with(ResponseTemplate::new(404).set_body_string(
                "<!DOCTYPE html><html><head><title>Page Not Found</title></head></html>",
            ))
            .mount(&server)
            .await;

        let error = client(&server)
            .await
            .web_profile_info("nobody")
            .await
            .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("nobody"), "{message}");
        assert!(!message.contains("DOCTYPE"), "{message}");
    }

    /// The other half of the same answer: a 200 whose envelope has no user.
    /// It must read exactly like the 404 above.
    #[tokio::test]
    async fn an_empty_profile_envelope_reads_like_the_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/web_profile_info/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"data":{"user":null}}"#))
            .mount(&server)
            .await;

        let error = client(&server)
            .await
            .web_profile_info("nobody")
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "the account \"nobody\" does not exist");
    }

    /// The body Instagram really sends for the accounts this fallback exists
    /// for. Captured live in August 2026.
    const BROKEN_PROFILE: &str = r#"{"message":"Asset asset://laser.provider/ig_business_category_subvertical has been deleted. You cannot use this schema","status":"fail"}"#;

    /// One search hit, shaped like the live answer: `pk` as a string, no
    /// counters anywhere, and the relationship under `friendship_status`.
    fn search_body(username: &str, pk: &str) -> String {
        format!(
            r#"{{"users":[{{"position":0,"user":{{"pk":"{pk}","username":"{username}",
               "full_name":"Someone","is_private":false,"is_verified":true,
               "profile_pic_url":"https://cdninstagram.com/p.jpg",
               "friendship_status":{{"following":true,"outgoing_request":false,
               "is_private":false}}}}}}],"status":"ok"}}"#
        )
    }

    fn mount_profile(server: &MockServer, response: ResponseTemplate) -> impl Future<Output = ()> {
        Mock::given(method("GET"))
            .and(path("/api/v1/users/web_profile_info/"))
            .respond_with(response)
            .mount(server)
    }

    /// The ordinary account is untouched: one request, the full answer, and the
    /// route it came from says so.
    ///
    /// This is the half that is easy to break while fixing the other one. The
    /// fallback must not cost anybody who does not need it a second request, so
    /// the charge is asserted rather than assumed.
    #[tokio::test]
    async fn an_ordinary_profile_still_costs_one_request() {
        let server = MockServer::start().await;
        mount_profile(
            &server,
            ResponseTemplate::new(200).set_body_string(
                r#"{"data":{"user":{"id":"7","username":"ann",
                   "edge_followed_by":{"count":10},"edge_follow":{"count":4}}}}"#,
            ),
        )
        .await;

        let client = client(&server).await;
        let profile = client.web_profile_info("ann").await.unwrap();

        assert_eq!(profile.id, Pk::new(7));
        assert_eq!(profile.via, crate::model::Via::Profile);
        assert!(profile.counters_are_knowable());
        assert_eq!(profile.follower_count(), Some(10));
        assert_eq!(client.pacer().spent(), 1, "the fallback was not needed");

        let asked = server.received_requests().await.unwrap();
        assert_eq!(asked.len(), 1, "search was reached on a working account");
    }

    /// Instagram failing to serialize its own reply does not take the account
    /// down with it.
    ///
    /// The 400 here is verbatim from the live API. Every command that names an
    /// account starts by turning the name into an id, so without the fallback
    /// this one body stops `pfp`, `scan` and every set command.
    #[tokio::test]
    async fn a_profile_instagram_cannot_serialize_is_resolved_by_search() {
        let server = MockServer::start().await;
        mount_profile(
            &server,
            ResponseTemplate::new(400).set_body_string(BROKEN_PROFILE),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/web/search/topsearch/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(search_body("rubius", "1506")))
            .mount(&server)
            .await;

        let client = client(&server).await;
        let profile = client.web_profile_info("rubius").await.unwrap();

        assert_eq!(profile.id, Pk::new(1506));
        assert_eq!(profile.username, "rubius");
        assert_eq!(profile.via, crate::model::Via::Search);

        // The two facts the private-account refusal turns on survive the
        // change of route, under different names.
        assert_eq!(profile.followed_by_viewer, Some(true));
        assert_eq!(profile.requested_by_viewer, Some(false));

        // And the counters do not. **`None`, never `Some(0)`**: a declared zero
        // is what would make `pager::verify_completion` call every short walk
        // complete.
        assert!(!profile.counters_are_knowable());
        assert_eq!(profile.follower_count(), None);
        assert_eq!(profile.following_count(), None);

        assert_eq!(client.pacer().spent(), 2, "the failure and the fallback");
    }

    /// A push-back is never worked around.
    ///
    /// This is the rule the whole fallback is written around: when a service
    /// says no, the answer is to stop asking. A 429 already carries a cooldown
    /// by the time the fallback would be considered, and sending a second
    /// request into an endpoint that has just refused is exactly how a
    /// momentary limit becomes a lasting one.
    #[tokio::test]
    async fn a_push_back_is_not_worked_around() {
        let server = MockServer::start().await;
        mount_profile(
            &server,
            ResponseTemplate::new(429).set_body_string(r#"{"message":"feedback_required"}"#),
        )
        .await;

        let budget = Arc::new(Recording::default());
        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        let client = IgClient::new(session, crate::pace::Pacer::new(budget.clone()))
            .unwrap()
            .with_base_url(Url::parse(&server.uri()).unwrap());

        let error = client.web_profile_info("ann").await.unwrap_err();
        assert!(matches!(error, IgError::FeedbackRequired), "{error:?}");
        assert_eq!(
            client.pacer().spent(),
            1,
            "a second request was sent anyway"
        );

        let asked = server.received_requests().await.unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(budget.calls().len(), 1, "the cooldown still gets recorded");
    }

    /// Every answer that means "no" means no, and a 404 is a real answer.
    ///
    /// Asking search about a name nobody owns spends a request to be told the
    /// same thing, and the two session errors and the cancel must not be
    /// retried at all.
    #[test]
    fn only_a_broken_answer_earns_a_second_route() {
        assert!(
            IgError::Unexpected {
                status: 400,
                body: BROKEN_PROFILE.into()
            }
            .worth_a_second_route()
        );

        for refused in [
            IgError::RateLimited,
            IgError::FeedbackRequired,
            IgError::Challenge { url: None },
            IgError::Checkpoint { url: None },
            IgError::SessionExpired,
            IgError::UserAgentMismatch,
            IgError::Canceled,
            IgError::NotFound { what: None },
            IgError::Decode("a captive portal".into()),
            // The server being unwell is what `Reaction::Retry` is for, and a
            // second route there would hide an outage behind a worse answer.
            IgError::Unexpected {
                status: 503,
                body: String::new(),
            },
        ] {
            assert!(
                !refused.worth_a_second_route(),
                "{refused:?} would be worked around"
            );
        }
    }

    /// Search matches loosely, and a loose match is a different account.
    ///
    /// This is the failure the fallback could introduce and the reason the
    /// exact-name comparison is in `search_user_id` rather than left to a
    /// caller: without it, asking about a name that Instagram would not serve
    /// hands back whatever the search box suggested instead, and the run then
    /// walks a stranger's followers under the name that was typed.
    #[tokio::test]
    async fn search_does_not_hand_back_somebody_else() {
        let server = MockServer::start().await;
        mount_profile(
            &server,
            ResponseTemplate::new(400).set_body_string(BROKEN_PROFILE),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/web/search/topsearch/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(search_body("rubius_fanpage", "999")),
            )
            .mount(&server)
            .await;

        let client = client(&server).await;
        let error = client.web_profile_info("rubius").await.unwrap_err();

        // The original failure, not a wrong account and not a "no such
        // account": search not finding it is no evidence the name is free.
        assert!(
            matches!(&error, IgError::Unexpected { status: 400, .. }),
            "{error:?}"
        );
    }

    /// Case is not identity, but it is not a different account either.
    #[tokio::test]
    async fn search_matches_the_name_whatever_its_case() {
        let server = MockServer::start().await;
        mount_profile(
            &server,
            ResponseTemplate::new(400).set_body_string(BROKEN_PROFILE),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/web/search/topsearch/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(search_body("Rubius", "12")))
            .mount(&server)
            .await;

        let client = client(&server).await;
        let profile = client.web_profile_info("rubius").await.unwrap();
        assert_eq!(profile.id, Pk::new(12));
    }

    /// When the fallback is the one that hits the wall, the wall is what gets
    /// reported.
    ///
    /// A cooldown recorded on the second request is a fact about the account
    /// that the user has to act on, and letting the first failure stand would
    /// bury it under a message about serialization.
    #[tokio::test]
    async fn a_cooldown_on_the_fallback_is_not_swallowed() {
        let server = MockServer::start().await;
        mount_profile(
            &server,
            ResponseTemplate::new(400).set_body_string(BROKEN_PROFILE),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/web/search/topsearch/"))
            .respond_with(ResponseTemplate::new(429).set_body_string(r#"{"message":"spam"}"#))
            .mount(&server)
            .await;

        let client = client(&server).await;
        let error = client.web_profile_info("rubius").await.unwrap_err();
        assert!(matches!(error, IgError::RateLimited), "{error:?}");
    }

    /// Both envelope shapes carry the same reel, and the caller cannot tell
    /// which arrived. Reading only the one seen during development is how this
    /// breaks quietly when Instagram switches.
    #[tokio::test]
    async fn stories_are_read_out_of_either_envelope() {
        let item = r#"{"pk":"1","media_type":1,"taken_at":100,"expiring_at":200,
            "image_versions2":{"candidates":[{"url":"https://x/s.jpg","width":640,"height":1136},
            {"url":"https://x/b.jpg","width":1080,"height":1920}]}}"#;

        for envelope in [
            format!(r#"{{"reels_media":[{{"items":[{item}]}}]}}"#),
            format!(r#"{{"reels":{{"42":{{"items":[{item}]}}}}}}"#),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/api/v1/feed/reels_media/"))
                .and(query_param("reel_ids", "42"))
                .respond_with(ResponseTemplate::new(200).set_body_string(&envelope))
                .mount(&server)
                .await;

            let reel = client(&server)
                .await
                .stories(Pk::new(42), "someone")
                .await
                .unwrap()
                .expect("a reel");
            assert_eq!(reel.items.len(), 1);
            assert_eq!(
                crate::model::largest(&reel.items[0].image_versions2.clone().unwrap().candidates)
                    .unwrap()
                    .url,
                "https://x/b.jpg",
                "the biggest candidate wins, not the first"
            );
        }
    }

    /// An account with nothing up is not a missing account.
    #[tokio::test]
    async fn an_account_with_no_stories_is_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/feed/reels_media/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"reels_media":[]}"#))
            .mount(&server)
            .await;

        assert!(
            client(&server)
                .await
                .stories(Pk::new(42), "someone")
                .await
                .unwrap()
                .is_none()
        );
    }
    /// The profile carries what the page opens with, and the reader keeps it.
    ///
    /// The fixture is the shape of a live answer from August 2026, with the
    /// names changed: counters as `edge_*` objects, the mutual preview as a
    /// GraphQL edge list, the highlight count as a bare number and the
    /// category as an empty string for an account that has none.
    #[tokio::test]
    async fn the_profile_page_fields_are_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/web_profile_info/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"data":{"user":{"id":"7","username":"someone","full_name":"Some One",
                "biography":"hello","external_url":null,"is_private":true,"is_verified":false,
                "followed_by_viewer":true,"follows_viewer":true,"requested_by_viewer":false,
                "has_requested_viewer":false,"edge_followed_by":{"count":244},
                "edge_follow":{"count":319},"highlight_reel_count":2,
                "edge_mutual_followed_by":{"count":33,"edges":[{"node":{"username":"ana"}},
                {"node":{"username":"luis"}},{"node":{"username":"eva"}}]},
                "is_business_account":false,"category_name":"",
                "edge_owner_to_timeline_media":{"count":0,"page_info":{"has_next_page":false,
                "end_cursor":null},"edges":[]}}},"status":"ok"}"#,
            ))
            .mount(&server)
            .await;

        let profile = client(&server)
            .await
            .web_profile_info("someone")
            .await
            .unwrap();
        assert_eq!(profile.follower_count(), Some(244));
        assert_eq!(profile.following_count(), Some(319));
        assert_eq!(profile.posts.map(|e| e.count), Some(0));
        assert_eq!(profile.follows_viewer, Some(true));
        assert_eq!(profile.highlight_reel_count, Some(2));
        assert_eq!(profile.biography.as_deref(), Some("hello"));
        assert_eq!(profile.category_name.as_deref(), Some(""));
        let mutual = profile.mutual.as_ref().expect("the preview is there");
        assert_eq!(mutual.count, 33);
        assert_eq!(mutual.names().collect::<Vec<_>>(), ["ana", "luis", "eva"]);
    }

    /// The tray: one request, made from the account's own page, and every
    /// highlight with its id spelled the way the items are then asked for.
    #[tokio::test]
    async fn the_highlights_tray_is_read_from_the_profile_page() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/highlights/7/highlights_tray/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"tray":[{"id":"highlight:17900000000000001","reel_type":"highlight_reel",
                "title":"trip","created_at":1690000000,"media_count":5,
                "updated_timestamp":1700100000,"latest_reel_media":1700050000,
                "cover_media":{"cropped_image_version":{"width":150,"height":150,
                "url":"https://cdn.test/cover.jpg"}}},
                {"id":"highlight:17900000000000002","title":"","media_count":6}],
                "status":"ok"}"#,
            ))
            .mount(&server)
            .await;

        let tray = client(&server)
            .await
            .highlights_tray(Pk::new(7), "someone")
            .await
            .unwrap();
        assert_eq!(tray.len(), 2);
        assert_eq!(tray[0].id, "highlight:17900000000000001");
        assert_eq!(tray[0].title.as_deref(), Some("trip"));
        assert_eq!(tray[0].media_count, Some(5));
        assert_eq!(
            tray[0].updated_timestamp,
            Some(snob_core::Epoch::new(1_700_100_000))
        );
        assert_eq!(
            tray[0]
                .cover_media
                .as_ref()
                .and_then(|c| c.cropped_image_version.as_ref())
                .map(|p| p.url.as_str()),
            Some("https://cdn.test/cover.jpg")
        );
        assert_eq!(tray[1].created_at, None);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get("referer")
                .unwrap()
                .to_str()
                .unwrap(),
            "https://www.instagram.com/someone/"
        );

        // An account with none answers with an empty tray, not an error.
        let empty = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"tray":[],"status":"ok"}"#),
            )
            .mount(&empty)
            .await;
        assert!(
            client(&empty)
                .await
                .highlights_tray(Pk::new(7), "someone")
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A highlight's items come from the stories endpoint, asked with the
    /// prefixed id, from the highlight's own page, and parse as a reel.
    #[tokio::test]
    async fn a_highlight_is_a_reel_asked_for_by_its_prefixed_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/feed/reels_media/"))
            .and(query_param("reel_ids", "highlight:17900000000000001"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"reels":{"highlight:17900000000000001":{"id":"highlight:17900000000000001",
                "reel_type":"highlight_reel","title":"trip","media_count":1,
                "user":{"pk":"7","username":"someone","is_private":true},
                "items":[{"pk":"3600000000000000001","media_type":2,"taken_at":1700000000,
                "image_versions2":{"candidates":[{"url":"https://cdn.test/a.jpg","width":720,
                "height":1278}]},"video_versions":[{"url":"https://cdn.test/a.mp4","width":720,
                "height":1278}]}]}},"reels_media":[{"id":"highlight:17900000000000001",
                "items":[{"pk":"3600000000000000001","media_type":2,"taken_at":1700000000}]}],
                "status":"ok"}"#,
            ))
            .mount(&server)
            .await;

        let reel = client(&server)
            .await
            .highlight("highlight:17900000000000001", &[], "someone")
            .await
            .unwrap()
            .expect("the highlight has an item");
        assert_eq!(reel.items.len(), 1);
        assert_eq!(reel.items[0].pk, "3600000000000000001");
        // Absent on every highlight item, which is why the field is optional.
        assert_eq!(reel.items[0].expiring_at, None);

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get("referer")
                .unwrap()
                .to_str()
                .unwrap(),
            "https://www.instagram.com/stories/highlights/17900000000000001/"
        );
    }

    /// The mutual list is a followers page by another name, twelve at a time,
    /// asked from the dialog's own address, with the offset as the cursor.
    #[tokio::test]
    async fn the_mutual_list_pages_twelve_at_a_time_from_the_dialog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/friendships/7/mutual_followers/"))
            .and(query_param("page_size", "12"))
            .and(query_param("max_id", "12"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"users":[{"pk":12345678901,"username":"friend","full_name":"D",
                "is_private":true,"is_verified":false}],"big_list":false,"page_size":12,
                "status":"ok"}"#,
            ))
            .mount(&server)
            .await;

        let page = client(&server)
            .await
            .mutual_followers_page(Pk::new(7), "someone", Some("12"))
            .await
            .unwrap();
        assert_eq!(page.users.len(), 1);
        assert_eq!(page.users[0].username, "friend");
        assert_eq!(page.next_max_id, None, "the last page carries no cursor");

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests[0]
                .headers
                .get("referer")
                .unwrap()
                .to_str()
                .unwrap(),
            "https://www.instagram.com/someone/followers/mutualOnly"
        );
    }

    /// From the browser, the session is checked and named by the tab's
    /// document: the viewer is asked, nothing is sent and nothing is paid.
    #[tokio::test]
    async fn with_a_page_the_session_is_checked_from_the_viewer_for_nothing() {
        use crate::client::harness::{Scripted, spending_client};

        let page = Scripted::building(|_| panic!("nothing is sent"));
        let (client, budget) = spending_client(page.clone());
        client.validate().await.unwrap();
        let identity = client.whoami().await.unwrap();
        assert_eq!(identity.pk, Pk::new(42));
        assert_eq!(identity.username.as_deref(), Some("some.viewer"));
        let own = client.resolve_username(Pk::new(42)).await.unwrap();
        assert_eq!(own.as_deref(), Some("some.viewer"));
        assert_eq!(page.asks().len(), 3);
        assert!(page.asks().iter().all(|call| call.ask == Ask::Viewer));
        assert!(page.asked().is_empty());
        assert_eq!(budget.reserved(), (0, 0));
    }

    /// A tab served a logged-out document, or another account's, is an
    /// expired session.
    #[tokio::test]
    async fn with_a_page_a_logged_out_document_is_an_expired_session() {
        use crate::client::harness::{Scripted, spending_client};
        use crate::client::page::PageError;

        let page = Scripted::telling(|_| Err(PageError::LoggedOut));
        let (client, _) = spending_client(page);
        let error = client.validate().await.unwrap_err();
        assert!(matches!(error, IgError::SessionExpired), "{error:?}");
    }

    /// A fake Instagram behind a page that builds each ask as the tab does:
    /// the route definitions know `routes`, a name, its pk; `broken` is
    /// answered with an error; the profile query and the hover card know
    /// `profiles`, a pk, its name, and describe 42 as the viewer's own.
    fn building_instagram(
        routes: &'static [(&'static str, u64)],
        profiles: &'static [(u64, &'static str)],
    ) -> Arc<crate::client::harness::Scripted> {
        crate::client::harness::Scripted::building(move |request| {
            instagram_answer(routes, profiles, request)
        })
    }

    /// What the fake of [`building_instagram`] answers `request` with.
    fn instagram_answer(
        routes: &'static [(&'static str, u64)],
        profiles: &'static [(u64, &'static str)],
        request: &crate::client::page::PageRequest,
    ) -> Result<crate::client::page::PageResponse, crate::client::page::PageError> {
        use crate::client::harness::page_said;
        {
            let form: Vec<(String, String)> =
                url::form_urlencoded::parse(request.body.as_deref().unwrap_or_default().as_bytes())
                    .into_owned()
                    .collect();
            let field = |name: &str| {
                form.iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            if request.url.ends_with("/ajax/bulk-route-definitions/") {
                let route = field("route_urls[0]");
                let name = route.trim_matches('/');
                let entry = match routes.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
                    _ if name == "broken" => r#"{"error":{"code":1}}"#.to_string(),
                    Some((_, pk)) => format!(
                        r#"{{"error":false,"result":{{"type":"route_definition","exports":{{"hostableView":{{"props":{{"id":"{pk}"}}}}}}}}}}"#
                    ),
                    None => r#"{"error":false,"result":{"type":"route_redirect"}}"#.to_string(),
                };
                return Ok(page_said(
                    200,
                    &format!(r#"for (;;);{{"payload":{{"payloads":{{"{route}":{entry}}}}}}}"#),
                ));
            }
            // The reads that open a profile beside its query.
            let burst = match field("fb_api_req_friendly_name").as_str() {
                "PolarisProfileStoryHighlightsTrayContentQuery" => Some(
                    r#"{"data":{"highlights":{"edges":[],"page_info":{"has_next_page":false}}}}"#,
                ),
                "PolarisProfileNoteBubbleQuery" => {
                    Some(r#"{"data":{"xdt_get_inbox_tray_items":{"inbox_tray_items":[]}}}"#)
                }
                "PolarisSchoolPartnerProfileBadgeQuery" => {
                    Some(r#"{"data":{"xig_user_by_igid_v2":{"school_partner":null}}}"#)
                }
                _ => None,
            };
            if let Some(burst) = burst {
                return Ok(page_said(200, burst));
            }
            let variables: serde_json::Value = serde_json::from_str(&field("variables")).unwrap();
            let hover = field("fb_api_req_friendly_name") == "PolarisUserHoverCardContentV2Query";
            let id = if hover { "userID" } else { "id" };
            let pk: u64 = variables[id].as_str().unwrap().parse().unwrap();
            let user = match profiles.iter().find(|(p, _)| *p == pk) {
                Some((pk, name)) => {
                    let relationship = if *pk == 42 {
                        "null"
                    } else {
                        r#"{"following":false,"followed_by":true,"outgoing_request":false,"incoming_request":false}"#
                    };
                    format!(
                        r#"{{"pk":"{pk}","username":"{name}","follower_count":10,"following_count":20,"friendship_status":{relationship}}}"#
                    )
                }
                None => "null".to_string(),
            };
            if hover {
                return Ok(page_said(
                    200,
                    &format!(r#"{{"data":{{"xig_user_by_igid_v2":{{"user_dict":{user}}}}}}}"#),
                ));
            }
            Ok(page_said(
                200,
                &format!(r#"{{"data":{{"user":{user},"viewer":{{"user":{{"pk":"42"}}}}}}}}"#),
            ))
        }
    }

    const ROUTES: &[(&str, u64)] = &[("someone", 9001), ("some.viewer", 42)];
    const PROFILES: &[(u64, &str)] = &[
        (9001, "someone"),
        (9002, "someone.else"),
        (42, "some.viewer"),
    ];

    /// A name never seen costs the route definitions and the profile query;
    /// one whose pk is known, the query alone; and one whose stored pk now
    /// answers to another name is looked up again. The profile accepted is
    /// opened with the three reads the app sends beside its query, and only
    /// that one: never the one that named somebody else.
    #[tokio::test]
    async fn a_profile_by_name_spends_one_read_once_its_pk_is_known() {
        use crate::client::harness::spending_client;

        for (known, spent) in [(None, 2 + 3), (Some(9001), 1 + 3), (Some(9002), 3 + 3)] {
            let page = building_instagram(ROUTES, PROFILES);
            let (client, budget) = spending_client(page.clone());
            let profile = client
                .profile_named("SomeOne", known.map(Pk::new))
                .await
                .unwrap();
            assert_eq!(profile.id, Pk::new(9001), "{known:?}");
            assert_eq!(profile.username, "someone");
            assert_eq!(profile.via, crate::model::Via::Graph);
            assert!(profile.counters_are_knowable());
            assert_eq!(profile.follower_count(), Some(10));
            assert_eq!(budget.reserved(), (spent, 0), "{known:?}");
            let asked = page.asked();
            assert_eq!(asked.len(), spent as usize, "{known:?}");
            assert!(
                asked.last().unwrap().referrer.ends_with("/SomeOne/"),
                "the query is made from the profile's page"
            );
            let names: Vec<String> = asked
                .iter()
                .filter_map(|request| {
                    url::form_urlencoded::parse(request.body.as_deref()?.as_bytes())
                        .find(|(n, _)| n == "fb_api_req_friendly_name")
                        .map(|(_, v)| v.into_owned())
                })
                .collect();
            assert_eq!(
                names[names.len() - 4..],
                [
                    "PolarisProfilePageContentQuery",
                    "PolarisProfileStoryHighlightsTrayContentQuery",
                    "PolarisProfileNoteBubbleQuery",
                    "PolarisSchoolPartnerProfileBadgeQuery",
                ],
                "{known:?}"
            );
            let burst: Vec<&str> = asked[asked.len() - 3..]
                .iter()
                .map(|request| request.body.as_deref().unwrap())
                .collect();
            assert!(
                burst[0].contains("%22user_id%22%3A%229001%22"),
                "{}",
                burst[0]
            );
            assert!(
                burst[1].contains("%22user_id%22%3A%229001%22"),
                "{}",
                burst[1]
            );
            assert!(burst[2].contains("%22igid%22%3A%229001%22"), "{}", burst[2]);
        }
    }

    /// A fake Instagram behind a page that builds each ask as the tab does:
    /// every list answers one page of two accounts, `show_many` answers as
    /// `statuses` says, and a navigation answers its route.
    fn building_lists(statuses: u16) -> Arc<crate::client::harness::Scripted> {
        use crate::client::harness::{Scripted, page_said};
        Scripted::building(move |request| {
            Ok(if request.url.ends_with(crate::web::SHOW_MANY) {
                page_said(statuses, r#"{"friendship_statuses":{},"status":"ok"}"#)
            } else if request.url.ends_with(crate::web::NAVIGATION) {
                page_said(200, r#"for (;;);{"payload":{"payload":{"error":false}}}"#)
            } else {
                page_said(
                    200,
                    r#"{"users":[{"pk":"11","username":"a"},{"pk":12,"username":"b"}],
                    "next_max_id":"12","big_list":true,"status":"ok"}"#,
                )
            })
        })
    }

    /// The token the app's REST POSTs carry, as one of them is taken in.
    fn a_rest_post_of_the_apps(page: &crate::client::harness::Scripted) {
        let token = "rest:token";
        let form = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                ("user_ids", "1"),
                ("jazoest", crate::graphql::jazoest(token).as_str()),
                ("fb_dtsg", token),
            ])
            .finish();
        page.app_saw("/api/v1/news/inbox/", &form);
    }

    /// From the browser each list page is followed by the app's `show_many`
    /// about its accounts, before anything else is asked, from the list's
    /// page, paid as a read: when the app has sent the token it goes with.
    #[tokio::test]
    async fn a_list_page_is_followed_by_show_many_about_its_accounts() {
        use crate::client::harness::spending_client;

        let page = building_lists(200);
        a_rest_post_of_the_apps(&page);
        let (client, budget) = spending_client(page.clone());
        client
            .friendships_page(Pk::new(7), "someone", Direction::Followers, 12, None)
            .await
            .unwrap();
        client
            .mutual_followers_page(Pk::new(7), "someone", None)
            .await
            .unwrap();
        assert_eq!(budget.reserved(), (4, 0), "two pages, two show_many");
        let sent: Vec<(String, String)> = page
            .asked()
            .into_iter()
            .map(|r| {
                let path = url::Url::parse(&r.url).unwrap().path().to_string();
                (path, r.referrer)
            })
            .collect();
        assert_eq!(
            sent,
            [
                (
                    "/api/v1/friendships/7/followers/".to_string(),
                    "http://127.0.0.1:9/someone/".to_string()
                ),
                (
                    "/api/v1/friendships/show_many/".into(),
                    "http://127.0.0.1:9/someone/".into()
                ),
                (
                    "/api/v1/friendships/7/mutual_followers/".into(),
                    "http://127.0.0.1:9/someone/followers/mutualOnly".into()
                ),
                (
                    "/api/v1/friendships/show_many/".into(),
                    "http://127.0.0.1:9/someone/followers/mutualOnly".into()
                ),
            ]
        );
        let body = page.asked()[1].body.clone().unwrap();
        assert!(body.starts_with("user_ids=11%2C12&jazoest="), "{body}");
        assert!(page.asks().iter().any(|call| matches!(
            &call.ask,
            Ask::Statuses { pks } if *pks == [Pk::new(11), Pk::new(12)]
        )));
    }

    /// Without the token the app's REST POSTs carry, `show_many` is not
    /// built; the tab is asked once, and never again by the same client.
    #[tokio::test]
    async fn show_many_without_the_apps_token_is_asked_once_and_never_sent() {
        use crate::client::harness::spending_client;

        let page = building_lists(200);
        let (client, budget) = spending_client(page.clone());
        for _ in 0..3 {
            client
                .friendships_page(Pk::new(7), "someone", Direction::Following, 12, None)
                .await
                .unwrap();
        }
        let statuses = page
            .asks()
            .iter()
            .filter(|call| matches!(call.ask, Ask::Statuses { .. }))
            .count();
        assert_eq!(statuses, 1);
        assert_eq!(page.asked().len(), 3, "the three pages and nothing else");
        assert_eq!(budget.reserved(), (4, 0));
    }

    /// A `show_many` that fails without a push-back is passed over, and the
    /// page it followed is the answer; a push-back on it is one on the walk.
    #[tokio::test]
    async fn a_failed_show_many_is_passed_over_unless_it_pushes_back() {
        use crate::client::harness::spending_client;

        let page = building_lists(500);
        a_rest_post_of_the_apps(&page);
        let (client, _) = spending_client(page);
        let listed = client
            .friendships_page(Pk::new(7), "someone", Direction::Followers, 12, None)
            .await
            .unwrap();
        assert_eq!(listed.users.len(), 2);

        let page = building_lists(429);
        a_rest_post_of_the_apps(&page);
        let (client, _) = spending_client(page);
        let error = client
            .friendships_page(Pk::new(7), "someone", Direction::Followers, 12, None)
            .await
            .unwrap_err();
        assert!(matches!(error, IgError::RateLimited), "{error:?}");
    }

    /// A list is opened with the app's navigation to the profile, or to its
    /// mutual tab, from the route itself: one read, and nothing for a name
    /// that cannot be a profile's or without a browser.
    #[tokio::test]
    async fn a_list_is_opened_with_the_apps_navigation() {
        use crate::client::harness::spending_client;

        let page = building_lists(200);
        let (client, budget) = spending_client(page.clone());
        client.open_list("someone", false).await.unwrap();
        client.open_list("someone", true).await.unwrap();
        client.open_list("", false).await.unwrap();
        client.open_list("not/a name", true).await.unwrap();
        assert_eq!(budget.reserved(), (2, 0));
        let routes: Vec<(String, String)> = page
            .asked()
            .into_iter()
            .map(|r| {
                let route = url::form_urlencoded::parse(r.body.as_deref().unwrap().as_bytes())
                    .find(|(n, _)| n == "route_url")
                    .map(|(_, v)| v.into_owned())
                    .unwrap();
                (route, r.referrer)
            })
            .collect();
        assert_eq!(
            routes,
            [
                (
                    "/someone/".to_string(),
                    "http://127.0.0.1:9/someone/".to_string()
                ),
                (
                    "/someone/followers/mutualOnly".into(),
                    "http://127.0.0.1:9/someone/followers/mutualOnly".into()
                ),
            ]
        );

        let server = wiremock::MockServer::start().await;
        let rest = crate::client::harness::client(&server).await;
        rest.open_list("someone", false).await.unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A walk from the browser goes as the app reads a list: the profile's
    /// navigation, then each page followed by its `show_many` before the
    /// next page is asked.
    #[tokio::test]
    async fn a_walk_opens_its_list_and_follows_each_page_with_show_many() {
        use crate::client::harness::spending_client;
        use crate::pager::{ListRequest, ListWalker, OverBudget};

        let page = building_lists(200);
        a_rest_post_of_the_apps(&page);
        let (client, _) = spending_client(page.clone());
        let request = ListRequest {
            pk: Pk::new(7),
            username: "someone",
            direction: Direction::Following,
            from: None,
            estimated: None,
            max_pages: Some(2),
            already_stored: 0,
            over_budget: OverBudget::Continue,
        };
        ListWalker::new(&client)
            .walk(request, |p, _| Ok(p.users.len()), |_| {})
            .await
            .unwrap();
        let asks: Vec<&str> = page
            .asks()
            .iter()
            .map(|call| match call.ask {
                Ask::Navigation { .. } => "navigation",
                Ask::Rest { .. } => "page",
                Ask::Statuses { .. } => "show_many",
                _ => "other",
            })
            .collect();
        assert_eq!(
            asks,
            ["navigation", "page", "show_many", "page", "show_many"]
        );
    }

    /// The tray a profile was opened with is the one the command that shows
    /// it takes, for nothing, once and within the action; a second ask, or
    /// one after another action began, is a read again.
    #[tokio::test]
    async fn the_tray_a_profile_opened_with_is_not_asked_again() {
        use crate::client::harness::spending_client;

        let (client, budget) = spending_client(building_instagram(ROUTES, PROFILES));
        let profile = client
            .profile_named("someone", Some(Pk::new(9001)))
            .await
            .unwrap();
        assert_eq!(budget.reserved(), (4, 0));
        let tray = client.highlights_tray(profile.id, "someone").await.unwrap();
        assert!(tray.is_empty());
        assert_eq!(budget.reserved(), (4, 0), "the held tray costs nothing");
        client.highlights_tray(profile.id, "someone").await.unwrap();
        assert_eq!(budget.reserved(), (5, 0), "and is taken once");

        client
            .profile_named("someone", Some(Pk::new(9001)))
            .await
            .unwrap();
        client.pacer().begin_action();
        client
            .highlights_tray(Pk::new(9001), "someone")
            .await
            .unwrap();
        assert_eq!(budget.reserved(), (10, 0), "another action reads it again");
    }

    /// A read of the burst that fails without a push-back is passed over:
    /// the profile is the answer. A push-back on one stops it all.
    #[tokio::test]
    async fn a_burst_read_that_fails_is_passed_over_unless_it_pushes_back() {
        use crate::client::harness::{page_said, spending_client};

        for (status, body, stops) in [
            (500, "oops", false),
            (200, r#"{"errors":[{"message":"x"}]}"#, false),
            (429, "", true),
        ] {
            let page = crate::client::harness::Scripted::building(move |request| {
                if request
                    .body
                    .as_deref()
                    .is_some_and(|b| b.contains("PolarisProfileNoteBubbleQuery"))
                {
                    return Ok(page_said(status, body));
                }
                instagram_answer(ROUTES, PROFILES, request)
            });
            let (client, _) = spending_client(page);
            let opened = client.profile_named("someone", Some(Pk::new(9001))).await;
            if stops {
                assert!(matches!(opened, Err(IgError::RateLimited)), "{opened:?}");
            } else {
                assert_eq!(opened.unwrap().id, Pk::new(9001), "{status}");
            }
        }
    }

    /// A route that is no profile is a missing account; one that answers
    /// with an error is not; a name that cannot be anybody's is missing
    /// without a request.
    #[tokio::test]
    async fn a_name_that_is_nobodys_is_not_found_and_an_error_is_not_that() {
        use crate::client::harness::spending_client;

        let page = building_instagram(ROUTES, PROFILES);
        let (client, budget) = spending_client(page.clone());
        let error = client.profile_named("nobody", None).await.unwrap_err();
        assert!(matches!(error, IgError::NotFound { .. }), "{error:?}");
        assert_eq!(budget.reserved(), (1, 0));

        let error = client.profile_named("broken", None).await.unwrap_err();
        assert!(matches!(error, IgError::Unexpected { .. }), "{error:?}");

        let asked = page.asks().len();
        let error = client.profile_named("not/a name", None).await.unwrap_err();
        assert!(matches!(error, IgError::NotFound { .. }), "{error:?}");
        assert_eq!(page.asks().len(), asked, "nothing was asked");
        assert_eq!(budget.reserved(), (2, 0));
    }

    /// A name whose profile document is a 404, which the tab loads when the
    /// app made no route call to copy, is not found by that name.
    #[tokio::test]
    async fn a_name_whose_profile_is_a_404_is_named_in_the_error() {
        use crate::client::harness::{Scripted, page_said, spending_client};

        let page = Scripted::telling(|_| {
            Ok(Told::Pk {
                answer: page_said(404, ""),
                pk: RouteAnswer::Error,
            })
        });
        let (client, _) = spending_client(page);
        let error = client.profile_named("gone.name", None).await.unwrap_err();
        assert!(
            matches!(&error, IgError::NotFound { what: Some(name) } if name == "gone.name"),
            "{error:?}"
        );
        assert!(error.to_string().contains("gone.name"), "{error}");
    }

    /// The viewer's own profile carries no relationship with itself.
    #[tokio::test]
    async fn the_own_profile_has_no_relationship_flags() {
        use crate::client::harness::spending_client;

        let (client, _) = spending_client(building_instagram(ROUTES, PROFILES));
        let own = client.profile_named("some.viewer", None).await.unwrap();
        assert_eq!(own.id, Pk::new(42));
        assert_eq!(own.followed_by_viewer, None);
        assert_eq!(own.follows_viewer, None);
        assert_eq!(own.requested_by_viewer, None);

        let other = client.profile_named("someone", None).await.unwrap();
        assert_eq!(other.follows_viewer, Some(true));
        assert_eq!(other.followed_by_viewer, Some(false));
    }

    /// Another account's name, from the browser, is its profile's.
    #[tokio::test]
    async fn another_accounts_name_is_read_from_its_profile() {
        use crate::client::harness::spending_client;

        let (client, budget) = spending_client(building_instagram(ROUTES, PROFILES));
        let name = client.resolve_username(Pk::new(9002)).await.unwrap();
        assert_eq!(name.as_deref(), Some("someone.else"));
        assert_eq!(budget.reserved(), (1, 0));
    }

    /// From the browser the counters are the hover card's, asked by pk from
    /// the home page with no name needed: one read. A card that names nobody
    /// is a missing account.
    #[tokio::test]
    async fn the_counters_are_the_hover_cards_by_pk() {
        use crate::client::harness::spending_client;

        let page = building_instagram(ROUTES, PROFILES);
        let (client, budget) = spending_client(page.clone());
        let counters = client.counters(Pk::new(42), None).await.unwrap();
        assert_eq!(
            counters,
            Some(Counters {
                followers: Some(10),
                following: Some(20),
            })
        );
        assert_eq!(budget.reserved(), (1, 0));
        let asked = page.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].referrer, "http://127.0.0.1:9/");
        let form: Vec<(String, String)> =
            url::form_urlencoded::parse(asked[0].body.as_deref().unwrap().as_bytes())
                .into_owned()
                .collect();
        let field = |name: &str| {
            form.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            field("fb_api_req_friendly_name"),
            Some("PolarisUserHoverCardContentV2Query")
        );
        assert_eq!(field("variables"), Some(r#"{"userID":"42"}"#));

        let error = client.counters(Pk::new(7), Some("gone")).await.unwrap_err();
        assert!(
            matches!(&error, IgError::NotFound { what: Some(name) } if name == "gone"),
            "{error:?}"
        );
    }

    /// A fake Instagram behind a page that builds each ask as the tab does,
    /// answering the highlights tray of 9001 with two highlights, and the
    /// highlights page with a reel for each id it is asked, in its order.
    fn building_highlights() -> Arc<crate::client::harness::Scripted> {
        use crate::client::harness::{Scripted, page_said};
        Scripted::building(|request| {
            let form: Vec<(String, String)> =
                url::form_urlencoded::parse(request.body.as_deref().unwrap_or_default().as_bytes())
                    .into_owned()
                    .collect();
            let field = |name: &str| {
                form.iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            let variables: serde_json::Value = serde_json::from_str(&field("variables")).unwrap();
            if field("fb_api_req_friendly_name") == "PolarisProfileStoryHighlightsTrayContentQuery"
            {
                assert_eq!(variables, serde_json::json!({ "user_id": "9001" }));
                return Ok(page_said(
                    200,
                    r#"{"data":{"highlights":{"edges":[
                    {"node":{"id":"highlight:17900000000000001","title":"trip",
                    "cover_media":{"cropped_image_version":{"url":"https://cdn.test/1.jpg"}},
                    "user":{"pk":"9001"}}},
                    {"node":{"id":"highlight:17900000000000002","title":"home",
                    "cover_media":null,"user":{"pk":"9001"}}}],
                    "page_info":{"has_next_page":false,"end_cursor":null}}}}"#,
                ));
            }
            let edges: Vec<String> = variables["reel_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| {
                    let id = id.as_str().unwrap();
                    format!(
                        r#"{{"node":{{"id":"{id}","items":[{{"pk":"{}","media_type":1,
                        "taken_at":1700000000,"expiring_at":1700086400}}]}}}}"#,
                        id.trim_start_matches("highlight:")
                    )
                })
                .collect();
            Ok(page_said(
                200,
                &format!(
                    r#"{{"data":{{"xdt_api__v1__feed__reels_media__connection":{{"edges":[{}],
                    "page_info":{{"has_next_page":false}}}}}}}}"#,
                    edges.join(",")
                ),
            ))
        })
    }

    /// From the browser the tray is the web client's tray query, by pk, made
    /// from the account's page: one read, whose highlights have a title and
    /// a cover and no count or dates.
    #[tokio::test]
    async fn with_a_page_the_tray_is_the_tray_query_and_knows_no_counts_or_dates() {
        use crate::client::harness::spending_client;

        let page = building_highlights();
        let (client, budget) = spending_client(page.clone());
        let tray = client
            .highlights_tray(Pk::new(9001), "someone")
            .await
            .unwrap();
        assert_eq!(budget.reserved(), (1, 0));
        let ids: Vec<&str> = tray.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            ["highlight:17900000000000001", "highlight:17900000000000002"]
        );
        assert_eq!(tray[0].title.as_deref(), Some("trip"));
        assert!(
            tray[0]
                .cover_media
                .as_ref()
                .is_some_and(|c| c.cropped_image_version.is_some())
        );
        for highlight in &tray {
            assert_eq!(highlight.media_count, None);
            assert_eq!(highlight.created_at, None);
            assert_eq!(highlight.updated_timestamp, None);
        }
        let asked = page.asked();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].url, "http://127.0.0.1:9/api/graphql");
        assert_eq!(asked[0].referrer, "http://127.0.0.1:9/someone/");
    }

    /// From the browser a highlight's items are the web client's highlights
    /// query: opened at the highlight, with every id of the tray in its
    /// order, three after and two before, on `/graphql/query` with the root
    /// field and the Bloks version, from the account's page. The reel kept
    /// is the one asked for, and its items do not expire.
    #[tokio::test]
    async fn with_a_page_a_highlight_is_the_highlights_query_over_the_whole_tray() {
        use crate::client::harness::spending_client;

        let page = building_highlights();
        let (client, budget) = spending_client(page.clone());
        let tray = [
            "highlight:17900000000000001".to_string(),
            "highlight:17900000000000002".to_string(),
            "highlight:17900000000000003".to_string(),
        ];
        let reel = client
            .highlight("highlight:17900000000000002", &tray, "someone")
            .await
            .unwrap()
            .expect("the highlight has an item");
        assert_eq!(reel.id.as_deref(), Some("highlight:17900000000000002"));
        assert_eq!(reel.items.len(), 1);
        assert_eq!(reel.items[0].pk, "17900000000000002");
        assert_eq!(reel.items[0].expiring_at, None);
        assert_eq!(budget.reserved(), (1, 0));

        let asked = page.asked();
        assert_eq!(asked.len(), 1, "the query alone, and no reels_media");
        let sent = &asked[0];
        assert_eq!(sent.url, "http://127.0.0.1:9/graphql/query");
        assert_eq!(sent.referrer, "http://127.0.0.1:9/someone/");
        let header = |name: &str| {
            sent.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            header("x-root-field-name"),
            Some("xdt_api__v1__feed__reels_media__connection")
        );
        assert_eq!(
            header("x-bloks-version-id"),
            Some("0123456789abcdef".repeat(4).as_str())
        );
        let form: Vec<(String, String)> =
            url::form_urlencoded::parse(sent.body.as_deref().unwrap().as_bytes())
                .into_owned()
                .collect();
        let field = |name: &str| {
            form.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            field("fb_api_req_friendly_name"),
            Some("PolarisStoriesV3HighlightsPageQuery")
        );
        assert_eq!(field("doc_id"), Some("28325328583775973"));
        assert_eq!(
            field("variables"),
            Some(
                r#"{"initial_reel_id":"highlight:17900000000000002","reel_ids":["highlight:17900000000000001","highlight:17900000000000002","highlight:17900000000000003"],"first":3,"last":2,"__relay_internal__pv__PolarisCommunityNoteStoriesLabelEnabledrelayprovider":true}"#
            )
        );

        // An id the answer does not carry is a highlight with nothing in it.
        let gone = client
            .highlight("highlight:17900000000000009", &tray, "someone")
            .await
            .unwrap();
        assert!(gone.is_none());
    }

    /// A highlights query answered under a root field other than the one
    /// it was sent with does not read as an empty highlight: it fails, and
    /// the error names the field Instagram answered under.
    #[tokio::test]
    async fn a_highlight_answered_under_another_root_field_does_not_decode() {
        use crate::client::harness::{Scripted, page_said, spending_client};

        let page = Scripted::building(|_| {
            Ok(page_said(
                200,
                r#"{"data":{"xdt_api__v1__feed__reels_media":{"reels_media":[{"id":"highlight:17900000000000002","items":[]}]}}}"#,
            ))
        });
        let (client, _) = spending_client(page);
        let tray = ["highlight:17900000000000002".to_string()];
        let error = client
            .highlight("highlight:17900000000000002", &tray, "someone")
            .await
            .expect_err("an answer under another field is not an empty highlight");
        assert!(matches!(error, IgError::Decode(_)), "{error:?}");
        let message = error.to_string();
        assert!(
            message.contains("xdt_api__v1__feed__reels_media\"")
                && message.contains("xdt_api__v1__feed__reels_media__connection"),
            "{message}"
        );
        assert!(
            !message.contains("17900000000000002"),
            "never a value: {message}"
        );
    }

    /// A fake Instagram behind a page showing the stories tray `tray`, that
    /// answers the gallery with a reel of one story for each id it is asked,
    /// in its order, the story tagging `tagged` the way the gallery tags.
    fn building_gallery(tray: Option<&[&str]>) -> Arc<crate::client::harness::Scripted> {
        use crate::client::harness::{Scripted, page_said};
        Scripted::building_under(tray, |request| {
            let form: Vec<(String, String)> =
                url::form_urlencoded::parse(request.body.as_deref().unwrap_or_default().as_bytes())
                    .into_owned()
                    .collect();
            let variables = form
                .iter()
                .find(|(n, _)| n == "variables")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let variables: serde_json::Value = serde_json::from_str(&variables).unwrap();
            let edges: Vec<String> = variables["reel_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| {
                    let id = id.as_str().unwrap();
                    format!(
                        r#"{{"node":{{"id":"{id}","user":{{"pk":"{id}","username":"user{id}"}},"items":[{{"pk":"3{id}",
                        "media_type":1,"taken_at":1700000000,"expiring_at":1700086400,
                        "story_bloks_stickers":[{{"bloks_sticker":{{"sticker_data":
                        {{"ig_mention":{{"username":"tagged"}}}}}}}}]}}]}}}}"#
                    )
                })
                .collect();
            Ok(page_said(
                200,
                &format!(
                    r#"{{"data":{{"xdt_api__v1__feed__reels_media__connection":{{"edges":[{}],
                    "page_info":{{"has_next_page":false}}}}}}}}"#,
                    edges.join(",")
                ),
            ))
        })
    }

    /// The variables of the one query `page` was asked to send.
    fn variables_sent(page: &crate::client::harness::Scripted) -> String {
        let asked = page.asked();
        assert_eq!(asked.len(), 1, "one query and nothing else");
        url::form_urlencoded::parse(asked[0].body.as_deref().unwrap().as_bytes())
            .find(|(n, _)| n == "variables")
            .map(|(_, v)| v.into_owned())
            .unwrap()
    }

    /// From the browser an account's stories are the web client's gallery,
    /// on `/graphql/query` with its root field, from the home page: opened at
    /// the account, over the tray the home page shows in its order when the
    /// account is in it, and over the account alone when it is not. The
    /// tray costs nothing; the gallery is one read; the reel kept is the
    /// account's, its mentions read from the gallery's stickers.
    #[tokio::test]
    async fn with_a_page_stories_are_the_gallery_over_the_trays_reels() {
        use crate::client::harness::spending_client;

        let page = building_gallery(Some(&["7", "9001", "8"]));
        let (client, budget) = spending_client(page.clone());
        let reel = client
            .stories(Pk::new(9001), "someone")
            .await
            .unwrap()
            .expect("the account has a story up");
        assert_eq!(reel.id.as_deref(), Some("9001"));
        assert_eq!(reel.items.len(), 1);
        assert_eq!(reel.items[0].pk, "39001");
        assert_eq!(reel.items[0].mentioned().collect::<Vec<_>>(), ["tagged"]);
        assert_eq!(budget.reserved(), (1, 0), "the tray is free");
        assert_eq!(
            variables_sent(&page),
            r#"{"initial_reel_id":"9001","reel_ids":["7","9001","8"],"first":3,"last":2,"__relay_internal__pv__PolarisCommunityNoteStoriesLabelEnabledrelayprovider":true}"#
        );
        let sent = &page.asked()[0];
        assert_eq!(sent.url, "http://127.0.0.1:9/graphql/query");
        assert_eq!(sent.referrer, "http://127.0.0.1:9/");
        assert!(
            sent.headers.iter().any(|(n, v)| n == "X-Root-Field-Name"
                && v == "xdt_api__v1__feed__reels_media__connection"),
            "{:?}",
            sent.headers.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        let form: Vec<(String, String)> =
            url::form_urlencoded::parse(sent.body.as_deref().unwrap().as_bytes())
                .into_owned()
                .collect();
        assert!(form.contains(&(
            "fb_api_req_friendly_name".into(),
            "PolarisStoriesV3ReelPageGalleryQuery".into()
        )));
        assert!(form.contains(&("doc_id".into(), "29088280257466613".into())));

        for tray in [Some(&["7", "8"][..]), None] {
            let page = building_gallery(tray);
            let (client, _) = spending_client(page.clone());
            let reel = client.stories(Pk::new(9001), "someone").await.unwrap();
            assert!(reel.is_some(), "{tray:?}");
            assert_eq!(
                variables_sent(&page),
                r#"{"initial_reel_id":"9001","reel_ids":["9001"],"first":3,"last":2,"__relay_internal__pv__PolarisCommunityNoteStoriesLabelEnabledrelayprovider":true}"#,
                "{tray:?}"
            );
        }
    }

    /// Without a browser the counters are the profile's, by name, and with
    /// no name there is nothing to ask with.
    #[tokio::test]
    async fn without_a_page_the_counters_need_a_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/users/web_profile_info/"))
            .and(query_param("username", "someone"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"data":{"user":{"id":"9001","username":"someone",
                "edge_followed_by":{"count":25},"edge_follow":{"count":11}}}}"#,
            ))
            .mount(&server)
            .await;

        let client = client(&server).await;
        assert_eq!(client.counters(Pk::new(9001), None).await.unwrap(), None);
        assert!(server.received_requests().await.unwrap().is_empty());
        let counters = client
            .counters(Pk::new(9001), Some("someone"))
            .await
            .unwrap();
        assert_eq!(
            counters,
            Some(Counters {
                followers: Some(25),
                following: Some(11),
            })
        );
    }
}
