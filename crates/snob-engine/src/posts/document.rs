//! A post, a comment and a thread of replies as the JSON contract has them
//! (`snob_core::model::post`, AGENTS.md's "Output contract").

use serde_json::Value;
use snob_core::model::post as document;
use snob_core::model::printable;

use super::{CommentLine, Post, Thread};

/// `post` as the JSON contract has it ([`document::Post`]): everything a
/// listing shows, its items with their addresses, and its number in the
/// listing when it has one.
pub fn post_document(post: &Post, number: Option<usize>) -> document::Post {
    let items = post
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| document::PostItem {
            number: i + 1,
            kind: item.kind.label().to_string(),
            width: item.width,
            height: item.height,
            url: item.url.clone(),
            pk: item.pk.clone(),
            alt: item.alt.clone(),
            duration: item.duration,
            best: item.best.as_ref().map(|best| document::BestTracks {
                width: best.video.width,
                height: best.video.height,
                codec: best.video.codecs.as_deref().map(printable),
                bytes: best.video.bytes.map(|video| {
                    let audio = best.audio.as_ref().and_then(|a| a.bytes).unwrap_or(0);
                    video.saturating_add(audio)
                }),
                video_url: best.video.url.clone(),
                audio_url: best.audio.as_ref().map(|a| a.url.clone()),
            }),
        })
        .collect();
    document::Post {
        pk: post.pk.to_string(),
        code: post.code.clone(),
        url: post.link(),
        owner: post.owner.clone(),
        kind: post.kind.word().to_string(),
        taken_at: post.taken_at.get(),
        caption: post.caption.join("\n"),
        location: post.location.clone(),
        likes: (!post.likes_hidden).then_some(post.likes).flatten(),
        liked_by: post.liked_by.as_ref().map(|l| document::LikedBy {
            name: l.name.clone(),
            others: l.others,
        }),
        comments: post.comments,
        plays: post.plays,
        tagged: post.tagged.clone(),
        mentions: post.mentions.clone(),
        with: post.with.clone(),
        items,
        number,
        owner_pk: post.owner_pk.clone(),
        tagged_accounts: post.tagged_accounts.clone(),
        with_accounts: post.with_accounts.clone(),
        likers_known: post.likers_known.clone(),
        likes_hidden: post.likes_hidden,
        reposts: post.reposts,
        shares: post.shares,
        plays_instagram: post.plays_instagram,
        plays_facebook: post.plays_facebook,
        duration: post.duration,
        music: post.music.clone(),
        place: post.place.clone(),
        pinned: post.pinned,
        paid_partnership: post.paid_partnership,
        caption_edited: post.caption_edited,
        notes: post.notes.clone(),
    }
}

/// `comment` as the JSON contract has it, its shown replies under it.
pub fn comment_document(comment: &CommentLine) -> document::PostComment {
    document::PostComment {
        author: comment.author.clone(),
        text: comment.text.clone(),
        created_at: comment.at.get(),
        likes: comment.likes,
        replies: comment.replies_count,
        shown_replies: comment.replies.iter().map(comment_document).collect(),
        pk: comment.pk.clone(),
        author_pk: comment.author_pk.clone(),
        more_replies: comment.more_replies,
    }
}

/// `thread` as the JSON contract has it, under `replies_of`.
pub fn thread_document(thread: &Thread) -> document::RepliesOf {
    document::RepliesOf {
        comment_pk: thread.comment.to_string(),
        found: thread.found,
        count: thread.count,
        more: thread.more,
        replies: thread.replies.iter().map(comment_document).collect(),
    }
}

/// A post as JSON ([`post_document`]), for the listings that put rows
/// together as values. A document of these types always serializes.
pub fn post_json(post: &Post, number: Option<usize>) -> Value {
    serde_json::to_value(post_document(post, number)).unwrap_or_default()
}

/// A comment as JSON ([`comment_document`]).
pub fn comment_json(comment: &CommentLine) -> Value {
    serde_json::to_value(comment_document(comment)).unwrap_or_default()
}
