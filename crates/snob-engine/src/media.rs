//! Stories and highlight items once they are read, and how they are kept:
//! the words a listing and a view both use for them, and the downloads from
//! the CDN. `stories`, `highlights`, `profile` and their views all share it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use snob_core::Epoch;
use snob_core::model::printable;
use snob_ig::client::IgClient;
use snob_ig::error::IgError;
use snob_ig::model::PictureVersion;
use snob_ig::model::dash::Track;

use crate::exit::{ExitCode, ExitError};
use crate::ffmpeg::Ffmpeg;
use crate::files::{self, ABANDONED_AFTER};
use crate::host;
use crate::sentences;

/// Ceiling on one downloaded story.
///
/// Higher than the profile-picture ceiling, and it has to be: a fifteen-second
/// story video at Instagram's own bitrate lands in the single-digit megabytes
/// and the 8 MB cap on `IgClient::download` refuses some of them.
pub const MAX_STORY_BYTES: usize = 64 * 1024 * 1024;

/// A photo or a video, as Instagram's `media_type` integer means it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Photo,
    Video,
    Unknown,
}

impl Kind {
    pub fn of(media_type: u8) -> Self {
        match media_type {
            1 => Self::Photo,
            2 => Self::Video,
            _ => Self::Unknown,
        }
    }

    /// What the printed listings and the views write for it: different
    /// shapes, and the words in them are one decision.
    pub fn label(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Unknown => "unknown",
        }
    }
}

/// One story or highlight item, reduced to what a listing and a download need.
#[derive(Debug, Clone)]
pub struct Story {
    /// Instagram's id for the item, which keeps it apart from every other
    /// however the tray changes. Filtered: it came off somebody else's profile.
    pub pk: String,
    pub kind: Kind,
    pub taken_at: Epoch,
    pub expiring_at: Option<Epoch>,
    /// Where the best copy is.
    pub url: Option<String>,
    /// Accounts tagged in it, already filtered for a terminal.
    pub mentions: Vec<String>,
    /// A small picture of it for a view to draw: a photo's own, a video's
    /// cover ([`thumb_among`]). Not in the JSON contract.
    pub thumb: Option<String>,
}

/// The narrowest picture at least this wide is a thumbnail. The views draw
/// it in real pixels where the terminal can (`ui::graphics`), and the panel
/// beside a post's text reaches some 600 pixels wide, so a picture of 320
/// was drawn blown up and looked pixelated (Dennis, 2026-11-13); 640 fills
/// it and is still a fraction of the full picture's bytes.
const THUMB_WIDTH: u32 = 640;

/// The picture a view draws small, of the `candidates` Instagram offers for one
/// item (a video's are its cover): the narrowest at least [`THUMB_WIDTH`] wide,
/// and when none is, the widest; on a tie, the one listed first.
pub fn thumb_among(candidates: &[PictureVersion]) -> Option<String> {
    let width = |v: &&PictureVersion| v.width.unwrap_or(0);
    candidates
        .iter()
        .filter(|v| width(v) >= THUMB_WIDTH)
        .min_by_key(width)
        .or_else(|| candidates.iter().rev().max_by_key(width))
        .map(|v| v.url.clone())
}

/// Everything the story listing needs, gathered before anything is printed.
#[derive(Debug, Clone)]
pub struct Stories {
    /// As Instagram spells it, not as it was typed.
    pub username: String,
    pub items: Vec<Story>,
}

/// The most one small picture may weigh ([`small_pictures`]): half a megabyte.
pub(crate) const MAX_SMALL_PICTURE: usize = 512 * 1024;

/// The most the small pictures of one listing weigh together: four megabytes,
/// three pages of a grid at a hundred kilobytes a picture.
pub(crate) const MOST_SMALL_PICTURES: usize = 4 * 1024 * 1024;

/// A small picture of each of `wanted`, a listing's things by their number in
/// it and the address of each one's thumbnail (a video's is its cover), for a
/// reader that looks rather than saves: `snob mcp`, which hands them to a model
/// as images (`--small-pictures`).
///
/// **In the JSON, never on standard output as bytes**, and never an address:
/// what is handed on is the picture, which names nobody and expires nowhere.
pub async fn small_pictures(
    client: &IgClient,
    wanted: Vec<(usize, Option<String>)>,
) -> Result<Vec<serde_json::Value>> {
    use base64::Engine as _;

    let mut pictures = Vec::with_capacity(wanted.len());
    let mut total = 0usize;
    for (of, address) in wanted {
        let missing = |why: &str| serde_json::json!({ "of": of, "missing": why });
        let Some(address) = address else {
            pictures.push(missing("none is offered"));
            continue;
        };
        if total >= MOST_SMALL_PICTURES {
            pictures.push(missing("past what one answer carries"));
            continue;
        }
        match client.download_capped(&address, MAX_SMALL_PICTURE).await {
            Ok(bytes) => match picture_type(&bytes) {
                Some(mime) => {
                    total += bytes.len();
                    pictures.push(serde_json::json!({
                        "of": of,
                        "mime": mime,
                        "bytes": bytes.len(),
                        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
                    }));
                }
                None => pictures.push(missing("what came is not a picture")),
            },
            Err(IgError::Canceled) => {
                return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
            }
            // A push-back heard while a picture was fetched by the account's
            // tab, or the cooldown one started: the run ends there, as any
            // run does, and no further picture is asked for.
            Err(e) if ends_the_run(&e) => return Err(e.into()),
            // Not the error's own words: they can hold the address.
            Err(_) => pictures.push(missing("it could not be fetched")),
        }
    }
    Ok(pictures)
}

/// What kind of picture `bytes` are, by how they start: the three kinds
/// Instagram serves. `None` for anything else, a video among them.
pub(crate) fn picture_type(bytes: &[u8]) -> Option<&'static str> {
    let webp = bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP");
    match bytes {
        [0xff, 0xd8, 0xff, ..] => Some("image/jpeg"),
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        _ if webp => Some("image/webp"),
        _ => None,
    }
}

/// A listing's JSON document with `pictures` beside what it lists
/// ([`small_pictures`]).
pub fn with_pictures(text: String, pictures: Option<Vec<serde_json::Value>>) -> Result<String> {
    let Some(pictures) = pictures else {
        return Ok(text);
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(mut document)) => {
            document.insert("pictures".into(), serde_json::Value::Array(pictures));
            Ok(serde_json::to_string_pretty(&document)?)
        }
        _ => Ok(text),
    }
}

/// The accounts a story tags, `@a @b`, for a table cell.
pub fn mentions_of(story: &Story) -> String {
    story
        .mentions
        .iter()
        .map(|m| format!("@{m}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// When a story was posted, and what is left of it: the two time columns of
/// the printed listing and of the browser, read off the same clock code.
pub fn posted_of(story: &Story) -> String {
    sentences::stored_on(story.taken_at)
}

pub fn left_of(story: &Story) -> String {
    remaining(story.expiring_at)
}

/// How long a story has left, or nothing when Instagram did not say.
fn remaining(expiring_at: Option<Epoch>) -> String {
    let Some(at) = expiring_at else {
        return "-".into();
    };
    countdown(at - snob_core::clock::now())
}

/// "7h 23m", "12m", "expired".
///
/// Not `snob_core::duration::format`, which exists to write a duration back
/// into the monitor's configuration file the way somebody would have typed it —
/// so it uses the largest unit that divides **exactly** and falls back to
/// seconds.
fn countdown(seconds: i64) -> String {
    if seconds <= 0 {
        return "expired".into();
    }
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    match (hours, minutes) {
        (0, 0) => "under a minute".into(),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

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
fn url_of(items: &[Story], number: usize) -> Option<&str> {
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
fn joinable(file: &Named) -> (Option<&Best>, Option<String>) {
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
async fn joined(
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

/// One file of a download told as a document ([`save_telling`]), with what
/// its listing says of it.
#[derive(Debug, Clone)]
pub struct Told {
    pub file: Named,
    pub pk: String,
    pub kind: Kind,
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
    bounds: crate::posts::Bounds,
) -> Result<(serde_json::Map<String, serde_json::Value>, usize)> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let directory = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let started = std::time::Instant::now();
    let (mut saved, mut failed, mut left) = (Vec::new(), Vec::new(), Vec::new());
    let mut fetched: u64 = 0;
    for Told { file, pk, kind } in files {
        if client.pacer().cancel_token().is_canceled() {
            return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
        }
        let past = if bounds.bytes.is_some_and(|most| fetched >= most) {
            Some("bytes")
        } else if bounds.within.is_some_and(|most| started.elapsed() >= most) {
            Some("time")
        } else {
            None
        };
        if let Some(why) = past {
            left.push(serde_json::json!({ "item": file.number, "why": why }));
            continue;
        }
        match save_file(client, &file, &directory).await {
            Ok(kept) => {
                let (path, already) = match kept {
                    Saved::Now(path) => (path, false),
                    Saved::Already(path) => (path, true),
                };
                let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                if !already {
                    fetched = fetched.saturating_add(bytes);
                }
                saved.push(serde_json::json!({
                    "item": file.number,
                    "pk": pk,
                    "kind": kind.label(),
                    "file": path.file_name().map(|name| name.to_string_lossy().into_owned()),
                    "bytes": bytes,
                    "already": already,
                }));
            }
            Err(e) if was_canceled(&e) => {
                return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
            }
            // A push-back, or the cooldown it started: the run ends, with no
            // further file asked for.
            Err(e) if stopped_the_run(&e) => return Err(e),
            Err(e) => failed.push(serde_json::json!({
                "item": file.number,
                "why": without_addresses(&e.to_string()),
            })),
        }
    }
    let failures = failed.len();
    let mut document = serde_json::Map::new();
    document.insert("directory".into(), directory.to_string_lossy().into());
    document.insert("saved".into(), saved.into());
    document.insert("failed".into(), failed.into());
    document.insert("left".into(), left.into());
    document.insert("bytes".into(), fetched.into());
    Ok((document, failures))
}

/// A picture of `bytes`, as an entry of a document's `pictures`
/// ([`small_pictures`]) for a reader that looks: `of` is the number it goes
/// by, and one that is no picture, or weighs more than a listing's small
/// picture may ([`MAX_SMALL_PICTURE`]), is a `missing` entry that says so.
pub fn picture_entry(of: usize, bytes: &[u8]) -> serde_json::Value {
    use base64::Engine as _;

    let Some(mime) = picture_type(bytes) else {
        return serde_json::json!({ "of": of, "missing": "what came is not a picture" });
    };
    if bytes.len() > MAX_SMALL_PICTURE {
        return serde_json::json!({
            "of": of,
            "missing": "it is larger than an answer carries; it can be saved instead",
        });
    }
    serde_json::json!({
        "of": of,
        "mime": mime,
        "bytes": bytes.len(),
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(url: &str, bytes: Option<u64>) -> Track {
        Track {
            url: url.into(),
            width: Some(1080),
            height: Some(1920),
            bandwidth: None,
            codecs: None,
            bytes,
        }
    }

    fn file(best: Option<Best>, ffmpeg: bool) -> Named {
        Named {
            number: 1,
            stem: "a-b".into(),
            url: Some("https://x.cdninstagram.com/p.mp4".into()),
            what: "item 1".into(),
            cap: 1,
            best,
            ffmpeg: ffmpeg.then(|| {
                std::sync::Arc::new(Ffmpeg::find(Some(&std::env::current_exe().unwrap())).unwrap())
            }),
        }
    }

    /// Best tracks are what is saved only when their sound can be joined
    /// (or there is none) and they weigh no more than the ceiling, which is
    /// known before a byte is fetched.
    #[test]
    fn a_track_past_the_ceiling_falls_back_before_a_byte_is_fetched() {
        let pair = |video: u64, audio: u64| Best {
            video: track("https://x.fbcdn.net/v", Some(video)),
            audio: Some(track("https://x.fbcdn.net/a", Some(audio))),
        };
        assert!(joinable(&file(Some(pair(30, 20)), true)).0.is_some());
        let heavy = file(Some(pair(MAX_BEST_BYTES, 1)), true);
        let (best, said) = joinable(&heavy);
        assert!(best.is_none());
        assert!(said.is_some_and(|s| s.contains("past 536 MB")), "said why");
        // A sound with nothing to join it.
        assert!(joinable(&file(Some(pair(30, 20)), false)).0.is_none());
        // No sound needs no ffmpeg.
        let silent = Best {
            video: track("https://x.fbcdn.net/v", None),
            audio: None,
        };
        assert!(joinable(&file(Some(silent), false)).0.is_some());
        assert_eq!(joinable(&file(None, true)), (None, None));
    }

    #[test]
    fn the_extension_comes_from_the_bytes() {
        let mut mp4 = vec![0, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypmp42");
        assert_eq!(extension_of(&mp4), "mp4");
        assert_eq!(extension_of(b"\x89PNG\r\n\x1a\n"), "png");
        assert_eq!(extension_of(b"RIFF\0\0\0\0WEBPVP8 "), "webp");
        assert_eq!(extension_of(b"\xff\xd8\xff\xe0anything"), "jpg");
        // A PNG signature cut short after five bytes is still a PNG, and
        // unrecognizable bytes get the usual case rather than no name at all.
        assert_eq!(extension_of(&[0x89, b'P', b'N', b'G', 0x0D]), "png");
        assert_eq!(extension_of(b"???"), "jpg");
    }

    #[test]
    fn an_unknown_expiry_is_not_reported_as_expired() {
        assert_eq!(remaining(None), "-");
    }

    /// A stem off the server that is no file name is refused before a path
    /// is built from it: an earlier copy is not looked for outside the
    /// directory (here one is waiting there, and was handed back as saved),
    /// and no `.part` is made anywhere.
    #[tokio::test]
    async fn a_stem_that_is_no_file_name_is_refused_before_any_path_is_built() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("downloads");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(tmp.path().join("escaped-1.jpg"), b"theirs").unwrap();

        let session = snob_core::session::Session::from_sessionid(
            "42%3Aabc%3A1",
            "Mozilla/5.0",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        let client = IgClient::new(session, snob_ig::pace::Pacer::unlimited()).unwrap();

        // No address, so nothing could be sent whatever the order of the
        // steps; what is under test is what is looked at and made first.
        let saved = save_named(&client, "../escaped-1", None, "story 1", 1024, &dir).await;
        let error = saved.err().expect("a stem with a separator was used");
        assert!(
            error.to_string().contains("cannot be used as a file name"),
            "{error}"
        );

        let left: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(
            left.len(),
            2,
            "something was made beside the directory: {left:?}"
        );
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// The same for a video saved from its best tracks, which are looked for
    /// and fetched under the stem before the one file is: an earlier copy
    /// outside the directory was handed back as saved, and the tracks'
    /// scratch files were made there.
    #[tokio::test]
    async fn a_videos_tracks_are_not_fetched_under_a_stem_that_is_no_file_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("downloads");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(tmp.path().join("escaped-1.mp4"), b"theirs").unwrap();

        let session = snob_core::session::Session::from_sessionid(
            "42%3Aabc%3A1",
            "Mozilla/5.0",
            snob_core::session::SessionOrigin::Paste,
        )
        .unwrap();
        let client = IgClient::new(session, snob_ig::pace::Pacer::unlimited()).unwrap();
        // No host of the CDN's, so nothing is sent whatever is tried.
        let silent = Best {
            video: track("https://cdn.invalid/v", None),
            audio: None,
        };
        let named = Named {
            stem: "../escaped-1".into(),
            url: None,
            ..file(Some(silent.clone()), false)
        };

        let saved = save_file_saying(&client, &named, &dir).await;
        let error = saved.err().expect("a stem with a separator was used");
        assert!(
            error.to_string().contains("cannot be used as a file name"),
            "{error}"
        );
        let fetched = joined(&client, &named.stem, &silent, None, &dir).await;
        let error = fetched.expect_err("the tracks were fetched under it");
        assert!(
            error.to_string().contains("cannot be used as a file name"),
            "{error}"
        );

        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    /// A story or an item named by a number the listing does not hold is
    /// one with no file, not an index past the end.
    #[test]
    fn a_number_past_the_listing_names_no_file() {
        let story = Story {
            pk: "1".into(),
            kind: Kind::Photo,
            taken_at: Epoch::default(),
            expiring_at: None,
            url: Some("https://x.cdninstagram.com/s.jpg".into()),
            mentions: Vec::new(),
            thumb: None,
        };
        let items = [story];
        assert_eq!(url_of(&items, 1), Some("https://x.cdninstagram.com/s.jpg"));
        assert_eq!(url_of(&items, 0), None);
        assert_eq!(url_of(&items, 2), None);
    }

    /// A push-back is not one file that failed: a loop that collects its
    /// failures stops on it, and on nothing a single file can do.
    #[test]
    fn a_push_back_ends_the_run_and_a_file_that_failed_does_not() {
        assert!(ends_the_run(&IgError::RateLimited));
        assert!(ends_the_run(&IgError::FeedbackRequired));
        assert!(ends_the_run(&IgError::Canceled));
        let wrapped = anyhow::Error::new(IgError::RateLimited).context("could not save 3");
        assert!(stopped_the_run(&wrapped));
        assert!(!stopped_the_run(&anyhow::anyhow!("no space left")));
    }

    /// A failure's sentence handed to a program names no address: an error
    /// of the network ends in the signed one it was fetching.
    #[test]
    fn a_failure_told_to_a_program_names_no_address() {
        let said =
            "error sending request for url (https://scontent.cdninstagram.com/v/a.mp4?sig=abc)";
        assert_eq!(
            without_addresses(said),
            "error sending request for url (an address)"
        );
        assert_eq!(without_addresses("no space left"), "no space left");
    }
}
