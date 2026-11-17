//! How a post's files are kept: a video at its best or as the app's own
//! file ([`Videos`]), one item for a view to open or to keep, and a run's
//! files told as one document ([`save_telling`]).

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde_json::Value;
use snob_ig::client::IgClient;
use snob_store::config::VideoQuality;

use super::{Post, post_document};
use crate::exit::{ExitCode, ExitError};
use crate::ffmpeg::{self, Ffmpeg};
use crate::files;
use crate::host;
use crate::media::{Bounds, Named, Saved, save_file_saying, save_named};

/// What a run saves a post's videos as (`--quality`, or `quality` in `watch.toml`).
#[derive(Debug, Clone)]
pub enum Videos {
    /// The progressive file; `say_how` when it is so because no ffmpeg was
    /// found, which a run that would have saved better says once.
    Compat { say_how: bool },
    /// The best tracks, joined by this ffmpeg.
    Best(std::sync::Arc<Ffmpeg>),
}

impl Videos {
    /// What `asked` and `named` (`--ffmpeg`) make of this run: best when
    /// ffmpeg is found and nothing else was asked, and `--quality best`
    /// with no ffmpeg refused before anything is asked of Instagram.
    pub fn resolve(asked: Option<VideoQuality>, named: Option<&Path>) -> Result<Self> {
        let found = Ffmpeg::find(named).map(std::sync::Arc::new);
        match (asked, found) {
            (Some(VideoQuality::Compat), _) => Ok(Self::Compat { say_how: false }),
            (_, Some(ffmpeg)) => Ok(Self::Best(ffmpeg)),
            (None, None) => Ok(Self::Compat { say_how: true }),
            (Some(VideoQuality::Best), None) => Err(ExitError::new(
                ExitCode::Error,
                "--quality best joins a video's tracks with ffmpeg, and none was found",
            )
            .with_hint(format!(
                "install it ({}), or name it with --ffmpeg or {}",
                ffmpeg::how_to_install(),
                ffmpeg::ENV
            ))
            .into()),
        }
    }

    /// What the full-screen view keeps a video as: the run's default, which
    /// never fails.
    pub fn for_the_view() -> Self {
        Self::resolve(None, None).unwrap_or(Self::Compat { say_how: false })
    }

    /// `files` made ready to save as this says: each video's best tracks kept,
    /// with the ffmpeg that joins them, or dropped.
    pub fn prepare(&self, files: Vec<Named>) -> Vec<Named> {
        if matches!(self, Self::Compat { say_how: true }) && files.iter().any(|f| f.best.is_some())
        {
            host::info(&format!(
                "Videos are saved as the app's 720-wide file: with ffmpeg installed ({}), \
                 snob saves them at their best, joined into one MP4",
                ffmpeg::how_to_install()
            ));
        }
        files
            .into_iter()
            .map(|file| match self {
                Self::Best(ffmpeg) => Named {
                    ffmpeg: Some(std::sync::Arc::clone(ffmpeg)),
                    ..file
                },
                Self::Compat { .. } => Named { best: None, ..file },
            })
            .collect()
    }
}

/// Saves item `index` (zero-based) of `post` into `dir` under its own name,
/// as its progressive file: what the view hands the system viewer, which
/// starts at once and plays anywhere (QuickTime plays no VP9).
async fn save_item(client: &IgClient, post: &Post, index: usize, dir: &Path) -> Result<Saved> {
    let file = post.file(index);
    save_named(
        client,
        &file.stem,
        file.url.as_deref(),
        &file.what,
        file.cap,
        dir,
    )
    .await
}

/// Item `index` of `post` as the view keeps one: at its best when ffmpeg
/// is found ([`Videos::for_the_view`]).
fn kept_file(post: &Post, index: usize) -> Result<Named> {
    anyhow::ensure!(
        index < post.items.len(),
        "item {} is not in the post",
        index + 1
    );
    Videos::for_the_view()
        .prepare(vec![post.file(index)])
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("item {} is not in the post", index + 1))
}

/// Saves item `index` of `post` into `dir` as the view keeps one, with the
/// sentence to show when it was saved as the app's smaller file after all.
async fn keep_one(
    client: &IgClient,
    post: &Post,
    index: usize,
    dir: &Path,
) -> Result<(Saved, Option<String>)> {
    save_file_saying(client, &kept_file(post, index)?, dir).await
}

/// What [`keep_item`] saved: where, and the sentence to show beside it
/// when the video was saved as the app's smaller file after all.
#[derive(Debug)]
pub struct Kept {
    pub path: PathBuf,
    pub said: Option<String>,
}

/// Saves `files`, items of `post`, into the directory `dir`, one after another,
/// and answers with what became of each as one JSON document, and with how many
/// failed:
///
/// - `post`: what tells the post from every other (`pk`, `code`, `url`,
///   `owner`, `kind`, `taken_at`);
/// - `directory`: where the files are, absolute;
/// - `saved`: each file there, by its item's number: its `file` name, its
///   `bytes`, its `kind` and the item's `pk`, `already` when an earlier run had
///   saved it and nothing was fetched, and a `note` when a video was saved as
///   the app's smaller file;
/// - `failed`: the items that could not be fetched, each with `why`;
/// - `left`: the items not started because a bound of `bounds` was reached,
///   each with `why` (`bytes` or `time`): asking again saves them, since those
///   already there are not fetched twice;
/// - `bytes`: what this run fetched.
///
/// **For a caller that reads the answer rather than a directory** (`post -d
/// --json`, which `snob mcp` runs for an agent): `dir` is always a directory,
/// made when it is not there, so one file is never written over a name, and
/// nothing is, since every file goes in under a name of its own
/// (`media::save_named`).
pub async fn save_telling(
    client: &IgClient,
    post: &Post,
    files: Vec<Named>,
    dir: &Path,
    bounds: Bounds,
) -> Result<(Value, usize)> {
    use anyhow::Context as _;

    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let directory = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let started = std::time::Instant::now();
    let (mut saved, mut failed, mut left) = (Vec::new(), Vec::new(), Vec::new());
    let mut fetched: u64 = 0;
    for file in files {
        if client.pacer().cancel_token().is_canceled() {
            return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
        }
        if let Some(why) = bounds.passed(fetched, started) {
            left.push(serde_json::json!({ "item": file.number, "why": why }));
            continue;
        }
        let item = post.items.get(file.number.wrapping_sub(1));
        // What the best tracks weigh, when they are tried: fetched whether
        // or not they end as the file kept, so counted when they do not.
        let tried = crate::media::best_weight(&file);
        match crate::media::save_file_saying(client, &file, &directory).await {
            Ok((kept, note)) => {
                let (path, already) = match kept {
                    Saved::Now(path) => (path, false),
                    Saved::Already(path) => (path, true),
                };
                let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                if !already {
                    fetched = fetched.saturating_add(bytes);
                    // A note is the best tracks given up for the smaller
                    // file: both were fetched.
                    if note.is_some() {
                        fetched = fetched.saturating_add(tried);
                    }
                }
                // To a program, without the address a failure's words name.
                let note = note.map(|note| crate::media::without_addresses(&note));
                saved.push(serde_json::json!({
                    "item": file.number,
                    "pk": item.and_then(|item| item.pk.clone()),
                    "kind": item.map(|item| item.kind.label()),
                    "file": path.file_name().map(|name| name.to_string_lossy().into_owned()),
                    "bytes": bytes,
                    "already": already,
                    "note": note,
                }));
            }
            Err(e) if crate::media::was_canceled(&e) => {
                return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
            }
            // A push-back, or the cooldown it started: the run ends, with
            // no further file asked for.
            Err(e) if crate::media::stopped_the_run(&e) => return Err(e),
            Err(e) => {
                // What it may have fetched before it failed counts too.
                fetched = fetched.saturating_add(tried);
                failed.push(serde_json::json!({
                    "item": file.number,
                    "why": crate::media::without_addresses(&e.to_string()),
                }));
            }
        }
    }
    let about = post_document(post, None);
    let document = serde_json::json!({
        "post": {
            "pk": about.pk,
            "code": about.code,
            "url": about.url,
            "owner": about.owner,
            "kind": about.kind,
            "taken_at": about.taken_at,
        },
        "directory": directory.to_string_lossy(),
        "saved": saved,
        "failed": failed,
        "left": left,
        "bytes": fetched,
    });
    let failures = document["failed"].as_array().map_or(0, Vec::len);
    Ok((document, failures))
}

/// The files of every item of `posts`, in order.
pub fn files_of(posts: &[&Post]) -> Vec<Named> {
    let mut number = 0;
    let mut files = Vec::new();
    for post in posts {
        for index in 0..post.items.len() {
            number += 1;
            files.push(Named {
                number,
                ..post.file(index)
            });
        }
    }
    files
}

/// Where a post shown by a view was saved into the scratch directory to be
/// handed to the system viewer, per item, so opening one twice is one
/// download.
pub type Opened = Vec<Option<PathBuf>>;

/// Item `index` of `post`, fetched into `scratch` as the view opens one, the
/// app's own file, and not opened: once per item, so asking twice is one
/// download.
pub async fn fetched_item(
    client: &IgClient,
    post: &Post,
    index: usize,
    scratch: &Path,
    opened: &mut Opened,
) -> Result<PathBuf> {
    if let Some(existing) = opened.get(index).cloned().flatten()
        && existing.is_file()
    {
        return Ok(existing);
    }
    let path = match save_item(client, post, index, scratch).await? {
        Saved::Now(path) | Saved::Already(path) => path,
    };
    if opened.len() < post.items.len() {
        opened.resize(post.items.len(), None);
    }
    if let Some(slot) = opened.get_mut(index) {
        *slot = Some(path.clone());
    }
    Ok(path)
}

/// Saves item `index` of `post` where the user works: the copy already fetched
/// to look at, when there is one and it is what would be saved, and otherwise
/// the CDN's.
pub async fn keep_item(
    client: &IgClient,
    post: &Post,
    index: usize,
    opened: &Opened,
) -> Result<Kept> {
    let at_its_best = kept_file(post, index)?.best.is_some();
    if !at_its_best
        && let Some(seen) = opened.get(index).cloned().flatten()
        && seen.is_file()
        && let Some(name) = seen.file_name()
    {
        let target = Path::new(".").join(name);
        let mut out = files::create_new(&target)?;
        let mut from = std::fs::File::open(&seen)?;
        std::io::copy(&mut from, &mut out)?;
        return Ok(Kept {
            path: target,
            said: None,
        });
    }
    match keep_one(client, post, index, Path::new(".")).await? {
        (Saved::Now(path), said) => Ok(Kept { path, said }),
        (Saved::Already(path), _) => Err(anyhow!("{} is already here", path.display())),
    }
}

/// Saves every item of `post` where the user works, one at a time.
pub async fn keep_post(client: &IgClient, post: &Post) -> (String, bool) {
    let total = post.items.len();
    let (mut kept, mut failed, mut smaller) = (0usize, 0usize, 0usize);
    for index in 0..total {
        if client.pacer().cancel_token().is_canceled() {
            return (format!("Stopped after {kept} of {total}"), kept > 0);
        }
        match keep_one(client, post, index, Path::new(".")).await {
            Ok((_, said)) => {
                kept += 1;
                smaller += usize::from(said.is_some());
            }
            Err(_) => failed += 1,
        }
    }
    let mut note = if failed == 0 {
        format!("Saved {kept} of {total} from {} here", post.code)
    } else {
        format!(
            "Saved {kept} of {total} from {}; {failed} failed",
            post.code
        )
    };
    if smaller > 0 {
        note.push_str(&format!("; {smaller} as the app's smaller file"));
    }
    (note, kept > 0)
}
