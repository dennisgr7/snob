//! The accounts that liked a post, as many as Instagram shows the web app.
//!
//! The app is shown about a hundred (105 of 603,328 twice in the capture of
//! 2026-11-13, 125 of 1,796 in the capture of 2026-11-14), the viewer's
//! contacts first, with no cursor to a second page (the list scrolled to its
//! end on 2026-11-14 asked for none): one read, and what it answers is said
//! to be a sample unless it is all of them.
//!
//! **They are other people's accounts**: whoever reads them asks first
//! (AGENTS.md, "Consent"), which is the front's to do before it calls this.

use anyhow::Result;
use snob_core::model::post::{Liker, Likers};
use snob_ig::client::IgClient;
use snob_ig::model::post::LikersPage;

use super::Post;

/// The most accounts one answer was seen to hold, with a margin: what the
/// day's accounts must still allow before the request is sent, so a read the
/// day cannot pay for is refused rather than made. 105 twice in the capture
/// of 2026-11-13 and 125 in the capture of 2026-11-14, past the 120 this was
/// until then; 150 leaves a fifth over the largest seen.
pub const LIKERS_AT_MOST: u32 = 150;

/// The accounts that liked `post`, as far as Instagram shows them: one read,
/// from the post's page.
pub async fn likers(client: &IgClient, post: &Post) -> Result<Likers> {
    let page = client.media_likers(post.pk, &post.page()).await?;
    Ok(likers_of(post, &page))
}

/// The document of a post's likers: every account the answer named, its
/// names made safe to print, and the count unless the owner hid it.
fn likers_of(post: &Post, page: &LikersPage) -> Likers {
    let accounts: Vec<Liker> = page
        .accounts()
        .into_iter()
        .map(|(account, standing)| Liker {
            account: snob_core::model::User {
                username: account.safe_username(),
                full_name: account.safe_full_name(),
                ..account
            },
            you_follow: standing.as_ref().map(|s| s.following),
            follows_you: standing.as_ref().map(|s| s.followed_by),
        })
        .collect();
    // The post's count first, the one the app prints; the likers' own when the
    // post said none.
    let likes = if post.likes_hidden {
        None
    } else {
        post.likes.or(page.user_count)
    };
    let shown = accounts.len();
    Likers {
        code: post.code.clone(),
        url: post.link(),
        likes,
        shown,
        complete: likes.is_some_and(|likes| likes == shown as u64),
        accounts,
    }
}
