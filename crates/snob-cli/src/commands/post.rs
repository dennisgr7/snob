//! `snob post LINK` (and `snob reel LINK`): one post or reel by its address,
//! and how to keep a copy.

use std::path::Path;

use anyhow::Result;
use snob_ig::shortcode::Shortcode;
use snob_store::paths::AccountPaths;
use snob_store::secrets::SecretStore;

use crate::cli::{Format, PostArgs, StoryFormat};
use crate::commands::common;
use crate::commands::posts::{download_items, post_text, videos_of};
use crate::exit::{ExitCode, ExitError};
use crate::media;
use crate::output::{self, Presentation};
use crate::posts::{
    self, CommentLine, Comments, Post, Thread, comment_json, post_json, thread_document,
};
use crate::report;
use crate::ui;

pub async fn run(args: PostArgs, store: SecretStore, paths: &AccountPaths) -> Result<ExitCode> {
    // Before anything is opened or asked: a link that names no post costs
    // nothing.
    let link = Shortcode::parse(&args.link).map_err(|e| {
        ExitError::new(
            ExitCode::Error,
            format!("{e}: give a post's or a reel's link"),
        )
    })?;
    args.action.refuse_stdout_download_early()?;
    if args.json && args.action.output.as_deref().is_some_and(output::is_stdout) {
        return Err(ExitError::new(
            ExitCode::Error,
            "--json prints what was saved on standard output, so the files cannot go there \
             too: give -o a directory",
        )
        .into());
    }
    let threads =
        usize::try_from(args.replies.unwrap_or(0)).unwrap_or(usize::MAX) + args.replies_of.len();
    if threads > posts::MOST_THREADS {
        return Err(ExitError::new(
            ExitCode::Error,
            format!(
                "--replies and --replies-of ask for {threads} threads, and one run reads {} at \
                 most; nothing was sent",
                posts::MOST_THREADS
            ),
        )
        .into());
    }
    let videos = videos_of(&args.video, &args.action)?;
    let app = common::reader(&store, paths, args.action.interactive, false)?;
    let browses = args.action.browses(
        args.list.format.is_some(),
        ui::a_human_would_watch_the_listing_scroll_by(),
    );

    let post = posts::fetch_link(app.client(), &link).await?;
    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }

    if browses {
        return crate::ui::posts::browse_post(app.client(), post, app.viewer(), paths).await;
    }
    // The app never opens a post without its comments, so neither does the
    // printed form nor a download; the printed form reads on when asked to.
    let destination = args.action.output.as_deref();
    if let Some(selection) = args.action.selection() {
        posts::first_comments(app.client(), &post).await?;
        if args.json {
            let bounds = posts::Bounds {
                bytes: args.save_at_most,
                within: args.save_within,
            };
            return save_telling(&app, &post, selection, destination, &videos, bounds).await;
        }
        return download_items(&app, &post, selection, destination, &videos).await;
    }
    let Some(mut comments) =
        posts::comment_pages(app.client(), &post, args.comments.unwrap_or(1)).await?
    else {
        return Ok(ExitCode::Interrupted);
    };
    let first = usize::try_from(args.replies.unwrap_or(0)).unwrap_or(usize::MAX);
    let Some(threads) =
        posts::read_threads(app.client(), &post, &mut comments, first, &args.replies_of).await?
    else {
        return Ok(ExitCode::Interrupted);
    };
    let named = (!args.replies_of.is_empty()).then_some(threads.as_slice());
    let pictures = match args.list.small_pictures {
        true => Some(media::small_pictures(app.client(), post.thumbs()).await?),
        false => None,
    };
    list_post(
        &post,
        Some(&comments),
        named,
        pictures,
        args.list.format,
        destination,
    )
}

/// `-d --json`: the items `selection` names saved into `destination`, a
/// directory (the working one when none is given), and what became of each
/// printed as one document.
async fn save_telling(
    app: &crate::app::App,
    post: &Post,
    selection: crate::cli::DownloadSelection,
    destination: Option<&Path>,
    videos: &crate::posts::Videos,
    bounds: posts::Bounds,
) -> Result<ExitCode> {
    let numbers = crate::output::media::numbers_of(selection, post.items.len(), |n| {
        crate::commands::posts::no_such_item(post, n)
    })?;
    let files = videos.prepare(numbers.iter().map(|n| post.file(n - 1)).collect());
    let total = files.len();
    let dir = destination.unwrap_or(Path::new("."));
    let (document, failed) = posts::save_telling(app.client(), post, files, dir, bounds).await?;
    ui::say!("{}", serde_json::to_string_pretty(&document)?);
    if failed > 0 {
        return Err(ExitError::new(
            ExitCode::Error,
            format!("{failed} of {total} files could not be downloaded"),
        )
        .into());
    }
    Ok(ExitCode::Ok)
}

/// Prints the post, its items numbered, its comments when they were read,
/// and the threads of the comments `--replies-of` named (`named`), under
/// `replies_of` in the JSON. The numbers are what `-d` takes.
fn list_post(
    post: &Post,
    comments: Option<&Comments>,
    named: Option<&[Thread]>,
    pictures: Option<Vec<serde_json::Value>>,
    format: Option<StoryFormat>,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    let format = common::checked_format(format, destination, "a post")?;
    let text = match format {
        Format::Json | Format::Ndjson => {
            let rows = post_json(post, None)["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let listed = crate::output::media::rows_or_envelope(rows, format, |_| {
                let mut document = serde_json::json!({ "post": post_json(post, None) });
                if let Some(comments) = comments {
                    document["comments"] = serde_json::json!(
                        comments.lines.iter().map(comment_json).collect::<Vec<_>>()
                    );
                    document["more_comments"] = serde_json::json!(comments.next.is_some());
                }
                if let Some(named) = named {
                    document["replies_of"] =
                        serde_json::json!(named.iter().map(thread_document).collect::<Vec<_>>());
                }
                document
            })?;
            media::with_pictures(listed, pictures)?
        }
        _ => {
            let mut text = post_text(post, Presentation::detect(destination));
            if let Some(comments) = comments {
                text.push('\n');
                text.push_str(&comments_text(comments));
            }
            for thread in named.unwrap_or_default() {
                text.push_str("\n\n");
                text.push_str(&thread_text(thread));
            }
            text
        }
    };
    crate::output::media::print_listing(
        text,
        destination,
        "--download <number> to save one, --download all for every one, or --interactive to see it with its comments",
    )
}

/// The comments read, one to a line, each shown reply under its comment.
fn comments_text(comments: &Comments) -> String {
    let mut text = match comments.total {
        Some(total) => format!("Comments ({}):\n", posts::grouped(total)),
        None => "Comments:\n".to_string(),
    };
    if comments.lines.is_empty() {
        text.push_str("  none\n");
    }
    for comment in &comments.lines {
        text.push_str(&format!("  {}\n", comment_line(comment)));
        for reply in &comment.replies {
            text.push_str(&format!("      ↳ {}\n", comment_line(reply)));
        }
        if comment.more_replies == Some(true) {
            text.push_str(&format!("      {MORE_NOT_SHOWN}\n"));
        }
    }
    if comments.next.is_some() {
        text.push_str("  … more with --comments <pages>, or in the view (-i)\n");
    }
    text.pop();
    text
}

/// What a thread that says it holds more than it handed out is followed
/// by: they are there, and not read, since no capture shows how they are
/// asked for.
pub const MORE_NOT_SHOWN: &str = "… more replies, not shown";

/// The replies under a comment `--replies-of` named, or why there are none.
fn thread_text(thread: &Thread) -> String {
    let mut text = format!("Replies to comment {}:\n", thread.comment);
    if !thread.found {
        text.push_str("  none found: the comment is gone, or not on this post\n");
    } else if thread.replies.is_empty() {
        text.push_str("  none\n");
    }
    for reply in &thread.replies {
        text.push_str(&format!("  ↳ {}\n", comment_line(reply)));
    }
    if thread.more {
        text.push_str(&format!("  {MORE_NOT_SHOWN}\n"));
    }
    text.pop();
    text
}

/// `@author: text · Aug 3, 2026 · 3 likes · 2 replies`.
pub fn comment_line(comment: &CommentLine) -> String {
    let mut line = format!(
        "@{}: {} · {}",
        comment.author,
        if comment.text.is_empty() {
            "(a sticker or a GIF)"
        } else {
            &comment.text
        },
        report::dated(comment.at)
    );
    if comment.likes > 0 {
        line.push_str(&match comment.likes {
            1 => " · 1 like".to_string(),
            n => format!(" · {} likes", posts::grouped(n)),
        });
    }
    if comment.replies_count > 0 {
        line.push_str(&match comment.replies_count {
            1 => " · 1 reply".to_string(),
            n => format!(" · {} replies", posts::grouped(n)),
        });
    }
    line
}

#[cfg(test)]
mod tests {
    use snob_core::Epoch;

    use super::*;

    fn comment(author: &str, text: &str, likes: u64, replies: u64) -> CommentLine {
        CommentLine {
            author: author.into(),
            text: text.into(),
            at: Epoch::new(1_790_510_400),
            likes,
            replies_count: replies,
            replies: Vec::new(),
            pk: None,
            author_pk: None,
            more_replies: None,
        }
    }

    #[test]
    fn a_comment_says_its_likes_and_replies() {
        let line = comment_line(&comment("a", "nice", 3, 1));
        assert!(line.starts_with("@a: nice · "), "{line}");
        assert!(line.ends_with(" · 3 likes · 1 reply"), "{line}");
        let quiet = comment_line(&comment("b", "", 0, 0));
        assert!(quiet.starts_with("@b: (a sticker or a GIF) · "), "{quiet}");
        assert!(!quiet.contains("like"), "{quiet}");
    }

    /// A thread read says what it handed out, and that more are there only
    /// when Instagram said so, never by its count less what came.
    #[test]
    fn fewer_replies_than_the_count_are_said_as_they_came() {
        let mut read = comment("a", "nice", 0, 9);
        read.replies = vec![comment("b", "yes", 0, 0)];
        read.more_replies = Some(false);
        let mut more = comment("c", "hm", 0, 4);
        more.more_replies = Some(true);
        let text = comments_text(&Comments {
            lines: vec![read, more],
            next: None,
            total: Some(2),
        });
        assert!(!text.contains("8 more"), "{text}");
        assert_eq!(text.matches(MORE_NOT_SHOWN).count(), 1, "{text}");

        let thread = Thread {
            comment: snob_ig::shortcode::CommentPk::new(18_000_000_000_000_001),
            found: false,
            count: None,
            more: false,
            replies: Vec::new(),
        };
        let said = thread_text(&thread);
        assert!(
            said.starts_with("Replies to comment 18000000000000001:"),
            "{said}"
        );
        assert!(said.contains("none found"), "{said}");
    }

    #[test]
    fn the_comments_show_their_replies_and_whether_more_are_left() {
        let mut first = comment("a", "nice", 0, 2);
        first.replies = vec![comment("b", "yes", 0, 0)];
        let text = comments_text(&Comments {
            lines: vec![first],
            next: Some("x".into()),
            total: Some(1_200),
        });
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Comments (1,200):");
        assert!(lines[1].starts_with("  @a: nice"), "{text}");
        assert!(lines[2].starts_with("      ↳ @b: yes"), "{text}");
        assert_eq!(
            lines[3],
            "  … more with --comments <pages>, or in the view (-i)"
        );
    }
}
