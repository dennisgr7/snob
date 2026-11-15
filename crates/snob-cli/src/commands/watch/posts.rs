//! Saving a watched account's new posts where its entry in `watch.toml` says.
//!
//! **Only the person chooses the directory**, in `watch.toml` (`save_to` under
//! `[account.posts]`, written by hand or by `watch setup`): no flag carries it,
//! and the monitor is offered to no agent.
//!
//! **From what the grid answered, and nothing more**: the grid's answer carries
//! every item of every post, so no post is opened and nothing but the files is
//! fetched, and the files come from the CDN, outside the request budget, which
//! is why a run's saving has bounds of its own ([`SAVE_AT_MOST`],
//! [`SAVE_WITHIN`]).

use std::time::{Duration, Instant};

use anyhow::Result;

use crate::ui;
use snob_engine::app::App;
use snob_engine::engine::watch::{TickReport, Watched};
use snob_engine::engine::watch_posts::SavedPost;
use snob_engine::exit::ExitCode;
use snob_engine::posts::{Bounds, Post, Videos, files_of, save_telling};

/// The most one run fetches into an account's directory: 1 GiB, the ceiling one
/// call of an agent has (`commands::mcp::tools::SAVE_AT_MOST`, kept apart
/// because that module is behind the `mcp` feature).
pub(super) const SAVE_AT_MOST: u64 = 1 << 30;

/// How long one run goes on starting files for an account: 10 minutes, under
/// the 15 between two looks of the timer `watch unit` prints, so a run that
/// saves is over before the next one asks.
pub(super) const SAVE_WITHIN: Duration = Duration::from_secs(10 * 60);

/// What a video is saved as for `watched`, asked before anything is sent: a
/// `quality = "best"` with no ffmpeg on this machine is refused as a webhook
/// address that could never work is, costing nothing.
pub(super) fn videos_for(watched: &Watched) -> Result<Option<Videos>> {
    let Some(entry) = watched.posts() else {
        return Ok(None);
    };
    if entry.save_to.is_none() {
        return Ok(None);
    }
    Videos::resolve(entry.quality, None).map(Some)
}

/// Refuses a run whose entries ask for what this machine cannot save, before
/// it spends anything. What `once` and the scheduled mode ask first.
pub(super) fn refuse_what_cannot_be_saved(watched: &[Watched]) -> Result<()> {
    for entry in watched {
        videos_for(entry)?;
    }
    Ok(())
}

/// Saves the files the posts of `tick` owe, as its entry says, and writes on
/// the tick what became of them.
pub async fn save_new(app: &App, tick: &mut TickReport, watched: &Watched) -> Result<()> {
    let Some(dir) = watched.posts().and_then(|entry| entry.save_to.clone()) else {
        return Ok(());
    };
    let Some(report) = tick.report.posts.as_ref() else {
        return Ok(());
    };
    let Some(videos) = videos_for(watched)? else {
        return Ok(());
    };

    // The new posts first, then those an earlier run left owing a file that
    // are still on the pages read, newest first.
    let owed: Vec<u64> =
        snob_store::store::watch_posts::pending_saves(app.db().conn(), tick.report.account_pk)?
            .into_iter()
            .map(|(pk, _)| pk)
            .collect();
    let mut posts: Vec<(Post, bool)> = report
        .news
        .iter()
        .map(|post| (post.clone(), true))
        .collect();
    posts.extend(
        tick.read_posts
            .iter()
            .filter(|post| owed.contains(&post.pk.get()))
            .filter(|post| !report.news.iter().any(|new| new.pk == post.pk))
            .map(|post| (post.clone(), false)),
    );

    let started = Instant::now();
    let mut fetched: u64 = 0;
    let mut done = Vec::new();
    let mut said = Vec::new();
    for (post, new) in posts {
        let bounds = Bounds {
            bytes: Some(SAVE_AT_MOST.saturating_sub(fetched)),
            within: Some(SAVE_WITHIN.saturating_sub(started.elapsed())),
        };
        let files = videos.prepare(files_of(&[&post]));
        let item_count = files.len();
        match save_telling(app.client(), &post, files, &dir, bounds).await {
            Ok((document, failed)) => {
                fetched = fetched.saturating_add(document["bytes"].as_u64().unwrap_or(0));
                let left = document["left"].as_array().map_or(0, Vec::len);
                let names: Vec<String> = document["saved"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|saved| saved["file"].as_str().map(str::to_string))
                    .collect();
                if failed == 0 && left == 0 {
                    done.push(post.pk);
                }
                if new {
                    said.push(SavedPost {
                        pk: post.pk,
                        directory: document["directory"]
                            .as_str()
                            .map_or_else(|| dir.to_string_lossy().into_owned(), str::to_string),
                        files: names,
                        failed,
                        left,
                    });
                }
            }
            // Ctrl+C: the tick ends, and nothing of it is committed.
            Err(e) if snob_engine::exit::exit_code_for(&e) == ExitCode::Interrupted => {
                return Err(e);
            }
            // A push-back, or the cooldown it started: no further file is
            // asked for, and the report still goes out.
            Err(e) if snob_engine::media::stopped_the_run(&e) => {
                ui::warn(&format!(
                    "saving the new posts stopped: {e:#}. What was not saved is owed to a later \
                     run."
                ));
                break;
            }
            // The directory could not be made or written: every file of the
            // post is owed, and so is every post after it, which would meet
            // the same directory.
            Err(e) => {
                ui::warn(&format!(
                    "the new posts could not be saved to {}: {e:#}",
                    snob_core::model::printable(&dir.to_string_lossy())
                ));
                if new {
                    said.push(SavedPost {
                        pk: post.pk,
                        directory: dir.to_string_lossy().into_owned(),
                        files: Vec::new(),
                        failed: item_count,
                        left: 0,
                    });
                }
                break;
            }
        }
    }
    tick.record_saves(done, said);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two bounds, as their comments say.
    #[test]
    fn a_run_saves_at_most_a_gibibyte_within_ten_minutes() {
        assert_eq!(SAVE_AT_MOST, 1_073_741_824);
        assert_eq!(SAVE_WITHIN, Duration::from_secs(600));
        const _: () = assert!(SAVE_WITHIN.as_secs() < 15 * 60);
    }
}
