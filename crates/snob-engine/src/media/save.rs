//! A download from the CDN: one story, the files of a post, and the document
//! that says what became of each.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use snob_core::model::printable;
use snob_ig::client::IgClient;
use snob_ig::error::IgError;
use snob_ig::model::dash::Track;

use super::{Kind, MAX_STORY_BYTES, Story};
use crate::exit::{ExitCode, ExitError};
use crate::ffmpeg::Ffmpeg;
use crate::files::{self, ABANDONED_AFTER};
use crate::host;

/// What [`save_story`] found: the file it wrote, or the one already there.
pub enum Saved {
    Now(PathBuf),
    Already(PathBuf),
}

/// Saves one story under the name the listing implies, to disk as it arrives.
///
/// **Nothing is fetched for a story already on disk**, and the file is
/// written to `<stem>.<pid>.part` and put under its name once whole, never
/// over one that is there (`files::place_new`).
pub async fn save_story(
    client: &IgClient,
    stem_base: &str,
    items: &[Story],
    number: usize,
    dir: &Path,
) -> Result<Saved> {
    let stem = format!("{}-{number}", printable(stem_base));
    let url = url_of(items, number);
    save_named(
        client,
        &stem,
        url,
        &format!("story {number}"),
        MAX_STORY_BYTES,
        dir,
    )
    .await
}

/// Where story `number` of `items` is, counted from one; a number that is
/// none of them has no address, and is refused as a story with no file.
pub(super) fn url_of(items: &[Story], number: usize) -> Option<&str> {
    items.get(number.checked_sub(1)?)?.url.as_deref()
}

/// [`save_story`] for any file of the CDN: `url` saved into `dir` as
/// `<stem>.<extension>`, the extension read from the bytes, by the same steps
/// and for the same reasons.
///
/// **The stem passes `files::usable_stem` before anything is made of it.**
pub(crate) async fn save_named(
    client: &IgClient,
    stem: &str,
    url: Option<&str>,
    what: &str,
    cap: usize,
    dir: &Path,
) -> Result<Saved> {
    files::usable_stem(stem)?;
    if let Some(existing) = already_saved(dir, stem) {
        return Ok(Saved::Already(existing));
    }
    let url = url.ok_or_else(|| anyhow!("{what} has no downloadable version"))?;

    // The scratch name carries the process id, so two runs saving into the same
    // directory never share one: neither can unlink the other's half-written
    // download, nor publish the other's truncated bytes under the final name
    // with its rename.
    sweep_stale_parts(dir, stem);
    let part = dir.join(format!("{stem}.{}.part", std::process::id()));
    let file = files::create_new(&part)?;
    // Only once the file is this download's own: one found there is not.
    let part = files::Part(part);
    // Bound after `part`, so it is closed before `part` is removed.
    let mut file = file;
    let downloaded = client.download_to(url, cap, &mut file).await?;
    file.sync_all()
        .with_context(|| format!("could not finish writing {}", part.0.display()))?;
    drop(file);
    files::place_new(&part.0, dir, stem, extension_of(&downloaded.head)).map(Saved::Now)
}

/// Removes `.part` files for this stem that no live download can still own.
fn sweep_stale_parts(dir: &Path, stem: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // `user-3.part` and `user-3.<pid>.part` both belong to this stem;
        // `user-33.part` does not, which is what the dot after the stem asks.
        let ours = name
            .strip_prefix(stem)
            .is_some_and(|rest| rest.starts_with('.') && rest.ends_with(".part"));
        if !ours {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age > ABANDONED_AFTER)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The file a story with this stem was saved to earlier, if any extension
/// `extension_of` can produce is already there.
fn already_saved(dir: &Path, stem: &str) -> Option<PathBuf> {
    ["mp4", "webp", "png", "jpg"]
        .into_iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|candidate| candidate.is_file())
}

/// Downloads all of them into a directory.
pub async fn download_many(
    client: std::sync::Arc<IgClient>,
    stem_base: String,
    items: Vec<Story>,
    numbers: Vec<usize>,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    let files = numbers
        .iter()
        .map(|&number| Named {
            number,
            stem: format!("{}-{number}", printable(&stem_base)),
            url: url_of(&items, number).map(str::to_string),
            what: format!("story {number}"),
            cap: MAX_STORY_BYTES,
            best: None,
            ffmpeg: None,
        })
        .collect();
    download_named(client, files, destination, "stories").await
}

/// One file of a [`download_named`]: the number it is reported under, the
/// name it is saved as (filtered, without its extension), where it is, what
/// a sentence calls it, and the ceiling on its size; and for a post's video,
/// its best tracks and the ffmpeg that joins them, when it is to be saved
/// from those ([`save_file`]).
#[derive(Debug, Clone)]
pub struct Named {
    pub number: usize,
    pub stem: String,
    pub url: Option<String>,
    pub what: String,
    pub cap: usize,
    pub best: Option<Best>,
    pub ffmpeg: Option<std::sync::Arc<Ffmpeg>>,
}

/// A video's widest track and its sound, from its DASH manifest, when they
/// are wider than its progressive file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Best {
    pub video: Track,
    pub audio: Option<Track>,
}

/// The ceiling on a video saved from its best tracks, the two together. The
/// largest best track of the capture of 2026-11-13 was 210.6 MB, past
/// [`crate::posts::MAX_POST_BYTES`]; both stream to disk, so the ceiling is
/// only there so that something endless cannot fill it.
pub const MAX_BEST_BYTES: u64 = 512 * 1024 * 1024;

impl Best {
    /// What the two tracks say they weigh, together.
    fn bytes(&self) -> u64 {
        let video = self.video.bytes.unwrap_or(0);
        let audio = self.audio.as_ref().and_then(|a| a.bytes).unwrap_or(0);
        video.saturating_add(audio)
    }
}

/// `file` saved into `dir`: from its best tracks, joined by ffmpeg, when it has
/// them and an ffmpeg to join them; from its progressive file otherwise, and
/// when the join fails, which is said on standard error.
pub async fn save_file(client: &IgClient, file: &Named, dir: &Path) -> Result<Saved> {
    let (saved, said) = save_file_saying(client, file, dir).await?;
    if let Some(said) = said {
        host::warn(&said);
    }
    Ok(saved)
}

/// [`save_file`], with what it would have said handed back rather than
/// written: a full-screen view draws on standard error, where a line written
/// in the middle of a frame breaks it, and shows the sentence in its own
/// note line instead.
pub(crate) async fn save_file_saying(
    client: &IgClient,
    file: &Named,
    dir: &Path,
) -> Result<(Saved, Option<String>)> {
    // Before a path is made of it, as `save_named` asks: a video's tracks
    // are looked for and fetched under the stem first.
    files::usable_stem(&file.stem)?;
    let (best, mut said) = joinable(file);
    if let Some(best) = best {
        if let Some(existing) = already_saved(dir, &file.stem) {
            return Ok((Saved::Already(existing), None));
        }
        sweep_stale_parts(dir, &file.stem);
        match joined(client, &file.stem, best, file.ffmpeg.as_deref(), dir).await {
            Ok(part) => {
                let placed = files::place_new(&part, dir, &file.stem, "mp4")?;
                return Ok((Saved::Now(placed), None));
            }
            // A stop, and a push-back: neither is a reason to go on to the
            // smaller file, which would be one more fetch after a refusal.
            Err(e) if stopped_the_run(&e) => return Err(e),
            Err(e) => said = Some(smaller(file, &format!("{e:#}"))),
        }
    }
    let saved = save_named(
        client,
        &file.stem,
        file.url.as_deref(),
        &file.what,
        file.cap,
        dir,
    )
    .await?;
    Ok((saved, said))
}

/// [`save_file`] into `path`, replacing it as `-o` does, but only with a
/// file that arrived whole.
pub async fn save_file_to(client: &IgClient, file: &Named, path: &Path) -> Result<()> {
    let (best, said) = joinable(file);
    if let Some(said) = said {
        host::warn(&said);
    }
    if let Some(best) = best {
        let dir = path
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        match joined(client, &file.stem, best, file.ffmpeg.as_deref(), dir).await {
            Ok(part) => {
                return std::fs::rename(&part, path).map_err(|e| {
                    let _ = std::fs::remove_file(&part);
                    anyhow::Error::new(e).context(format!("could not replace {}", path.display()))
                });
            }
            Err(e) if was_canceled(&e) => return Err(e),
            Err(e) => host::warn(&smaller(file, &format!("{e:#}"))),
        }
    }
    let url = file
        .url
        .as_deref()
        .ok_or_else(|| anyhow!("{} has no downloadable version", file.what))?;
    let mut out = files::Replacing::open(path)?;
    client.download_to(url, file.cap, out.file()).await?;
    out.commit()
}

/// The sentence for a video saved as the app's file because `why`.
fn smaller(file: &Named, why: &str) -> String {
    format!("{} is saved as the app's smaller file: {why}", file.what)
}

/// `file`'s best tracks, when they are to be what is saved: there are some,
/// the sound can be joined (or there is none to join), and they weigh no
/// more than [`MAX_BEST_BYTES`]; with the sentence that says so when they
/// do not, for the caller to say where it can.
pub(super) fn joinable(file: &Named) -> (Option<&Best>, Option<String>) {
    let Some(best) = file.best.as_ref() else {
        return (None, None);
    };
    if best.audio.is_some() && file.ffmpeg.is_none() {
        return (None, None);
    }
    if best.bytes() > MAX_BEST_BYTES {
        let why = format!(
            "its best tracks weigh {} MB, past {} MB",
            best.bytes() / 1_000_000,
            MAX_BEST_BYTES / 1_000_000
        );
        return (None, Some(smaller(file, &why)));
    }
    (Some(best), None)
}

/// The best tracks of `stem` fetched into `dir` and joined into one MP4,
/// answered as the finished `.part` the caller moves into place.
pub(super) async fn joined(
    client: &IgClient,
    stem: &str,
    best: &Best,
    ffmpeg: Option<&Ffmpeg>,
    dir: &Path,
) -> Result<PathBuf> {
    files::usable_stem(stem)?;
    let pid = std::process::id();
    let video = dir.join(format!("{stem}.{pid}.v.part"));
    let audio = dir.join(format!("{stem}.{pid}.a.part"));
    let out = dir.join(format!("{stem}.{pid}.part"));
    let cap = usize::try_from(MAX_BEST_BYTES).unwrap_or(usize::MAX);
    let result = async {
        fetch_track(client, &best.video.url, cap, &video).await?;
        let (Some(track), Some(ffmpeg)) = (best.audio.as_ref(), ffmpeg) else {
            // A video with no sound is its video track alone.
            std::fs::rename(&video, &out)
                .with_context(|| format!("could not move {}", video.display()))?;
            return Ok(());
        };
        fetch_track(client, &track.url, cap, &audio).await?;
        ffmpeg
            .join(&video, &audio, &out, client.pacer().cancel_token())
            .await
    }
    .await;
    let _ = std::fs::remove_file(&video);
    let _ = std::fs::remove_file(&audio);
    match result {
        Ok(()) => Ok(out),
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            Err(e)
        }
    }
}

/// One track from `url` into `part`, a file this creates, synced.
async fn fetch_track(client: &IgClient, url: &str, cap: usize, part: &Path) -> Result<()> {
    let mut file = files::create_new(part)?;
    client.download_track_to(url, cap, &mut file).await?;
    file.sync_all()
        .with_context(|| format!("could not finish writing {}", part.display()))
}

/// [`download_many`] for any set of files of the CDN, each under the name it
/// carries: into `destination`, three at a time, past the ones that fail,
/// said at the end as so many of the `noun`.
pub async fn download_named(
    client: std::sync::Arc<IgClient>,
    files: Vec<Named>,
    destination: Option<&Path>,
    noun: &str,
) -> Result<ExitCode> {
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let dir = destination.unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;

    // Several in flight rather than one after another.
    let dir = std::sync::Arc::new(dir.to_path_buf());
    let slots = std::sync::Arc::new(Semaphore::new(STORY_DOWNLOADS_IN_FLIGHT));
    let total = files.len();
    let mut tasks = JoinSet::new();
    for file in files {
        let (client, dir, slots) = (
            std::sync::Arc::clone(&client),
            std::sync::Arc::clone(&dir),
            std::sync::Arc::clone(&slots),
        );
        tasks.spawn(async move {
            // A closed semaphore is impossible here -- nothing closes it --
            // so the only way this fails is the runtime shutting down, and
            // then there is nobody to report to.
            let _slot = slots.acquire_owned().await.ok()?;
            let saved = save_file(&client, &file, &dir).await;
            Some((file.number, saved))
        });
    }

    let mut failed: Vec<(usize, String)> = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let Ok(Some((number, saved))) = joined else {
            // A task that panicked or was aborted: the runtime is going
            // down, or Ctrl+C already won below. Nothing to add.
            continue;
        };
        match saved {
            // Numbered, because they finish in whichever order the network
            // decides and the listing's numbers are what the user reads.
            Ok(Saved::Now(path)) => host::info(&format!("Saved {number}: {}", path.display())),
            Ok(Saved::Already(path)) => {
                host::info(&format!("Already saved {number}: {}", path.display()));
            }
            Err(e) if was_canceled(&e) => {
                // The token is shared, so every download still running has
                // already answered `Canceled` or is about to; aborting only
                // stops their reports from arriving after "stopped".
                tasks.abort_all();
                return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
            }
            Err(e) => failed.push((number, e.to_string())),
        }
    }

    if failed.is_empty() {
        return Ok(ExitCode::Ok);
    }
    failed.sort_by_key(|(number, _)| *number);
    let lines: Vec<String> = failed
        .iter()
        .map(|(number, why)| format!("{number}: {why}"))
        .collect();
    Err(ExitError::new(
        ExitCode::Error,
        format!(
            "{} of {} {noun} could not be downloaded:\n{}",
            failed.len(),
            total,
            lines.join("\n")
        ),
    )
    .into())
}

/// How many stories download at once under `-d all` or a set.
const STORY_DOWNLOADS_IN_FLIGHT: usize = 3;

/// Whether a failed download was the user stopping it.
pub fn was_canceled(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|cause| matches!(cause.downcast_ref::<IgError>(), Some(IgError::Canceled)))
}

/// Whether `e` ends the run whatever it was fetching, rather than failing one
/// file of several: the stop itself, a push-back of Instagram's, the cooldown
/// one started, and a wait past `--max-wait`.
///
/// **A loop over files that collects its failures asks this first.** A
/// push-back counted as one file that failed lets the files after it be asked
/// for, and the run end as a success, against "a hard stop on the first
/// push-back" (AGENTS.md).
pub(crate) fn ends_the_run(e: &IgError) -> bool {
    matches!(
        e,
        IgError::Canceled
            | IgError::WaitTooLong { .. }
            | IgError::InCooldown { .. }
            | IgError::RateLimited
            | IgError::FeedbackRequired
            | IgError::Challenge { .. }
            | IgError::Checkpoint { .. }
    )
}

/// [`ends_the_run`], for an error with its context around it.
pub fn stopped_the_run(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|cause| cause.downcast_ref::<IgError>().is_some_and(ends_the_run))
}

/// `text` with every address in it replaced by the word: a failure's
/// sentence as a program is handed it (`post -d --json`, read by `snob
/// mcp`). **An error of the network names the address it was fetching**
/// (`reqwest` appends "for url (...)"), which for a file of the CDN is the
/// signed address no agent is handed (`answer::without_media_addresses`
/// makes null only a string that is an address as a whole).
pub(crate) fn without_addresses(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            if word.contains("://") {
                "(an address)"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What `file`'s best tracks are declared to weigh when they are what a
/// save tries first ([`joinable`]), and 0 when it goes straight to the one
/// file: what a save that ends with the smaller file, or with none, may
/// have fetched and thrown away, for a caller that bounds what it fetches.
pub(crate) fn best_weight(file: &Named) -> u64 {
    joinable(file).0.map_or(0, Best::bytes)
}

/// The bytes of one story, from the CDN.
pub async fn bytes_of(client: &IgClient, story: &Story) -> Result<Vec<u8>> {
    let url = story
        .url
        .as_deref()
        .ok_or_else(|| anyhow!("Instagram described this story but offered no media for it"))?;
    Ok(client.download_capped(url, MAX_STORY_BYTES).await?)
}

/// What arrived, read from the bytes rather than from the URL.
pub fn extension_of(bytes: &[u8]) -> &'static str {
    let webp = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP");
    match bytes {
        _ if bytes.get(4..8) == Some(b"ftyp") => "mp4",
        _ if webp => "webp",
        [0x89, b'P', b'N', b'G', ..] => "png",
        _ => "jpg",
    }
}

/// `someone-3.mp4`, next to whatever is already there.
pub fn default_name(dir: &Path, username: &str, number: usize, extension: &str) -> Result<PathBuf> {
    files::default_path(dir, &format!("{}-{number}", printable(username)), extension)
}

/// The stem a story or a highlight's item is saved under when what became of
/// the download is told as a document ([`save_telling`]):
/// `<username>-<kind>-<pk>`.
///
/// **Named by what the item is, not by where it stands**: a tray's numbers
/// move as stories are added and expire, so a name built on one would later
/// be another story's, and a reader that comes back to the same directory
/// would take it for a file it has.
pub fn stable_stem(username: &str, kind: &str, pk: &str) -> String {
    let pk: String = pk.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("{}-{kind}-{pk}", printable(username))
}

/// What a download told as a document stops at ([`save_telling`],
/// `posts::save_telling`): the bytes it may fetch and the time it may take
/// before it starts no further file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bounds {
    pub bytes: Option<u64>,
    pub within: Option<std::time::Duration>,
}

impl Bounds {
    /// The bound a run that has fetched `fetched` bytes since `started` is
    /// past, as an item it leaves says why: `bytes` or `time`.
    pub(crate) fn passed(&self, fetched: u64, started: std::time::Instant) -> Option<&'static str> {
        if self.bytes.is_some_and(|most| fetched >= most) {
            Some("bytes")
        } else if self.within.is_some_and(|most| started.elapsed() >= most) {
            Some("time")
        } else {
            None
        }
    }
}

/// One file of a download told as a document ([`save_telling`]), with what
/// its listing says of it.
#[derive(Debug, Clone)]
pub struct Told {
    pub file: Named,
    pub pk: String,
    pub kind: Kind,
}

/// One file [`keep_listed`] saves, and what its row of the document says.
pub(crate) struct Listed {
    pub(crate) file: Named,
    pub(crate) pk: Option<String>,
    pub(crate) kind: Option<String>,
    /// What the best tracks weigh. Counted when they are fetched and the file
    /// kept is not them, and when the file fails after they may have been.
    /// `0` when the caller has none.
    pub(crate) tried: u64,
    /// The row carries `note`: how a video was saved.
    pub(crate) noted: bool,
}

/// What [`keep_listed`] kept, in the order the document prints it.
pub(crate) struct Listing {
    pub(crate) directory: std::path::PathBuf,
    pub(crate) saved: Vec<serde_json::Value>,
    pub(crate) failed: Vec<serde_json::Value>,
    pub(crate) left: Vec<serde_json::Value>,
    pub(crate) bytes: u64,
}

/// The loop [`save_telling`] and a post's own telling share: each
/// file in order, a rest of `bounds` stopping the ones after it, a push-back
/// ending the run. A row's keys are `item`, `pk`, `kind`, `file`, `bytes`,
/// `already`, and `note` when [`Listed::noted`].
pub(crate) async fn keep_listed(
    client: &IgClient,
    files: Vec<Listed>,
    dir: &Path,
    bounds: Bounds,
) -> Result<Listing> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let directory = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let started = std::time::Instant::now();
    let (mut saved, mut failed, mut left) = (Vec::new(), Vec::new(), Vec::new());
    let mut fetched: u64 = 0;
    for Listed {
        file,
        pk,
        kind,
        tried,
        noted,
    } in files
    {
        if client.pacer().cancel_token().is_canceled() {
            return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
        }
        if let Some(why) = bounds.passed(fetched, started) {
            left.push(serde_json::json!({ "item": file.number, "why": why }));
            continue;
        }
        let kept = if noted {
            save_file_saying(client, &file, &directory).await
        } else {
            save_file(client, &file, &directory)
                .await
                .map(|kept| (kept, None))
        };
        match kept {
            Ok((kept, note)) => {
                let (path, already) = match kept {
                    Saved::Now(path) => (path, false),
                    Saved::Already(path) => (path, true),
                };
                let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                if !already {
                    fetched = fetched.saturating_add(bytes);
                    // A note is the best tracks given up for the smaller file:
                    // both were fetched.
                    if note.is_some() {
                        fetched = fetched.saturating_add(tried);
                    }
                }
                let mut row = serde_json::json!({
                    "item": file.number,
                    "pk": pk,
                    "kind": kind,
                    "file": path.file_name().map(|name| name.to_string_lossy().into_owned()),
                    "bytes": bytes,
                    "already": already,
                });
                if noted && let Some(row) = row.as_object_mut() {
                    let note = note.map(|note| without_addresses(&note));
                    row.insert("note".into(), serde_json::json!(note));
                }
                saved.push(row);
            }
            Err(e) if was_canceled(&e) => {
                return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
            }
            // A push-back, or the cooldown it started: the run ends, with no
            // further file asked for.
            Err(e) if stopped_the_run(&e) => return Err(e),
            Err(e) => {
                // What it may have fetched before it failed counts too.
                fetched = fetched.saturating_add(tried);
                failed.push(serde_json::json!({
                    "item": file.number,
                    "why": without_addresses(&e.to_string()),
                }));
            }
        }
    }
    Ok(Listing {
        directory,
        saved,
        failed,
        left,
        bytes: fetched,
    })
}

/// Saves `files` into the directory `dir`, one after another, and answers with
/// what became of each, as the keys of a document the caller completes with
/// what it is about, and with how many failed:
///
/// - `directory`: where the files are, absolute;
/// - `saved`: each file there, by its number in the listing: its `file` name,
///   its `bytes`, its `kind` and its `pk`, and `already` when an earlier run
///   had saved it and nothing was fetched;
/// - `failed`: the items that could not be fetched, each with `why`;
/// - `left`: the items not started because a bound of `bounds` was reached,
///   each with `why` (`bytes` or `time`): asking again saves them, since those
///   already there are not fetched twice;
/// - `bytes`: what this run fetched.
///
/// For a caller that reads the answer rather than a directory (`stories -d
/// --json`, which `snob mcp` runs for an agent); `posts::save_telling` is the
/// same for a post's items.
pub async fn save_telling(
    client: &IgClient,
    files: Vec<Told>,
    dir: &Path,
    bounds: Bounds,
) -> Result<(serde_json::Map<String, serde_json::Value>, usize)> {
    let files = files
        .into_iter()
        .map(|Told { file, pk, kind }| Listed {
            file,
            pk: Some(pk),
            kind: Some(kind.label().to_string()),
            tried: 0,
            noted: false,
        })
        .collect();
    let kept = keep_listed(client, files, dir, bounds).await?;
    let failures = kept.failed.len();
    let mut document = serde_json::Map::new();
    document.insert("directory".into(), kept.directory.to_string_lossy().into());
    document.insert("saved".into(), kept.saved.into());
    document.insert("failed".into(), kept.failed.into());
    document.insert("left".into(), kept.left.into());
    document.insert("bytes".into(), kept.bytes.into());
    Ok((document, failures))
}
