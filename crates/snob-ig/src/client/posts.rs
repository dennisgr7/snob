//! A profile's posts, one post, its comments, replies and likers, read the way
//! the recorded web app reads them.
//!
//! - **The grid**: the profile's posts query and its connection, twelve posts
//!   a page, by name from the profile's page.
//! - **The reels, tagged and reposts tabs** ([`ProfileTab`]): a query each and
//!   its connection, by pk; the first page from the profile's page, then the
//!   app's navigation to `/<name>/<tab>/`, then the next pages from there
//!   (captures of 2026-11-13 and 2026-11-14). **Their numbers are seeds**
//!   (`Operation::doc_id`), never vouched for.
//! - **A post**: `media/<pk>/info/`, made from the page it opens on. The page
//!   itself is never loaded: a reel's link lands on the reels screen, which
//!   reports what it plays.
//! - **Comments, replies and likers**: `media/<pk>/comments/`, `.../<comment_pk>/child_comments/`
//!   (the first page alone) and `media/<pk>/likers/`, each a list of other
//!   people's accounts and charged to the day's accounts. The app's
//!   `show_many` after the likers is not sent.
//!
//! A page of a grid or a tab is one read, counted in the sitting and not
//! charged to the day's accounts. **None of it tells anybody what was looked
//! at**: the app's view reports are calls the allowlist refuses and
//! `no_seen.rs` keeps out of the source.

use crate::allowlist::{Operation, Rest};
use crate::error::IgError;
use crate::model::post::{
    CommentsPage, LikersPage, Media, MediaInfo, ReelsTabPage, RepliesPage, RepostsTabPage,
    TaggedTabPage, TimelinePage, comments_cursor_has_its_shape, reels_cursor_has_its_shape,
    tagged_cursor_has_its_shape,
};
use crate::shortcode::{CommentPk, MediaPk};
use crate::web::{Ask, Told};

use super::IgClient;
use super::ask::{answered, profile_path, told_otherwise};

#[derive(Debug, Clone)]
pub struct PostsPage {
    pub posts: Vec<Media>,
    /// `None` on the last page.
    pub next: Option<String>,
}

/// A tab of a profile besides its grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileTab {
    Reels,
    /// The posts other accounts tagged it in, each owned by its author.
    Tagged,
    /// What it reposted, each post owned by its author.
    Reposts,
}

impl ProfileTab {
    pub fn segment(self) -> &'static str {
        match self {
            Self::Reels => "reels",
            Self::Tagged => "tagged",
            Self::Reposts => "reposts",
        }
    }

    /// The query of its first page, and the one of its next pages.
    pub fn operations(self) -> (Operation, Operation) {
        match self {
            Self::Reels => (Operation::ProfileReels, Operation::ProfileReelsPage),
            Self::Tagged => (Operation::ProfileTagged, Operation::ProfileTaggedPage),
            Self::Reposts => (Operation::ProfileReposts, Operation::ProfileRepostsPage),
        }
    }

    /// Only a cursor of the tab's own shape goes back to Instagram.
    fn takes(self, cursor: &str) -> bool {
        match self {
            Self::Reels | Self::Reposts => reels_cursor_has_its_shape(cursor),
            Self::Tagged => tagged_cursor_has_its_shape(cursor),
        }
    }
}

impl IgClient {
    /// A page of `username`'s posts: the first when `after` is `None`. One read.
    ///
    /// **From the browser only**: there is no capture of a REST read of the
    /// grid to send without one, and none is invented.
    pub async fn profile_posts(
        &self,
        username: &str,
        after: Option<&str>,
    ) -> Result<PostsPage, IgError> {
        if !self.has_page() {
            return Err(IgError::Browser(
                "a profile's posts are read from the browser, and SNOB_NO_BROWSER has none".into(),
            ));
        }
        let (operation, variables) = match after {
            None => (
                Operation::ProfilePosts,
                crate::web::posts_variables(username),
            ),
            Some(after) => (
                Operation::ProfilePostsPage,
                crate::web::posts_page_variables(username, after),
            ),
        };
        let ask = Ask::Query {
            operation,
            variables,
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, &profile_path(username)).await? else {
            return Err(told_otherwise("a profile's posts"));
        };
        let page: TimelinePage =
            self.decode_query(&answered(&endpoint, answer), operation.root_field())?;
        let (posts, next) = page.posts();
        self.count_in_the_sitting().await?;
        Ok(PostsPage { posts, next })
    }

    /// A page of `tab` of the account `pk`: the first when `after` is `None`,
    /// made from the profile's page; the next ones from the tab's page, which
    /// [`Self::open_tab`] navigates to first. One read, from the browser only.
    /// A cursor not of the tab's shape is refused with nothing sent.
    pub async fn profile_tab(
        &self,
        tab: ProfileTab,
        pk: snob_core::Pk,
        username: &str,
        after: Option<&str>,
    ) -> Result<PostsPage, IgError> {
        if !self.has_page() {
            return Err(IgError::Browser(
                "a profile's tabs are read from the browser, and SNOB_NO_BROWSER has none".into(),
            ));
        }
        if after.is_some_and(|after| !tab.takes(after)) {
            return Err(IgError::NotAllowed(
                "a tab's page after a cursor Instagram never handed out".into(),
            ));
        }
        let (first, next) = tab.operations();
        let (operation, variables, from) = match (tab, after) {
            (ProfileTab::Reels, None) => (first, crate::web::reels_variables(pk), None),
            (ProfileTab::Tagged, None) => (first, crate::web::tagged_variables(pk), None),
            (ProfileTab::Reposts, None) => (first, crate::web::reposts_variables(pk), None),
            (ProfileTab::Reels, Some(after)) => {
                (next, crate::web::reels_page_variables(pk, after), Some(tab))
            }
            (ProfileTab::Tagged, Some(after)) => (
                next,
                crate::web::tagged_page_variables(pk, after),
                Some(tab),
            ),
            (ProfileTab::Reposts, Some(after)) => (
                next,
                crate::web::reposts_page_variables(pk, after),
                Some(tab),
            ),
        };
        let referrer = match from {
            Some(tab) => tab_path(username, tab),
            None => profile_path(username),
        };
        let ask = Ask::Query {
            operation,
            variables,
        };
        let endpoint = ask.endpoint();
        let Told::Answer(answer) = self.ask_page(ask, &referrer).await? else {
            return Err(told_otherwise("a profile's tab"));
        };
        let answer = answered(&endpoint, answer);
        let root = operation.root_field();
        let (posts, next) = match tab {
            ProfileTab::Reels => self.decode_query::<ReelsTabPage>(&answer, root)?.posts(),
            ProfileTab::Tagged => self.decode_query::<TaggedTabPage>(&answer, root)?.posts(),
            ProfileTab::Reposts => self.decode_query::<RepostsTabPage>(&answer, root)?.posts(),
        };
        self.count_in_the_sitting().await?;
        Ok(PostsPage { posts, next })
    }

    /// The app's navigation to `/<name>/<tab>/` as the tab is clicked (6 of 6
    /// in the capture of 2026-11-14). One read, from the browser only; a
    /// failure is treated as `IgClient::navigation` treats one.
    pub async fn open_tab(&self, username: &str, tab: ProfileTab) -> Result<(), IgError> {
        if !self.has_page() || !crate::allowlist::profile_name(username) {
            return Ok(());
        }
        self.navigation(&tab_path(username, tab)).await
    }

    /// Counts a page of a grid or of comments in the account's sitting, as a
    /// list page is (`Pacer::page_walked`). The rest a full sitting owes is
    /// paid before the next page (`IgClient::rest_before_a_page`), not here.
    async fn count_in_the_sitting(&self) -> Result<(), IgError> {
        self.pacer
            .page_walked(&crate::pace::Pace::default(), snob_core::clock::now_ms())
            .await
            .map(|_| ())
    }

    /// The post `pk`, asked from the page it opens on (`shortcode::page_of`).
    /// `None` when the answer holds no post: deleted, or not shown to this
    /// account.
    pub async fn media_info(&self, pk: MediaPk, page: &str) -> Result<Option<Media>, IgError> {
        let info: MediaInfo = self.rest_read(Rest::MediaInfo(pk), &[], page).await?;
        Ok(info.items.into_iter().next())
    }

    /// A page of the comments of post `pk`: the first when `after` is `None`,
    /// otherwise the one the page before named in its `next_min_id`.
    pub async fn media_comments(
        &self,
        pk: MediaPk,
        after: Option<&str>,
        page: &str,
    ) -> Result<CommentsPage, IgError> {
        let read = Rest::Comments(pk);
        // Only a cursor of the shape Instagram hands out goes back to it.
        if after.is_some_and(|after| !comments_cursor_has_its_shape(after)) {
            return Err(IgError::NotAllowed(
                "a page of comments after a cursor Instagram never handed out".into(),
            ));
        }
        // In the app's order (capture of 2026-10-01).
        let query: Vec<(&str, &str)> = match after {
            None => vec![
                ("can_support_threading", "true"),
                ("permalink_enabled", "false"),
            ],
            Some(after) => vec![
                ("can_support_threading", "true"),
                ("min_id", after),
                ("sort_order", "popular"),
            ],
        };
        let comments: CommentsPage = self.rest_read(read, &query, page).await?;
        self.charge_accounts(comments.accounts()).await;
        self.count_in_the_sitting().await?;
        Ok(comments)
    }
}

impl IgClient {
    /// The replies under `comment` of post `pk`, asked from the post's own
    /// page (`/p/<code>/` for a reel too: 3 of 3 in the capture of 2026-11-14).
    ///
    /// **The first page only, with an empty cursor**, the three keys in the
    /// app's order (7 calls of 7 over the captures of 2026-11-13 and
    /// 2026-11-14). Whether the thread holds more is [`RepliesPage::more`]'s
    /// to say.
    pub async fn comment_replies(
        &self,
        pk: MediaPk,
        comment: CommentPk,
        page: &str,
    ) -> Result<RepliesPage, IgError> {
        let read = Rest::Replies(pk, comment);
        let query = [
            ("min_id", ""),
            ("is_chronological", "true"),
            ("paging_direction", "view_more"),
        ];
        let replies: RepliesPage = self.rest_read(read, &query, page).await?;
        self.charge_accounts(replies.accounts()).await;
        self.count_in_the_sitting().await?;
        Ok(replies)
    }

    /// The accounts that liked post `pk`, as many as the app is shown, asked
    /// from the post's page (`/p/<code>/` both times in the capture of
    /// 2026-11-13). **Only after the caller asked for consent.**
    pub async fn media_likers(&self, pk: MediaPk, page: &str) -> Result<LikersPage, IgError> {
        let likers: LikersPage = self.rest_read(Rest::Likers(pk), &[], page).await?;
        self.charge_accounts(likers.users.len()).await;
        Ok(likers)
    }
}

fn tab_path(name: &str, tab: ProfileTab) -> String {
    format!("{}{}/", profile_path(name), tab.segment())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::client::harness::{NOWHERE, Scripted, page_said, spending_client};
    use crate::client::page::{Page, PageRequest};

    const GRID: &str = r#"{"data":{"xdt_api__v1__feed__user_timeline_graphql_connection":{
        "edges":[{"node":{"pk":"3807824420233075826","code":"DTYHCKvDNxy","media_type":1}}],
        "page_info":{"end_cursor":"AQH-next","has_next_page":true}}}}"#;

    /// A form field of a request the page was asked to send.
    fn field(request: &PageRequest, name: &str) -> Option<String> {
        url::form_urlencoded::parse(request.body.as_deref()?.as_bytes())
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.into_owned())
    }

    /// The first page is the posts query, the next one its connection from
    /// the cursor, both made from the profile's page and built as the tab
    /// builds them, each paid as one read.
    #[tokio::test]
    async fn the_grid_is_the_posts_query_and_then_its_connection() {
        let tab = Scripted::building(|_| Ok(page_said(200, GRID)));
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let first = client.profile_posts("someone", None).await.unwrap();
        assert_eq!(first.posts.len(), 1);
        assert_eq!(first.next.as_deref(), Some("AQH-next"));
        client
            .profile_posts("someone", first.next.as_deref())
            .await
            .unwrap();

        let sent = tab.asked();
        let names: Vec<_> = sent
            .iter()
            .filter_map(|r| field(r, "fb_api_req_friendly_name"))
            .collect();
        assert_eq!(
            names,
            [
                "PolarisProfilePostsQuery",
                "PolarisProfilePostsTabContentQuery_connection"
            ]
        );
        // On `/api/graphql`, where the app has sent the grid since 2026-11-13,
        // and with no root-field header, which that endpoint never carries.
        assert!(sent.iter().all(|r| r.url.ends_with("/api/graphql")));
        assert!(sent.iter().all(|r| {
            !r.headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("X-Root-Field-Name"))
        }));
        assert!(sent.iter().all(|r| r.referrer.ends_with("/someone/")));
        let next = field(&sent[1], "variables").unwrap();
        assert!(next.starts_with(r#"{"after":"AQH-next""#), "{next}");
        assert_eq!(budget.reserved(), (2, 0));
    }

    /// A post and its comments are the two GETs the app sends as it opens,
    /// made from the page it opens on; the next page of comments carries the
    /// cursor the first handed out, and the order.
    #[tokio::test]
    async fn a_post_is_its_info_and_its_comments_from_its_own_page() {
        let tab = Scripted::building(|request| {
            let body = if request.url.contains("/info/") {
                r#"{"items":[{"pk":"3947056156557494178","code":"DbGwql4IMei","media_type":2}],"status":"ok"}"#
            } else if request.url.contains("min_id=") {
                r#"{"comments":[],"has_more_headload_comments":false,"status":"ok"}"#
            } else {
                r#"{"comments":[{"pk":"1","text":"hi","user":{"username":"a"}}],"has_more_headload_comments":true,"next_min_id":"{\"cached_comments_cursor\":\"1\"}","status":"ok"}"#
            };
            Ok(page_said(200, body))
        });
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let pk = MediaPk::new(3_947_056_156_557_494_178);
        let page = "/reel/DbGwql4IMei/";
        let post = client.media_info(pk, page).await.unwrap();
        assert_eq!(post.unwrap().code.as_deref(), Some("DbGwql4IMei"));
        let first = client.media_comments(pk, None, page).await.unwrap();
        let next = first.next().map(str::to_string);
        assert_eq!(next.as_deref(), Some(r#"{"cached_comments_cursor":"1"}"#));
        let last = client
            .media_comments(pk, next.as_deref(), page)
            .await
            .unwrap();
        assert_eq!(last.next(), None);

        let sent = tab.asked();
        let urls: Vec<String> = sent
            .iter()
            .map(|r| {
                r.url
                    .trim_start_matches(NOWHERE.trim_end_matches('/'))
                    .to_string()
            })
            .collect();
        assert_eq!(
            urls,
            [
                "/api/v1/media/3947056156557494178/info/".to_string(),
                "/api/v1/media/3947056156557494178/comments/?can_support_threading=true&permalink_enabled=false".to_string(),
                format!(
                    "/api/v1/media/3947056156557494178/comments/?can_support_threading=true&min_id={}&sort_order=popular",
                    "%7B%22cached_comments_cursor%22%3A%221%22%7D"
                ),
            ]
        );
        assert!(sent.iter().all(|r| r.referrer.ends_with(page)));
        assert_eq!(budget.reserved(), (3, 0));
    }

    /// A cursor of a shape Instagram never handed out is refused before it
    /// leaves: nothing is sent and nothing paid.
    #[tokio::test]
    async fn a_comments_cursor_of_another_shape_is_never_sent() {
        let tab = Scripted::building(|_| Ok(page_said(200, r#"{"comments":[],"status":"ok"}"#)));
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let pk = MediaPk::new(3_947_056_156_557_494_178);
        for after in [r#"{"c":"1"}"#, "1&sort_order=newest", ""] {
            let refused = client
                .media_comments(pk, Some(after), "/p/DbGwql4IMei/")
                .await;
            assert!(
                matches!(refused, Err(IgError::NotAllowed(_))),
                "{after}: {refused:?}"
            );
        }
        assert!(tab.asked().is_empty());
        assert_eq!(budget.reserved(), (0, 0));
    }

    /// The likers are one GET with no query, from the post's own page, paid
    /// as one read and charged to the day's accounts; nothing follows it,
    /// the app's `show_many` included.
    #[tokio::test]
    async fn the_likers_are_one_get_and_no_status_call_follows() {
        let tab = Scripted::building(|_| {
            Ok(page_said(
                200,
                r#"{"users":[{"pk":"11","username":"a"},{"pk":"12","username":"b"}],"user_count":900,"status":"ok"}"#,
            ))
        });
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let pk = MediaPk::new(3_947_056_156_557_494_178);
        let likers = client.media_likers(pk, "/p/DbGwql4IMei/").await.unwrap();
        assert_eq!(likers.accounts().len(), 2);
        let sent = tab.asked();
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0]
                .url
                .ends_with("/api/v1/media/3947056156557494178/likers/"),
            "{}",
            sent[0].url
        );
        assert!(sent[0].referrer.ends_with("/p/DbGwql4IMei/"));
        assert_eq!(budget.reserved(), (1, 0));
        assert_eq!(budget.accounts_spent(), 2);
    }

    /// A thread's replies are one GET under the comment, with the app's
    /// three keys and an empty cursor, from the post's page; paid as one
    /// read, its authors charged to the day's accounts, and counted in the
    /// sitting.
    #[tokio::test]
    async fn replies_are_one_get_under_the_comment() {
        let tab = Scripted::building(|_| {
            Ok(page_said(
                200,
                r#"{"child_comment_count":9,"child_comments":[
                    {"pk":"18000000000000001","text":"a","user":{"pk":42,"username":"one"}},
                    {"pk":"18000000000000002","text":"b","user":{"pk":"43","username":"two"}}],
                  "has_more_head_child_comments":false,"has_more_tail_child_comments":false,
                  "parent_comment":{"pk":"18000000000000000","media_id":"3947056156557494178"},
                  "status":"ok"}"#,
            ))
        });
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let pk = MediaPk::new(3_947_056_156_557_494_178);
        let comment = CommentPk::new(18_000_000_000_000_000);
        let replies = client
            .comment_replies(pk, comment, "/p/DbGwql4IMei/")
            .await
            .unwrap();
        assert_eq!(replies.child_comments.len(), 2);
        assert!(replies.is_on(pk));
        let sent = tab.asked();
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].url.ends_with(
                "/api/v1/media/3947056156557494178/comments/18000000000000000/child_comments/\
                 ?min_id=&is_chronological=true&paging_direction=view_more"
            ),
            "{}",
            sent[0].url
        );
        assert!(sent[0].referrer.ends_with("/p/DbGwql4IMei/"));
        assert_eq!(budget.reserved(), (1, 0));
        assert_eq!(budget.accounts_spent(), 2);
    }

    /// A tab is its first query by pk from the profile's page, then, after
    /// the navigation to the tab, its connection from the tab's page; each
    /// page one read.
    #[tokio::test]
    async fn a_tab_is_its_query_then_the_route_then_its_connection() {
        let reels = r#"{"data":{"fetch__XDTUserDict":{"clips_connection":{
            "edges":[{"node":{"media":{"pk":"3807824420233075826","code":"DTYHCKvDNxy",
              "product_type":"clips","user":{"username":"coauthor"}}}}],
            "page_info":{"end_cursor":"AQHT-next","has_next_page":true}}}}}"#;
        let tab = Scripted::building(move |request| {
            Ok(if request.url.ends_with("/ajax/navigation/") {
                page_said(200, r#"for (;;);{"payload":{}}"#)
            } else {
                page_said(200, reels)
            })
        });
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let pk = snob_core::Pk::new(2_345_678_901);
        let first = client
            .profile_tab(ProfileTab::Reels, pk, "someone", None)
            .await
            .unwrap();
        assert_eq!(first.posts.len(), 1);
        assert_eq!(first.posts[0].owner(), Some("coauthor"), "its own owner");
        client.open_tab("someone", ProfileTab::Reels).await.unwrap();
        client
            .profile_tab(ProfileTab::Reels, pk, "someone", first.next.as_deref())
            .await
            .unwrap();

        let sent = tab.asked();
        let names: Vec<_> = sent
            .iter()
            .map(|r| {
                field(r, "fb_api_req_friendly_name")
                    .unwrap_or_else(|| field(r, "route_url").unwrap_or_default())
            })
            .collect();
        assert_eq!(
            names,
            [
                "PolarisProfileReelsTabContentQuery",
                "/someone/reels/",
                "PolarisProfileReelsTabContentQuery_connection"
            ]
        );
        assert!(sent[0].referrer.ends_with("/someone/"));
        assert!(sent[2].referrer.ends_with("/someone/reels/"));
        let first_variables = field(&sent[0], "variables").unwrap();
        assert!(
            first_variables.contains(r#""user_id":"2345678901""#),
            "{first_variables}"
        );
        let next = field(&sent[2], "variables").unwrap();
        assert!(next.starts_with(r#"{"after":"AQHT-next""#), "{next}");
        assert_eq!(budget.reserved(), (3, 0));
    }

    /// A tab's cursor of a shape it never hands out is refused before it
    /// leaves, and nothing is paid.
    #[tokio::test]
    async fn a_tab_cursor_of_another_shape_is_never_sent() {
        let tab = Scripted::building(|_| Ok(page_said(200, "{}")));
        let (client, budget) = spending_client(Arc::clone(&tab) as Arc<dyn Page>);
        let pk = snob_core::Pk::new(1);
        for (which, after) in [
            (ProfileTab::Tagged, "None"),
            (ProfileTab::Tagged, "AQHT"),
            (ProfileTab::Reels, "a&b"),
            (ProfileTab::Reposts, ""),
        ] {
            let refused = client.profile_tab(which, pk, "someone", Some(after)).await;
            assert!(matches!(refused, Err(IgError::NotAllowed(_))), "{after}");
        }
        assert!(tab.asked().is_empty());
        assert_eq!(budget.reserved(), (0, 0));
    }

    /// Without a browser the grid is not read, and nothing is spent finding
    /// that out.
    #[tokio::test]
    async fn without_a_browser_the_grid_is_refused_for_nothing() {
        let server = wiremock::MockServer::start().await;
        let client = crate::client::harness::client_with(&server, crate::pace::Pacer::unlimited());
        let refused = client.profile_posts("someone", None).await.unwrap_err();
        assert!(matches!(refused, IgError::Browser(_)), "{refused:?}");
        assert!(
            server
                .received_requests()
                .await
                .unwrap_or_default()
                .is_empty()
        );
    }
}
