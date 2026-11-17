//! Stories and highlight items once they are read (`stories`, `highlights`),
//! and how they are kept: the words a listing and a view both use for them,
//! and the downloads from the CDN (`save`). `profile` and every view share it.

use anyhow::Result;
use snob_core::Epoch;
use snob_core::model::printable;
use snob_ig::client::IgClient;
use snob_ig::error::IgError;
use snob_ig::model::{PictureVersion, ReelItem, largest};

use crate::exit::{ExitCode, ExitError};
use crate::sentences;

#[cfg(test)]
use crate::ffmpeg::Ffmpeg;
#[cfg(test)]
use snob_ig::model::dash::Track;

mod save;
pub use save::{
    Best, Bounds, MAX_BEST_BYTES, Named, Saved, Told, bytes_of, default_name, download_many,
    download_named, extension_of, save_file, save_file_to, save_story, save_telling, stable_stem,
    stopped_the_run, was_canceled,
};
pub(crate) use save::{
    Listed, best_weight, ends_the_run, keep_listed, save_file_saying, save_named,
};
#[cfg(test)]
use save::{joinable, joined, url_of, without_addresses};

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

/// The wire item, reduced.
pub fn story_from(item: &ReelItem) -> Story {
    let kind = Kind::of(item.media_type);
    let video = largest(&item.video_versions).map(|v| v.url.clone());
    let image = item
        .image_versions2
        .as_ref()
        .and_then(|c| largest(&c.candidates))
        .map(|v| v.url.clone());

    Story {
        pk: printable(&item.pk),
        kind,
        taken_at: item.taken_at,
        expiring_at: item.expiring_at,
        url: match kind {
            Kind::Video => video.or(image),
            _ => image.or(video),
        },
        mentions: item.mentioned().map(printable).collect(),
        thumb: item
            .image_versions2
            .as_ref()
            .and_then(|c| thumb_among(&c.candidates)),
    }
}

/// Everything the story listing needs, gathered before anything is printed.
#[derive(Debug, Clone)]
pub struct Stories {
    /// As Instagram spells it, not as it was typed.
    pub username: String,
    pub items: Vec<Story>,
}

/// The most one small picture may weigh ([`small_pictures`]): half a megabyte.
pub const MAX_SMALL_PICTURE: usize = 512 * 1024;

/// The most an account's largest picture may weigh: 1080 pixels square is
/// some hundreds of kilobytes, and this leaves room for any.
pub const MAX_LARGE_PICTURE: usize = 16 * 1024 * 1024;

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
pub fn picture_type(bytes: &[u8]) -> Option<&'static str> {
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

/// Twelve hex digits of the SHA-256 of `bytes`: what tells one picture from
/// another in a file's name (`pfp --json`), so a changed picture is a new file
/// and one already saved is not saved again.
pub fn short_digest(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    Sha256::digest(bytes)
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    use snob_ig::model::Candidates;

    /// A story's thumbnail is the narrowest of its pictures wide enough
    /// for a view, a video's cover included, and never the video's file.
    #[test]
    fn a_storys_thumbnail_is_its_narrowest_picture_wide_enough() {
        let item: ReelItem = serde_json::from_value(serde_json::json!({
            "pk": "1", "media_type": 2,
            "image_versions2": {"candidates": [
                {"url": "https://x.cdninstagram.com/1080.jpg", "width": 1080},
                {"url": "https://x.cdninstagram.com/720.jpg", "width": 720},
                {"url": "https://x.cdninstagram.com/480.jpg", "width": 480},
                {"url": "https://x.cdninstagram.com/240.jpg", "width": 240},
            ]},
            "video_versions": [{"url": "https://x.cdninstagram.com/v.mp4", "width": 720}],
        }))
        .unwrap();
        let story = story_from(&item);
        assert_eq!(
            story.url.as_deref(),
            Some("https://x.cdninstagram.com/v.mp4")
        );
        assert_eq!(
            story.thumb.as_deref(),
            Some("https://x.cdninstagram.com/720.jpg")
        );

        let bare: ReelItem = serde_json::from_value(serde_json::json!({"pk": "2"})).unwrap();
        assert_eq!(story_from(&bare).thumb, None);
    }

    fn version(url: &str, w: u32, h: u32) -> PictureVersion {
        PictureVersion {
            url: url.into(),
            width: Some(w),
            height: Some(h),
        }
    }

    fn item(media_type: u8, images: Vec<PictureVersion>, videos: Vec<PictureVersion>) -> ReelItem {
        ReelItem {
            pk: "1".into(),
            media_type,
            taken_at: Epoch::new(1_000),
            expiring_at: Some(Epoch::new(2_000)),
            image_versions2: Some(Candidates { candidates: images }),
            video_versions: videos,
            reel_mentions: Vec::new(),
            story_bloks_stickers: Vec::new(),
        }
    }

    #[test]
    fn the_biggest_version_wins_not_the_first() {
        let story = story_from(&item(
            1,
            vec![version("small", 320, 320), version("big", 1080, 1920)],
            vec![],
        ));
        assert_eq!(story.url.as_deref(), Some("big"));
    }

    /// A video item carries a poster frame in `image_versions2`. Taking the
    /// first URL of either list would hand back the poster for a video, which
    /// is a picture where a video was asked for.
    #[test]
    fn a_video_prefers_its_video_over_its_poster_frame() {
        let story = story_from(&item(
            2,
            vec![version("poster", 1080, 1920)],
            vec![version("clip", 720, 1280)],
        ));
        assert_eq!(story.kind, Kind::Video);
        assert_eq!(story.url.as_deref(), Some("clip"));
    }

    /// And falls back to it rather than to nothing, because a poster is more
    /// use than a refusal.
    #[test]
    fn a_video_with_no_video_falls_back_to_the_poster() {
        let story = story_from(&item(2, vec![version("poster", 1080, 1920)], vec![]));
        assert_eq!(story.url.as_deref(), Some("poster"));
    }

    /// A media type nobody has seen before is downloadable and honestly
    /// labeled, rather than called a photo and given a `.jpg`.
    #[test]
    fn an_unknown_media_type_is_not_guessed_at() {
        let story = story_from(&item(9, vec![version("something", 100, 100)], vec![]));
        assert_eq!(story.kind, Kind::Unknown);
        assert_eq!(story.kind.label(), "unknown");
        assert_eq!(story.url.as_deref(), Some("something"));
    }

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
