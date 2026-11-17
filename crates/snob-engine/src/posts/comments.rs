//! A post's comments and the threads of replies under them: a page at a
//! time, as "load more" is clicked, and one read a thread.

use anyhow::Result;
use snob_core::Epoch;
use snob_core::model::printable;
use snob_ig::client::IgClient;
use snob_ig::error::IgError;
use snob_ig::model::post::{Comment, CommentsPage};
use snob_ig::pace::Pace;
use snob_ig::shortcode::CommentPk;

use super::{Post, digits};
use crate::engine::rest_before_a_page;

/// One comment, or a reply shown under one, filtered for a terminal.
#[derive(Debug, Clone)]
pub struct CommentLine {
    pub author: String,
    pub text: String,
    pub at: Epoch,
    pub likes: u64,
    /// How many replies it has; only the ones in `replies` are shown.
    pub replies_count: u64,
    pub replies: Vec<CommentLine>,
    pub pk: Option<String>,
    pub author_pk: Option<String>,
    /// `None` until its thread is read ([`read_threads`]): `replies` are the
    /// page's previews.
    pub more_replies: Option<bool>,
}

impl CommentLine {
    /// Whether its thread holds replies the page did not show: what
    /// `--replies` and the view's `r` read.
    pub fn replies_missing(&self) -> bool {
        self.more_replies.is_none()
            && self.replies_count > u64::try_from(self.replies.len()).unwrap_or(u64::MAX)
    }

    /// Its id, when it has one a thread can be asked under.
    pub fn comment_pk(&self) -> Option<CommentPk> {
        self.pk.as_deref().and_then(CommentPk::parse)
    }

    fn from(comment: &Comment) -> Self {
        Self {
            author: printable(
                comment
                    .user
                    .as_ref()
                    .and_then(|u| u.username.as_deref())
                    .unwrap_or("someone"),
            ),
            text: printable(comment.text.as_deref().unwrap_or_default()),
            at: comment.created_at,
            likes: comment.comment_like_count.unwrap_or(0),
            replies_count: comment.child_comment_count.unwrap_or(0),
            replies: comment
                .preview_child_comments
                .iter()
                .map(Self::from)
                .collect(),
            pk: comment.pk.as_deref().and_then(digits),
            author_pk: comment
                .user
                .as_ref()
                .and_then(|u| u.pk.as_deref())
                .and_then(digits),
            more_replies: None,
        }
    }
}

/// The most threads of replies one run reads, `--replies` and
/// `--replies-of` together: ten, each one request, which with the twenty
/// pages of comments a run may read keeps one post under a sitting
/// ([`snob_ig::pace::PAGES_PER_SITTING`]).
pub const MOST_THREADS: usize = 10;

/// The replies under one comment, as far as the first page of its thread
/// goes ([`read_thread`]).
#[derive(Debug, Clone)]
pub struct Thread {
    pub comment: CommentPk,
    /// Whether Instagram answered with a thread of a comment on this post.
    pub found: bool,
    /// How many replies the comment has, as Instagram counts them: more
    /// than it hands out on every thread seen, so nothing is worked out
    /// from it.
    pub count: Option<u64>,
    /// Whether Instagram said the thread holds replies it did not hand out.
    pub more: bool,
    pub replies: Vec<CommentLine>,
}

impl Thread {
    /// A thread Instagram answered nothing of this post's for.
    fn not_found(comment: CommentPk) -> Self {
        Self {
            comment,
            found: false,
            count: None,
            more: false,
            replies: Vec::new(),
        }
    }

    /// What it puts under the comment it is of, once read.
    fn fill(&self, line: &mut CommentLine) {
        if self.found {
            line.replies.clone_from(&self.replies);
            line.more_replies = Some(self.more);
        }
    }
}

/// The replies under `comment` of `post`: one read, from the post's page.
///
/// **A thread Instagram answers nothing for is not a failure of the run**: an
/// answer that names another post's comment, or none, and a refusal of the
/// request itself (a 4xx that is no push-back, or "not found") are a thread
/// `found: false`, since a comment named by a person may be gone or on another
/// post.
pub async fn read_thread(client: &IgClient, post: &Post, comment: CommentPk) -> Result<Thread> {
    let page = match client.comment_replies(post.pk, comment, &post.page()).await {
        Ok(page) => page,
        Err(IgError::NotFound { .. }) => return Ok(Thread::not_found(comment)),
        Err(IgError::Unexpected { status, .. }) if (400..500).contains(&status) => {
            return Ok(Thread::not_found(comment));
        }
        Err(e) => return Err(e.into()),
    };
    if !page.is_on(post.pk) {
        return Ok(Thread::not_found(comment));
    }
    Ok(Thread {
        comment,
        found: true,
        count: page.child_comment_count,
        more: page.more(),
        replies: page.child_comments.iter().map(CommentLine::from).collect(),
    })
}

/// The comments of `comments` whose threads `--replies <first>` reads: the
/// first `first` that hold replies the page did not show, in the order they
/// were read.
pub fn threads_to_read(comments: &Comments, first: usize) -> Vec<CommentPk> {
    comments
        .lines
        .iter()
        .filter(|line| line.replies_missing())
        .filter_map(CommentLine::comment_pk)
        .take(first)
        .collect()
}

/// Reads the threads `--replies` and `--replies-of` ask for, once each and at
/// most [`MOST_THREADS`] in all: those of the first `first` comments read with
/// replies missing ([`threads_to_read`]), put under their comments, then each
/// of `of` not read already, which need not be among the comments read.
pub async fn read_threads(
    client: &IgClient,
    post: &Post,
    comments: &mut Comments,
    first: usize,
    of: &[CommentPk],
) -> Result<Option<Vec<Thread>>> {
    let mut asked = threads_to_read(comments, first);
    for comment in of {
        if !asked.contains(comment) {
            asked.push(*comment);
        }
    }
    asked.truncate(MOST_THREADS);
    let pace = Pace::for_comments();
    let mut read: Vec<Thread> = Vec::new();
    for comment in asked {
        if rest_before_a_page(client, "the replies").await? || client.step_between(&pace).await {
            return Ok(None);
        }
        let thread = read_thread(client, post, comment).await?;
        if let Some(line) = comments
            .lines
            .iter_mut()
            .find(|line| line.comment_pk() == Some(comment))
        {
            thread.fill(line);
        }
        read.push(thread);
    }
    Ok(Some(
        of.iter()
            .filter_map(|comment| read.iter().find(|t| t.comment == *comment).cloned())
            .collect(),
    ))
}

/// The replies under the comment `line` is, put under it: what the view's
/// `r` reads. One read.
pub async fn replies_under(client: &IgClient, post: &Post, line: &mut CommentLine) -> Result<bool> {
    let Some(comment) = line.comment_pk() else {
        return Ok(false);
    };
    let thread = read_thread(client, post, comment).await?;
    thread.fill(line);
    Ok(thread.found)
}

/// The comments of a post read so far, and where the next page starts.
#[derive(Debug, Clone, Default)]
pub struct Comments {
    pub lines: Vec<CommentLine>,
    pub next: Option<String>,
    pub total: Option<u64>,
}

impl Comments {
    pub(super) fn take(&mut self, page: &CommentsPage) {
        self.lines
            .extend(page.comments.iter().map(CommentLine::from));
        self.next = page.next().map(str::to_string);
        self.total = page.comment_count.or(self.total);
    }
}

/// The next page of `post`'s comments, one read.
pub async fn more_comments(
    client: &IgClient,
    post: &Post,
    comments: &mut Comments,
) -> Result<bool> {
    let Some(after) = comments.next.clone() else {
        return Ok(false);
    };
    let page = client
        .media_comments(post.pk, Some(&after), &post.page())
        .await?;
    comments.take(&page);
    Ok(true)
}

/// The first page of `post`'s comments, one read.
pub async fn first_comments(client: &IgClient, post: &Post) -> Result<Comments> {
    let mut comments = Comments::default();
    comments.take(&client.media_comments(post.pk, None, &post.page()).await?);
    Ok(comments)
}

/// The most pages of a post's comments one run reads: twenty, some 300
/// comments at the fifteen a page the app reads (capture of 2026-11-13),
/// and half a sitting ([`snob_ig::pace::PAGES_PER_SITTING`]), so that one post cannot be a
/// list's worth of reading.
pub const MOST_COMMENT_PAGES: u32 = 20;

/// `post`'s comments, `pages` pages of them, or fewer when they end: the first,
/// then each next one a comment step after the page before
/// ([`Pace::for_comments`]), as "load more" is clicked, past any rest the
/// account's sitting owes.
pub async fn comment_pages(client: &IgClient, post: &Post, pages: u32) -> Result<Option<Comments>> {
    let mut comments = first_comments(client, post).await?;
    let pace = Pace::for_comments();
    for _ in 1..pages.min(MOST_COMMENT_PAGES) {
        if comments.next.is_none() {
            break;
        }
        if rest_before_a_page(client, "the comments").await? || client.step_between(&pace).await {
            return Ok(None);
        }
        more_comments(client, post, &mut comments).await?;
    }
    Ok(Some(comments))
}
