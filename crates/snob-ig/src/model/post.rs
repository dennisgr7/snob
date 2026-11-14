//! Models of a profile's posts, one post, its comments, replies and likers,
//! as the web app reads them. The grid and the info read describe a post with
//! the same fields, so one type reads both. Every field but the pk is
//! optional, and `null` reads as empty.

use serde::Deserialize;
use snob_core::Epoch;

use super::web::Connection;
use super::{Candidates, PictureVersion, flexible_id, flexible_ids, nullable};
use crate::shortcode::MediaPk;

/// A post, a reel, or one item of a carousel.
#[derive(Debug, Clone, Deserialize)]
pub struct Media {
    pub pk: MediaPk,
    /// Eleven characters, or 39 for a private account's post.
    #[serde(default)]
    pub code: Option<String>,
    /// 1 a photo, 2 a video, 8 a carousel.
    #[serde(default)]
    pub media_type: u8,
    /// `feed`, `carousel_container`, `clips` (a reel), `carousel_item`.
    #[serde(default)]
    pub product_type: Option<String>,
    #[serde(default)]
    pub taken_at: Epoch,
    #[serde(default)]
    pub caption: Option<Caption>,
    #[serde(default)]
    pub like_count: Option<u64>,
    #[serde(default)]
    pub comment_count: Option<u64>,
    /// A reel's plays. The grid's answer leaves it out.
    #[serde(default)]
    pub play_count: Option<u64>,
    #[serde(default)]
    pub ig_play_count: Option<u64>,
    /// The owner hid the counts: the likes are then not said as a number.
    #[serde(default)]
    pub like_and_view_counts_disabled: Option<bool>,
    /// "Liked by <name> and others": the viewer's contacts who liked it, as
    /// names. At most one in the capture of 2026-11-13.
    #[serde(default, deserialize_with = "nullable")]
    pub top_likers: Vec<String>,
    /// The same contacts as accounts: none to three in the capture of
    /// 2026-11-13, `null` on some posts, some entries with no name.
    #[serde(default, deserialize_with = "nullable")]
    pub facepile_top_likers: Vec<Account>,
    #[serde(default)]
    pub media_repost_count: Option<u64>,
    /// A reel's shares, absent when the owner turned the count off.
    #[serde(default)]
    pub reshare_count: Option<u64>,
    #[serde(default)]
    pub share_count_disabled: Option<bool>,
    /// A reel's plays on Facebook, beside [`Self::ig_play_count`].
    #[serde(default)]
    pub fb_play_count: Option<u64>,
    /// Seconds.
    #[serde(default)]
    pub video_duration: Option<f64>,
    #[serde(default)]
    pub has_audio: Option<bool>,
    /// A video's DASH manifest, an MPD as text: its renditions, the widest
    /// wider than [`Self::video_versions`] on most videos.
    #[serde(default)]
    pub video_dash_manifest: Option<String>,
    /// Per item on a carousel.
    #[serde(default)]
    pub accessibility_caption: Option<String>,
    /// The accounts whose grid it is pinned to the top of, by pk: its owner's
    /// or a co-author's (20 lists of 20 in the capture of 2026-11-13).
    #[serde(default, deserialize_with = "flexible_ids")]
    pub timeline_pinned_user_ids: Vec<String>,
    #[serde(default)]
    pub is_paid_partnership: Option<bool>,
    #[serde(default)]
    pub caption_is_edited: Option<bool>,
    /// A reel's sound: licensed music, or the video's own.
    #[serde(default)]
    pub clips_metadata: Option<ClipsMetadata>,
    /// The music of a photo or a carousel; `null` on a reel.
    #[serde(default)]
    pub music_metadata: Option<MusicMetadata>,
    #[serde(default)]
    pub usertags: Option<Usertags>,
    /// A collaboration's other authors.
    #[serde(default, deserialize_with = "nullable")]
    pub coauthor_producers: Vec<Account>,
    /// The owner.
    #[serde(default)]
    pub user: Option<Account>,
    #[serde(default)]
    pub location: Option<Location>,
    #[serde(default)]
    pub image_versions2: Option<Candidates>,
    /// A video's progressive files: one MP4 with its sound, 720 wide at most
    /// in the captures. Higher resolutions are only in `video_dash_manifest`.
    #[serde(default, deserialize_with = "nullable")]
    pub video_versions: Vec<PictureVersion>,
    #[serde(default, deserialize_with = "nullable")]
    pub carousel_media: Vec<Media>,
    #[serde(default)]
    pub carousel_media_count: Option<u32>,
    #[serde(default)]
    pub original_width: Option<u32>,
    #[serde(default)]
    pub original_height: Option<u32>,
    /// The notes written on it where it was reposted: up to three on an item
    /// of a reposts tab (capture of 2026-11-14), `null` elsewhere.
    #[serde(default)]
    pub media_notes: Option<MediaNotes>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaNotes {
    #[serde(default, deserialize_with = "nullable")]
    pub items: Vec<MediaNote>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaNote {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Caption {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Usertags {
    #[serde(default, rename = "in", deserialize_with = "nullable")]
    pub tagged: Vec<Usertag>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Usertag {
    #[serde(default)]
    pub user: Option<Account>,
}

/// An account named on a post: owner, tagged, co-author, liker or commenter.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Account {
    /// Text in most answers, a number in some nested ones (capture of
    /// 2026-11-13).
    #[serde(default, deserialize_with = "flexible_id")]
    pub pk: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub is_private: Option<bool>,
    #[serde(default)]
    pub is_verified: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Location {
    #[serde(default, deserialize_with = "flexible_id")]
    pub pk: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub lat: Option<f64>,
    #[serde(default)]
    pub lng: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClipsMetadata {
    /// `licensed_music` or `original_sounds`; absent on some reels, which
    /// are told apart by which of the two below is filled.
    #[serde(default)]
    pub audio_type: Option<String>,
    #[serde(default)]
    pub music_info: Option<MusicInfo>,
    #[serde(default)]
    pub original_sound_info: Option<OriginalSound>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MusicMetadata {
    #[serde(default)]
    pub audio_type: Option<String>,
    #[serde(default)]
    pub music_info: Option<MusicInfo>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MusicInfo {
    #[serde(default)]
    pub music_asset_info: Option<MusicAsset>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MusicAsset {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub display_artist: Option<String>,
    #[serde(default)]
    pub ig_username: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OriginalSound {
    #[serde(default)]
    pub original_audio_title: Option<String>,
    #[serde(default)]
    pub ig_artist: Option<Account>,
}

/// What a post plays, whichever of the two places said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Music {
    /// `false` for a video's own sound.
    pub licensed: bool,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artist_username: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub pk: Option<String>,
    pub username: String,
}

/// The most likers the app names under a post: three faces in the capture of
/// 2026-11-13.
pub const FACEPILE_SHOWN: usize = 3;

impl Media {
    /// A carousel's items, or the post itself when it is one photo or video.
    pub fn items(&self) -> Vec<&Media> {
        if self.carousel_media.is_empty() {
            vec![self]
        } else {
            self.carousel_media.iter().collect()
        }
    }

    /// The best copy of this one item: a video's widest progressive file,
    /// otherwise the largest picture. `None` when the answer offered none.
    pub fn best_url(&self) -> Option<&str> {
        widest(&self.video_versions).or_else(|| {
            self.image_versions2
                .as_ref()
                .and_then(|c| widest(&c.candidates))
        })
    }

    pub fn is_video(&self) -> bool {
        self.media_type == 2 || !self.video_versions.is_empty()
    }

    pub fn caption_text(&self) -> &str {
        self.caption
            .as_ref()
            .and_then(|c| c.text.as_deref())
            .unwrap_or_default()
    }

    pub fn owner(&self) -> Option<&str> {
        self.user.as_ref().and_then(|u| u.username.as_deref())
    }

    /// The accounts tagged on it or on any of its items, each once, in order.
    pub fn tagged(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let all = std::iter::once(self).chain(self.carousel_media.iter());
        for media in all {
            let tags = media.usertags.iter().flat_map(|t| t.tagged.iter());
            for name in tags.filter_map(|t| t.user.as_ref()?.username.as_deref()) {
                if !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                    names.push(name.to_string());
                }
            }
        }
        names
    }

    /// The names the caption mentions with an at sign, each once.
    pub fn mentioned(&self) -> Vec<String> {
        mentions_in(self.caption_text())
    }

    /// [`Self::tagged`], with their pks.
    pub fn tagged_accounts(&self) -> Vec<Named> {
        let mut found: Vec<Named> = Vec::new();
        let all = std::iter::once(self).chain(self.carousel_media.iter());
        for media in all {
            let tags = media.usertags.iter().flat_map(|t| t.tagged.iter());
            for account in tags.filter_map(|t| named(t.user.as_ref()?)) {
                if !found
                    .iter()
                    .any(|n| n.username.eq_ignore_ascii_case(&account.username))
                {
                    found.push(account);
                }
            }
        }
        found
    }

    pub fn with_accounts(&self) -> Vec<Named> {
        self.coauthor_producers.iter().filter_map(named).collect()
    }

    /// The likers the viewer knows, at most [`FACEPILE_SHOWN`]: the faces, or
    /// the one name of [`Self::top_likers`] when there are none.
    pub fn known_likers(&self) -> Vec<Named> {
        let faces: Vec<Named> = self
            .facepile_top_likers
            .iter()
            .filter_map(named)
            .take(FACEPILE_SHOWN)
            .collect();
        if !faces.is_empty() {
            return faces;
        }
        self.top_likers
            .iter()
            .take(FACEPILE_SHOWN)
            .map(|name| Named {
                pk: None,
                username: name.clone(),
            })
            .collect()
    }

    /// A reel's song or its own sound, or a photo's song.
    pub fn music(&self) -> Option<Music> {
        let clips = self.clips_metadata.as_ref();
        let song = clips
            .and_then(|c| c.music_info.as_ref())
            .or_else(|| {
                self.music_metadata
                    .as_ref()
                    .and_then(|m| m.music_info.as_ref())
            })
            .and_then(|m| m.music_asset_info.as_ref())
            .filter(|song| song.title.is_some() || song.display_artist.is_some());
        if let Some(song) = song {
            return Some(Music {
                licensed: true,
                title: song.title.clone(),
                artist: song.display_artist.clone(),
                artist_username: song.ig_username.clone(),
            });
        }
        let sound = clips.and_then(|c| c.original_sound_info.as_ref())?;
        let artist = sound.ig_artist.as_ref().and_then(|a| a.username.clone());
        (sound.original_audio_title.is_some() || artist.is_some()).then(|| Music {
            licensed: false,
            title: sound.original_audio_title.clone(),
            artist: artist.clone(),
            artist_username: artist,
        })
    }

    /// Whether it is pinned to the top of its owner's grid or a co-author's.
    pub fn pinned(&self) -> bool {
        let owner = self.user.as_ref().and_then(|u| u.pk.as_deref());
        let coauthors = self
            .coauthor_producers
            .iter()
            .filter_map(|a| a.pk.as_deref());
        owner
            .into_iter()
            .chain(coauthors)
            .any(|pk| self.timeline_pinned_user_ids.iter().any(|id| id == pk))
    }

    /// The notes written on it where it was reposted, those with any text.
    pub fn notes(&self) -> Vec<&str> {
        self.media_notes
            .iter()
            .flat_map(|notes| notes.items.iter())
            .filter_map(|note| note.text.as_deref())
            .filter(|text| !text.trim().is_empty())
            .collect()
    }

    pub fn with(&self) -> Vec<String> {
        self.coauthor_producers
            .iter()
            .filter_map(|a| a.username.clone())
            .collect()
    }

    pub fn plays(&self) -> Option<u64> {
        self.play_count.or(self.ig_play_count)
    }

    /// "Liked by <name> and N others". `None` when nobody the viewer knows
    /// liked it.
    pub fn liked_by(&self) -> Option<LikedBy> {
        let name = self.top_likers.first()?.clone();
        let others = if self.like_and_view_counts_disabled == Some(true) {
            None
        } else {
            self.like_count.map(|n| n.saturating_sub(1))
        };
        Some(LikedBy { name, others })
    }
}

fn named(account: &Account) -> Option<Named> {
    Some(Named {
        pk: account.pk.clone(),
        username: account.username.clone()?,
    })
}

/// The widest of `versions`, the tallest among equals, the first among those.
fn widest(versions: &[PictureVersion]) -> Option<&str> {
    versions
        .iter()
        .rev()
        .max_by_key(|v| (v.width.unwrap_or(0), v.height.unwrap_or(0)))
        .map(|v| v.url.as_str())
}

/// `others` is `None` when the owner hid the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LikedBy {
    pub name: String,
    pub others: Option<u64>,
}

/// The names `text` mentions: an at sign not preceded by a name character,
/// then a profile name, without the dots a sentence may end it with. Each
/// once, in order.
pub fn mentions_in(text: &str) -> Vec<String> {
    let name_char = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_';
    let mut names: Vec<String> = Vec::new();
    let mut previous: Option<char> = None;
    for (at, c) in text.char_indices() {
        let starts = c == '@' && !previous.is_some_and(name_char);
        previous = Some(c);
        if !starts {
            continue;
        }
        let rest = &text[at + 1..];
        let len = rest.find(|c: char| !name_char(c)).unwrap_or(rest.len());
        let name = rest[..len].trim_end_matches('.');
        if crate::allowlist::profile_name(name) && !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The answer of the grid's two queries (`allowlist::Operation::ProfilePosts`).
#[derive(Debug, Clone, Deserialize)]
pub struct TimelinePage {
    #[serde(default)]
    pub data: Option<TimelineData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimelineData {
    #[serde(
        default,
        rename = "xdt_api__v1__feed__user_timeline_graphql_connection"
    )]
    pub timeline: Option<Connection<Media>>,
}

impl TimelinePage {
    /// The posts, in the grid's order, and where the next page starts.
    /// **A short page is not the last one** (two posts and more to come, in
    /// the capture of 2026-10-01): only the cursor says where the grid ends.
    pub fn posts(self) -> (Vec<Media>, Option<String>) {
        let Some(timeline) = self.data.and_then(|d| d.timeline) else {
            return (Vec::new(), None);
        };
        let next = timeline.next_cursor().map(str::to_string);
        (timeline.nodes(), next)
    }
}

/// The shape of every next cursor of a reels or reposts tab in the captures of
/// 2026-11-13 and 2026-11-14 (13 and 12 of them, each 90 characters of
/// base64url starting `AQHT`): `A-Z a-z 0-9 - _`, and nothing else.
pub fn reels_cursor_has_its_shape(cursor: &str) -> bool {
    (1..=MOST_TAB_CURSOR_BYTES).contains(&cursor.len())
        && cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// More than twice the 90 of every cursor seen.
pub const MOST_TAB_CURSOR_BYTES: usize = 200;

/// The shape of every next cursor of a tagged tab in the capture of
/// 2026-11-14 (9 of 9): a post's pk in digits. **`"None"` is no cursor**: the
/// last page names its end with the word.
pub fn tagged_cursor_has_its_shape(cursor: &str) -> bool {
    (1..=20).contains(&cursor.len()) && cursor.bytes().all(|b| b.is_ascii_digit())
}

/// A tab's next cursor, when it has the shape `shaped` holds to. Another
/// shape ends the tab: it would be text nobody vouched for in a request.
fn tab_cursor(next: Option<&str>, shaped: fn(&str) -> bool) -> Option<String> {
    let next = next.filter(|next| !next.is_empty() && *next != "None")?;
    if !shaped(next) {
        tracing::warn!(
            bytes = next.len(),
            "a tab's page handed out a cursor of a shape never seen; reading no further"
        );
        return None;
    }
    Some(next.to_string())
}

/// The reels tab's answer: `data.fetch__XDTUserDict.clips_connection`, one
/// reel per edge under `media`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReelsTabPage {
    #[serde(default)]
    pub data: Option<ReelsTabData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReelsTabData {
    #[serde(default, rename = "fetch__XDTUserDict")]
    pub user: Option<ReelsTabUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReelsTabUser {
    #[serde(default)]
    pub clips_connection: Option<Connection<ReelsTabNode>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReelsTabNode {
    pub media: Media,
}

impl ReelsTabPage {
    pub fn posts(self) -> (Vec<Media>, Option<String>) {
        let Some(clips) = self
            .data
            .and_then(|d| d.user)
            .and_then(|u| u.clips_connection)
        else {
            return (Vec::new(), None);
        };
        let next = tab_cursor(clips.next_cursor(), reels_cursor_has_its_shape);
        (clips.nodes().into_iter().map(|n| n.media).collect(), next)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaggedTabPage {
    #[serde(default)]
    pub data: Option<TaggedTabData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaggedTabData {
    #[serde(default, rename = "xdt_api__v1__usertags__user_id__feed_connection")]
    pub feed: Option<Connection<Media>>,
}

impl TaggedTabPage {
    pub fn posts(self) -> (Vec<Media>, Option<String>) {
        let Some(feed) = self.data.and_then(|d| d.feed) else {
            return (Vec::new(), None);
        };
        let next = tab_cursor(feed.next_cursor(), tagged_cursor_has_its_shape);
        (feed.nodes(), next)
    }
}

/// The reposts tab's answer: `data.fetch__XDTUserDict.user_reposts_timeline`.
#[derive(Debug, Clone, Deserialize)]
pub struct RepostsTabPage {
    #[serde(default)]
    pub data: Option<RepostsTabData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepostsTabData {
    #[serde(default, rename = "fetch__XDTUserDict")]
    pub user: Option<RepostsTabUser>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepostsTabUser {
    #[serde(default)]
    pub user_reposts_timeline: Option<RepostsTimeline>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepostsTimeline {
    #[serde(default, deserialize_with = "nullable")]
    pub repost_grid_items: Vec<RepostItem>,
    #[serde(default)]
    pub repost_more_available: Option<bool>,
    #[serde(default)]
    pub repost_next_max_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepostItem {
    pub media: Media,
}

impl RepostsTabPage {
    /// The reposts and where the next page starts, only while the answer says
    /// more are available (`false` and `null` ended the tab on 2026-11-13).
    pub fn posts(self) -> (Vec<Media>, Option<String>) {
        let Some(timeline) = self
            .data
            .and_then(|d| d.user)
            .and_then(|u| u.user_reposts_timeline)
        else {
            return (Vec::new(), None);
        };
        let next = timeline
            .repost_more_available
            .filter(|more| *more)
            .and_then(|_| {
                tab_cursor(
                    timeline.repost_next_max_id.as_deref(),
                    reels_cursor_has_its_shape,
                )
            });
        let posts = timeline
            .repost_grid_items
            .into_iter()
            .map(|item| item.media)
            .collect();
        (posts, next)
    }
}

/// `media/<pk>/info/`: the post, as the only item of `items`.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaInfo {
    #[serde(default, deserialize_with = "nullable")]
    pub items: Vec<Media>,
}

/// One page of `media/<pk>/comments/`.
#[derive(Debug, Clone, Deserialize)]
pub struct CommentsPage {
    #[serde(default, deserialize_with = "nullable")]
    pub comments: Vec<Comment>,
    #[serde(default)]
    pub comment_count: Option<u64>,
    #[serde(default)]
    pub has_more_comments: Option<bool>,
    #[serde(default)]
    pub has_more_headload_comments: Option<bool>,
    /// A JSON object as text, sent back as it came, as `min_id`.
    #[serde(default)]
    pub next_min_id: Option<String>,
}

impl CommentsPage {
    /// Where the next page starts, when there is one.
    ///
    /// **`has_more_comments` is no guide**: `false` on all 25 pages of the
    /// capture of 2026-10-01. The app pages by `next_min_id`, beside
    /// `has_more_headload_comments: true`.
    pub fn next(&self) -> Option<&str> {
        let more =
            self.has_more_headload_comments != Some(false) || self.has_more_comments == Some(true);
        let next = self
            .next_min_id
            .as_deref()
            .filter(|next| more && !next.is_empty())?;
        if !comments_cursor_has_its_shape(next) {
            // Ended rather than sent: text nobody vouched for in a request.
            tracing::warn!(
                bytes = next.len(),
                "a page of comments handed out a cursor of a shape never seen; reading no further"
            );
            return None;
        }
        Some(next)
    }

    /// The accounts the page names, authors of comments and of the replies
    /// shown: what it is charged to the day's accounts with.
    pub fn accounts(&self) -> usize {
        self.comments
            .iter()
            .map(|c| 1 + c.preview_child_comments.len())
            .sum()
    }
}

/// The longest cursor of the capture of 2026-11-13 was 346 bytes.
const MOST_CURSOR_BYTES: usize = 1_400;

/// The shape of every `next_min_id` of the capture of 2026-11-13 (9 of 9): a
/// JSON object holding `cached_comments_cursor`, a pk in digits, and maybe
/// `bifilter_token`, base64 of either alphabet, and nothing else.
pub fn comments_cursor_has_its_shape(cursor: &str) -> bool {
    if cursor.len() > MOST_CURSOR_BYTES {
        return false;
    }
    let Ok(serde_json::Value::Object(keys)) = serde_json::from_str(cursor) else {
        return false;
    };
    let digits = |v: &serde_json::Value| {
        v.as_str()
            .is_some_and(|s| (1..=32).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
    };
    let base64 = |v: &serde_json::Value| {
        v.as_str().is_some_and(|s| {
            !s.is_empty()
                && s.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_' | b'=')
                })
        })
    };
    keys.get("cached_comments_cursor").is_some_and(digits)
        && keys.iter().all(|(key, value)| match key.as_str() {
            "cached_comments_cursor" => true,
            "bifilter_token" => base64(value),
            _ => false,
        })
}

/// `media/<pk>/likers/`: as many likers as the app is shown (105 of 603,328
/// on 2026-11-13; 125 of 1,796 on 2026-11-14), and how many there are.
/// **No cursor**: the app asked for no second page.
#[derive(Debug, Clone, Deserialize)]
pub struct LikersPage {
    #[serde(default, deserialize_with = "nullable")]
    pub users: Vec<Liker>,
    #[serde(default)]
    pub user_count: Option<u64>,
}

impl LikersPage {
    /// The likers that name an account, each with how the viewer stands with
    /// it. One with no pk or no name is skipped.
    pub fn accounts(&self) -> Vec<(snob_core::model::User, Option<super::FriendshipStatus>)> {
        self.users
            .iter()
            .filter_map(|liker| {
                let pk = liker.pk.as_deref()?.parse().ok()?;
                let username = liker.username.clone().filter(|name| !name.is_empty())?;
                Some((
                    snob_core::model::User {
                        pk,
                        username,
                        full_name: liker.full_name.clone(),
                        is_private: liker.is_private,
                        is_verified: liker.is_verified,
                        pfp_url: liker.profile_pic_url.clone(),
                    },
                    liker.friendship_status.clone(),
                ))
            })
            .collect()
    }
}

/// One account of [`LikersPage`], with how the viewer stands with it, which
/// is why the app's `show_many` after the list is not needed.
#[derive(Debug, Clone, Deserialize)]
pub struct Liker {
    #[serde(default, deserialize_with = "flexible_id")]
    pub pk: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub is_private: Option<bool>,
    #[serde(default)]
    pub is_verified: Option<bool>,
    #[serde(default)]
    pub profile_pic_url: Option<String>,
    #[serde(default)]
    pub friendship_status: Option<super::FriendshipStatus>,
}

/// A comment, or a reply shown under one.
#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    #[serde(default, deserialize_with = "flexible_id")]
    pub pk: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub created_at: Epoch,
    #[serde(default)]
    pub user: Option<Account>,
    #[serde(default)]
    pub comment_like_count: Option<u64>,
    /// **More than any answer hands out**: threads of 9, 7 and 8 answered
    /// with 7, 5 and 7 (capture of 2026-11-14), so what is left is never
    /// worked out from it.
    #[serde(default)]
    pub child_comment_count: Option<u64>,
    /// The replies shown under it: none on most comments with replies (19 of
    /// 19 in the capture of 2026-11-14).
    #[serde(default, deserialize_with = "nullable")]
    pub preview_child_comments: Vec<Comment>,
}

impl Comment {
    /// Whether its thread holds replies the page did not show
    /// (`allowlist::Rest::Replies`).
    pub fn replies_missing(&self) -> bool {
        let shown = u64::try_from(self.preview_child_comments.len()).unwrap_or(u64::MAX);
        self.child_comment_count.unwrap_or(0) > shown
    }
}

/// The comment a thread's replies were asked under, and the post it is on.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ParentComment {
    #[serde(default, deserialize_with = "flexible_id")]
    pub pk: Option<String>,
    #[serde(default, deserialize_with = "flexible_id")]
    pub media_id: Option<String>,
}

/// `media/<pk>/comments/<comment_pk>/child_comments/`: the replies under one
/// comment, in the order written.
#[derive(Debug, Clone, Deserialize)]
pub struct RepliesPage {
    #[serde(default, deserialize_with = "nullable")]
    pub child_comments: Vec<Comment>,
    #[serde(default)]
    pub child_comment_count: Option<u64>,
    #[serde(default)]
    pub has_more_head_child_comments: Option<bool>,
    #[serde(default)]
    pub has_more_tail_child_comments: Option<bool>,
    #[serde(default)]
    pub parent_comment: Option<ParentComment>,
    /// The names of the answer's other keys, logged when a thread says it has
    /// more ([`Self::more`]): how a second page's cursor will be learned.
    #[serde(flatten)]
    pub other: std::collections::BTreeMap<String, serde::de::IgnoredAny>,
}

impl RepliesPage {
    /// Whether Instagram said the thread holds more. **Never sent for**: no
    /// capture shows how a second page is asked (AGENTS.md, "Open questions").
    pub fn more(&self) -> bool {
        let more = self.has_more_head_child_comments == Some(true)
            || self.has_more_tail_child_comments == Some(true);
        if more {
            let keys: Vec<&str> = self.other.keys().map(String::as_str).collect();
            tracing::debug!(
                ?keys,
                "a thread of replies said it has more than it handed out; its other keys"
            );
        }
        more
    }

    /// Whether the answer is the thread of a comment on post `pk`.
    pub fn is_on(&self, pk: MediaPk) -> bool {
        let pk = pk.to_string();
        self.parent_comment
            .as_ref()
            .and_then(|parent| parent.media_id.as_deref())
            .is_some_and(|media| media == pk)
    }

    /// Each reply's author: what the page is charged to the day's accounts with.
    pub fn accounts(&self) -> usize {
        self.child_comments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comments_cursor_is_taken_only_in_the_shape_instagram_hands_out() {
        let token = "QVFBd0RfZm9v-_=";
        for good in [
            format!(
                r#"{{"cached_comments_cursor":"18012345678901234","bifilter_token":"{token}"}}"#
            ),
            r#"{"cached_comments_cursor":"18012345678901234"}"#.to_string(),
        ] {
            assert!(comments_cursor_has_its_shape(&good), "{good}");
        }
        for bad in [
            String::new(),
            "18012345678901234".to_string(),
            r#"{"c":"1"}"#.to_string(),
            r#"{"bifilter_token":"QVFB"}"#.to_string(),
            r#"{"cached_comments_cursor":18012345678901234}"#.to_string(),
            r#"{"cached_comments_cursor":"12a"}"#.to_string(),
            r#"{"cached_comments_cursor":"1","bifilter_token":"a&b=c"}"#.to_string(),
            r#"{"cached_comments_cursor":"1","bifilter_token":""}"#.to_string(),
            r#"{"cached_comments_cursor":"1","sort_order":"newest"}"#.to_string(),
            format!(
                r#"{{"cached_comments_cursor":"1","bifilter_token":"{}"}}"#,
                "A".repeat(2_000)
            ),
        ] {
            assert!(!comments_cursor_has_its_shape(&bad), "{bad}");
        }
    }

    #[test]
    fn a_page_whose_cursor_has_another_shape_is_the_last() {
        let page = |next: &str| -> CommentsPage {
            serde_json::from_value(serde_json::json!({
                "comments": [], "has_more_headload_comments": true, "next_min_id": next,
            }))
            .unwrap()
        };
        let good = r#"{"cached_comments_cursor":"1","bifilter_token":"QVFB"}"#;
        assert_eq!(page(good).next(), Some(good));
        assert_eq!(page(r#"{"c":"1"}"#).next(), None);
    }

    /// The likers are read with their standing, a pk as text or as a
    /// number; one with no name or no pk is skipped, not the whole answer.
    #[test]
    fn the_likers_are_read_with_how_the_viewer_stands_with_each() {
        let page: LikersPage = serde_json::from_str(
            r#"{"users":[
              {"pk":"11","username":"first","is_private":true,
               "friendship_status":{"following":true,"followed_by":false}},
              {"pk":12,"username":"second"},
              {"pk":"13"},
              {"username":"nopk"}
            ],"user_count":603328,"status":"ok"}"#,
        )
        .unwrap();
        assert_eq!(page.user_count, Some(603_328));
        let accounts = page.accounts();
        let names: Vec<&str> = accounts.iter().map(|(u, _)| u.username.as_str()).collect();
        assert_eq!(names, ["first", "second"]);
        assert_eq!(accounts[0].0.pk, snob_core::Pk::new(11));
        assert!(
            accounts[0]
                .1
                .as_ref()
                .is_some_and(|f| f.following && !f.followed_by)
        );
        assert!(accounts[1].1.is_none());
        let empty: LikersPage = serde_json::from_str(r#"{"users":null}"#).unwrap();
        assert!(empty.accounts().is_empty());
    }

    fn media(json: &str) -> Media {
        serde_json::from_str(json).unwrap()
    }

    /// The faces the app draws are the known likers, at most three, by pk
    /// and name; one with no name is skipped, a `null` list is an empty one,
    /// and with no faces the one name of `top_likers` stands in.
    #[test]
    fn a_facepile_entry_with_no_name_is_skipped() {
        let faces = media(
            r#"{"pk":"1","top_likers":["first"],"facepile_top_likers":[
              {"pk":"11","username":"first"},
              {"id":"12","profile_pic_url":"https://x.cdninstagram.com/p.jpg"},
              {"pk":13,"username":"second"},
              {"pk":"14","username":"third"},
              {"pk":"15","username":"fourth"}]}"#,
        );
        let names: Vec<_> = faces
            .known_likers()
            .into_iter()
            .map(|n| (n.pk, n.username))
            .collect();
        assert_eq!(
            names,
            [
                (Some("11".into()), "first".into()),
                (Some("13".into()), "second".into()),
                (Some("14".into()), "third".into()),
            ]
        );
        let none = media(r#"{"pk":"1","top_likers":["first"],"facepile_top_likers":null}"#);
        assert!(none.facepile_top_likers.is_empty());
        assert_eq!(
            none.known_likers(),
            [Named {
                pk: None,
                username: "first".into()
            }]
        );
        assert!(media(r#"{"pk":"1"}"#).known_likers().is_empty());
    }

    /// An account's pk reads from text and from a number alike, as both
    /// occur; so do the ids a post is pinned for.
    #[test]
    fn an_account_pk_is_read_as_text_or_as_a_number() {
        let text: Account = serde_json::from_str(r#"{"pk":"42","username":"a"}"#).unwrap();
        let number: Account = serde_json::from_str(r#"{"pk":42,"username":"a"}"#).unwrap();
        assert_eq!(text.pk.as_deref(), Some("42"));
        assert_eq!(number.pk.as_deref(), Some("42"));
        let pinned = media(r#"{"pk":"1","timeline_pinned_user_ids":[42,"43"]}"#);
        assert_eq!(pinned.timeline_pinned_user_ids, ["42", "43"]);
        let none = media(r#"{"pk":"1","timeline_pinned_user_ids":null}"#);
        assert!(none.timeline_pinned_user_ids.is_empty());
    }

    /// Pinned is the owner's or a co-author's pk among the accounts it is
    /// pinned for, never another's.
    #[test]
    fn a_post_is_pinned_for_its_owner_or_a_coauthor() {
        let owners = media(r#"{"pk":"1","user":{"pk":"42"},"timeline_pinned_user_ids":[42]}"#);
        assert!(owners.pinned());
        let coauthors = media(
            r#"{"pk":"1","user":{"pk":"42"},"coauthor_producers":[{"pk":"7","username":"b"}],
            "timeline_pinned_user_ids":["7"]}"#,
        );
        assert!(coauthors.pinned());
        let anothers = media(r#"{"pk":"1","user":{"pk":"42"},"timeline_pinned_user_ids":["9"]}"#);
        assert!(!anothers.pinned());
        assert!(!media(r#"{"pk":"1"}"#).pinned());
    }

    /// A reel's song, a reel's own sound, and a photo's song, each read from
    /// where its kind of post says it; a sound that names nothing is none.
    #[test]
    fn music_is_read_from_a_reel_or_a_photo() {
        let song = media(
            r#"{"pk":"1","clips_metadata":{"audio_type":"licensed_music","music_info":
              {"music_asset_info":{"title":"A Song","display_artist":"Singer","ig_username":"singer"}}}}"#,
        );
        assert_eq!(
            song.music(),
            Some(Music {
                licensed: true,
                title: Some("A Song".into()),
                artist: Some("Singer".into()),
                artist_username: Some("singer".into()),
            })
        );
        let sound = media(
            r#"{"pk":"1","clips_metadata":{"audio_type":"original_sounds","music_info":null,
              "original_sound_info":{"original_audio_title":"Original audio","ig_artist":{"pk":"2","username":"maker"}}}}"#,
        );
        assert_eq!(
            sound.music(),
            Some(Music {
                licensed: false,
                title: Some("Original audio".into()),
                artist: Some("maker".into()),
                artist_username: Some("maker".into()),
            })
        );
        let photo = media(
            r#"{"pk":"1","clips_metadata":null,"music_metadata":{"audio_type":"licensed_music",
              "music_info":{"music_asset_info":{"title":"Tune","display_artist":"Band","ig_username":null}}}}"#,
        );
        assert_eq!(photo.music().and_then(|m| m.title), Some("Tune".into()));
        let silent = media(r#"{"pk":"1","clips_metadata":{"original_sound_info":{}}}"#);
        assert_eq!(silent.music(), None);
    }

    /// A grid page as the capture shapes it: a carousel of two photos, a
    /// reel with its three progressive files, and the cursor.
    fn grid() -> &'static str {
        r#"{"data":{"xdt_api__v1__feed__user_timeline_graphql_connection":{
            "edges":[
              {"node":{"pk":"3995556159532654737","code":"DdzEShhEWCR","media_type":8,
                "product_type":"carousel_container","taken_at":1790527432,
                "caption":{"text":"sun with @friend.one and @friend.one. hi@nope"},
                "like_count":80,"comment_count":8,"top_likers":["close"],
                "like_and_view_counts_disabled":false,
                "usertags":null,"coauthor_producers":[{"username":"partner"}],
                "user":{"username":"someone"},"location":{"name":"A beach"},
                "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/big.jpg","width":1440,"height":1920},{"url":"https://x.cdninstagram.com/small.jpg","width":150,"height":150}]},
                "video_versions":null,
                "carousel_media":[
                  {"pk":"3995554512760065776","media_type":1,"usertags":{"in":[{"user":{"username":"tagged"}}]},
                   "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/a.jpg","width":1080,"height":1440}]},"video_versions":null},
                  {"pk":"3995554515981225378","media_type":2,
                   "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/b.jpg","width":720,"height":1280}]},
                   "video_versions":[{"type":101,"width":720,"height":1280,"url":"https://x.cdninstagram.com/b.mp4"},{"type":102,"width":480,"height":854,"url":"https://x.cdninstagram.com/b-small.mp4"}]}
                ],"carousel_media_count":2}},
              {"node":{"pk":"3947056156557494178","code":"DbGwql4IMei","media_type":2,"product_type":"clips",
                "taken_at":1790000000,"caption":null,"like_count":206082,"comment_count":552,
                "like_and_view_counts_disabled":true,"top_likers":["close"],
                "video_versions":[{"type":101,"url":"https://x.cdninstagram.com/r.mp4"}],
                "carousel_media":null}}
            ],
            "page_info":{"end_cursor":"AQH-next","has_next_page":true}},
          "xdt_viewer":{"user":{"id":"42"}}},"extensions":{"is_final":true}}"#
    }

    #[test]
    fn a_grid_page_reads_its_posts_and_its_cursor() {
        let page: TimelinePage = serde_json::from_str(grid()).unwrap();
        let (posts, next) = page.posts();
        assert_eq!(next.as_deref(), Some("AQH-next"));
        assert_eq!(posts.len(), 2);

        let carousel = &posts[0];
        assert_eq!(carousel.pk, MediaPk::new(3_995_556_159_532_654_737));
        assert_eq!(carousel.items().len(), 2);
        assert_eq!(
            carousel.items()[0].best_url(),
            Some("https://x.cdninstagram.com/a.jpg")
        );
        assert!(carousel.items()[1].is_video());
        assert_eq!(
            carousel.items()[1].best_url(),
            Some("https://x.cdninstagram.com/b.mp4"),
            "the widest progressive file, not its cover"
        );
        assert_eq!(carousel.tagged(), ["tagged"]);
        assert_eq!(carousel.mentioned(), ["friend.one"]);
        assert_eq!(carousel.with(), ["partner"]);
        assert_eq!(carousel.owner(), Some("someone"));
        assert_eq!(
            carousel.liked_by(),
            Some(LikedBy {
                name: "close".into(),
                others: Some(79)
            })
        );

        let reel = &posts[1];
        assert_eq!(reel.items().len(), 1);
        assert_eq!(reel.caption_text(), "");
        assert_eq!(
            reel.liked_by(),
            Some(LikedBy {
                name: "close".into(),
                others: None
            }),
            "a hidden count is not said"
        );
    }

    #[test]
    fn the_last_grid_page_has_no_cursor() {
        let page: TimelinePage = serde_json::from_str(
            r#"{"data":{"xdt_api__v1__feed__user_timeline_graphql_connection":{"edges":[],"page_info":{"end_cursor":null,"has_next_page":false}}}}"#,
        )
        .unwrap();
        let (posts, next) = page.posts();
        assert!(posts.is_empty());
        assert_eq!(next, None);
    }

    /// The comments' next page is `next_min_id`, whatever
    /// `has_more_comments` says, as long as the page says more are loaded
    /// at its head.
    #[test]
    fn comments_page_by_their_next_min_id() {
        let page: CommentsPage = serde_json::from_str(
            r#"{"comments":[{"pk":"18177246133428661","text":"nice","created_at":1790527508,
                "user":{"username":"friend"},"comment_like_count":3,"child_comment_count":2,
                "preview_child_comments":[{"pk":1,"text":"yes","user":{"username":"owner"}}]}],
               "comment_count":836,"has_more_comments":false,"has_more_headload_comments":true,
               "next_min_id":"{\"cached_comments_cursor\":\"1\",\"bifilter_token\":\"x\"}"}"#,
        )
        .unwrap();
        assert_eq!(
            page.next(),
            Some(r#"{"cached_comments_cursor":"1","bifilter_token":"x"}"#)
        );
        assert_eq!(page.accounts(), 2);
        assert_eq!(page.comments[0].preview_child_comments.len(), 1);

        let last: CommentsPage = serde_json::from_str(
            r#"{"comments":[],"has_more_comments":false,"has_more_headload_comments":false,"next_min_id":"{}"}"#,
        )
        .unwrap();
        assert_eq!(last.next(), None);
    }

    /// A thread's replies as the capture of 2026-11-14 shapes them: fewer
    /// than its count, a reply's author pk as text and as a number in the
    /// same answer, and nothing more to read.
    #[test]
    fn fewer_replies_than_the_count_are_not_called_more_to_read() {
        let page: RepliesPage = serde_json::from_str(
            r#"{"child_comment_count":9,"child_comments":[
                {"pk":"18000000000000001","text":"a","user":{"pk":42,"username":"one"},"created_at":1790000000},
                {"pk":"18000000000000002","text":"b","user":{"pk":"43","username":"two"}}],
              "has_more_head_child_comments":false,"has_more_tail_child_comments":false,
              "is_ranked_replies":false,
              "parent_comment":{"pk":"18000000000000000","media_id":"3807824420233075826"},
              "status":"ok"}"#,
        )
        .unwrap();
        assert_eq!(page.child_comments.len(), 2);
        assert_eq!(page.child_comment_count, Some(9));
        assert!(
            !page.more(),
            "seven short of the count is still the whole thread"
        );
        assert_eq!(page.accounts(), 2);
        let authors: Vec<_> = page
            .child_comments
            .iter()
            .map(|c| c.user.as_ref().and_then(|u| u.pk.clone()))
            .collect();
        assert_eq!(authors, [Some("42".into()), Some("43".into())]);
        assert!(page.is_on(MediaPk::new(3_807_824_420_233_075_826)));
        assert!(!page.is_on(MediaPk::new(3_807_824_420_233_075_827)));
        assert!(page.other.contains_key("is_ranked_replies"));
    }

    /// A thread that says it has more is said to, and an answer that names
    /// no parent comment is no thread of any post.
    #[test]
    fn a_thread_that_says_it_has_more_is_believed() {
        let page: RepliesPage = serde_json::from_str(
            r#"{"child_comments":[],"has_more_tail_child_comments":true,"status":"ok"}"#,
        )
        .unwrap();
        assert!(page.more());
        assert!(!page.is_on(MediaPk::new(1)));
        let empty: RepliesPage = serde_json::from_str(r#"{"child_comments":null}"#).unwrap();
        assert!(empty.child_comments.is_empty() && !empty.more());
    }

    /// A comment whose previews hold its whole thread is not asked for; one
    /// with replies the page did not show is.
    #[test]
    fn a_thread_whose_previews_hold_it_whole_is_not_asked_for() {
        let comment = |json: &str| serde_json::from_str::<Comment>(json).unwrap();
        assert!(
            !comment(r#"{"pk":"1","child_comment_count":1,"preview_child_comments":[{"pk":"2"}]}"#)
                .replies_missing()
        );
        assert!(
            comment(r#"{"pk":"1","child_comment_count":9,"preview_child_comments":[]}"#)
                .replies_missing()
        );
        assert!(!comment(r#"{"pk":"1"}"#).replies_missing());
        assert!(!comment(r#"{"pk":"1","child_comment_count":0}"#).replies_missing());
    }

    /// A reels tab page as the capture of 2026-11-14 shapes it: each reel
    /// under `media`, with no date and no video address, and the cursor; the
    /// last page has none.
    #[test]
    fn a_reels_tab_reads_its_reels_and_its_cursor() {
        let page: ReelsTabPage = serde_json::from_str(
            r#"{"data":{"fetch__XDTUserDict":{"id":"42","clips_connection":{
                "edges":[{"__typename":"XDTClipsEdge","node":{"__typename":"XDTClipsNode","media":
                  {"pk":"3807824420233075826","code":"DTYHCKvDNxy","media_type":2,
                   "product_type":"clips","play_count":12,"like_count":3,
                   "like_and_view_counts_disabled":true,"user":{"pk":"42","username":"someone"},
                   "video_versions":null}}}],
                "page_info":{"end_cursor":"AQHT0vU8-x_y","has_next_page":true}}},
              "xdt_viewer":{"user":{"id":"1"}}}}"#,
        )
        .unwrap();
        let (reels, next) = page.posts();
        assert_eq!(reels.len(), 1);
        assert_eq!(reels[0].taken_at, Epoch::default());
        assert_eq!(reels[0].owner(), Some("someone"));
        assert_eq!(next.as_deref(), Some("AQHT0vU8-x_y"));
        let last: ReelsTabPage = serde_json::from_str(
            r#"{"data":{"fetch__XDTUserDict":{"clips_connection":{"edges":[],
                "page_info":{"end_cursor":null,"has_next_page":false}}}}}"#,
        )
        .unwrap();
        assert_eq!(last.posts().1, None);
    }

    /// The tagged tab's cursor is a post's pk; the word `None` its last
    /// page names its end with is no cursor, whatever `has_next_page` says.
    #[test]
    fn the_word_none_is_no_cursor() {
        let page = |cursor: &str, more: bool| -> TaggedTabPage {
            serde_json::from_value(serde_json::json!({"data": {
                "xdt_api__v1__usertags__user_id__feed_connection": {
                    "edges": [], "page_info": {"end_cursor": cursor, "has_next_page": more,
                                               "has_previous_page": false, "start_cursor": null}}}}))
            .unwrap()
        };
        assert_eq!(
            page("3807824420233075826", true).posts().1.as_deref(),
            Some("3807824420233075826")
        );
        assert_eq!(page("None", false).posts().1, None);
        assert_eq!(page("None", true).posts().1, None);
        assert_eq!(page("AQHT", true).posts().1, None, "not a pk");
    }

    /// The reposts tab goes on while it says more are available, by the
    /// `max_id` it hands out, and ends when it says no more; a post's owner
    /// is its own, and its notes are read.
    #[test]
    fn a_reposts_tab_ends_when_it_says_no_more() {
        let page = |more: serde_json::Value, next: serde_json::Value| -> RepostsTabPage {
            serde_json::from_value(
                serde_json::json!({"data": {"fetch__XDTUserDict": {"id": "42",
                "user_reposts_timeline": {
                    "repost_grid_items": [{"media": {"pk": "1", "user": {"username": "author"},
                        "media_notes": {"items": [{"id": "n", "text": "so good", "user_id": "7"},
                                                  {"id": "m", "text": " "}]}}}],
                    "repost_more_available": more, "repost_next_max_id": next}}}}),
            )
            .unwrap()
        };
        let (posts, next) = page(serde_json::json!(true), serde_json::json!("AQHT_x-y")).posts();
        assert_eq!(next.as_deref(), Some("AQHT_x-y"));
        assert_eq!(posts[0].owner(), Some("author"));
        assert_eq!(posts[0].notes(), ["so good"]);
        assert_eq!(
            page(serde_json::json!(false), serde_json::json!(null))
                .posts()
                .1,
            None
        );
        assert_eq!(
            page(serde_json::json!(false), serde_json::json!("AQHT"))
                .posts()
                .1,
            None
        );
        assert_eq!(
            page(serde_json::json!(true), serde_json::json!("a&b"))
                .posts()
                .1,
            None
        );
    }

    /// A tab's cursor goes back only in the shape Instagram hands it out.
    #[test]
    fn a_tab_cursor_of_another_shape_is_refused() {
        for good in ["AQHT0vU8QRVn5wKUsKTgqbp1zP3QVQhsRHZDYtRIy8j-_", "a"] {
            assert!(reels_cursor_has_its_shape(good), "{good}");
        }
        for bad in ["", "a b", "a&b=c", "a/b", "a+b", "a=", &"A".repeat(201)] {
            assert!(!reels_cursor_has_its_shape(bad), "{bad}");
        }
        for good in ["3807824420233075826", "1"] {
            assert!(tagged_cursor_has_its_shape(good), "{good}");
        }
        for bad in ["", "None", "-1", "12a", &"1".repeat(21)] {
            assert!(!tagged_cursor_has_its_shape(bad), "{bad}");
        }
    }

    #[test]
    fn a_mention_is_an_at_sign_and_a_name() {
        assert_eq!(
            mentions_in("@a.b, @c_d! mail@example.com (@e.) @@f @"),
            ["a.b", "c_d", "e", "f"]
        );
        assert!(mentions_in("no names here").is_empty());
    }
}
