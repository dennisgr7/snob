//! Posts and reels once they are read: what a listing and a view show of
//! them, how they are fetched, and how their files are kept. `posts`, `post`
//! and their views share it, as `stories` and `highlights` share
//! [`crate::media`].
//!
//! A post and the read of one are here; its parts each have a file: a
//! profile's grid and tabs (`grid`), a post's comments and their threads
//! (`comments`), the accounts that liked it (`likers`), how its files
//! are kept (`save`) and what the JSON contract has of it (`document`).
//! Every front names them as `posts::<name>`.
//!
//! What is read: a grid is its profile and one read per page of twelve, a
//! step apart; opening a post is its info and its first page of comments; a
//! thread of replies is one read ([`read_threads`], at most
//! [`MOST_THREADS`]); a tab is as the grid, with the app's navigation before
//! its second page ([`read_tab`]). The files come from the CDN, which is not
//! paced.
//!
//! **Nothing here tells anybody what was looked at**: the allowlist and
//! `crates/snob-core/tests/no_seen.rs` keep it that way.

use anyhow::Result;
use snob_core::Epoch;
use snob_core::model::post::{AccountRef, Music, Place};
use snob_core::model::printable;
use snob_ig::client::IgClient;
use snob_ig::model::dash;
use snob_ig::model::post::{LikedBy, Media, Named as NamedAccount};
use snob_ig::pace::{PAGES_PER_SITTING, Pace};
use snob_ig::shortcode::{self, MediaPk, Opens, Shortcode};

use crate::exit::{ExitCode, ExitError};
use crate::media::{Best, Kind, Named};

mod comments;
mod document;
mod grid;
mod likers;
mod save;

pub use comments::{
    CommentLine, Comments, MOST_COMMENT_PAGES, MOST_THREADS, Thread, comment_pages, first_comments,
    more_comments, read_thread, read_threads, replies_under, threads_to_read,
};
pub use document::{comment_document, comment_json, post_document, post_json, thread_document};
pub use grid::{
    Fetched, Grid, GridRead, UNTIL_PAGES, fetch_grid, first_page, more, reached, read_grid,
    read_pages, read_tab,
};
pub use likers::{LIKERS_AT_MOST, likers, refuse_short_day};
pub use save::{Kept, Opened, Videos, fetched_item, files_of, keep_item, keep_post, save_telling};

/// Ceiling on one downloaded file of a post: 128 MiB.
///
/// Far above [`crate::media::MAX_STORY_BYTES`], and it has to be: a reel runs for minutes where a
/// story runs for seconds, and a video post for longer still, though a 720-pixel reel rarely passes
/// 100 MiB.
pub const MAX_POST_BYTES: usize = 128 * 1024 * 1024;

/// The most pages of a grid one run reads: one sitting ([`PAGES_PER_SITTING`]),
/// about 480 posts.
pub const MOST_PAGES: u32 = PAGES_PER_SITTING;

/// The site, for the address a listing prints for each post.
const SITE: &str = "https://www.instagram.com";

/// What a post is, as a listing says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostKind {
    Photo,
    Video,
    /// A video posted as a reel.
    Reel,
    /// A carousel, and how many items it holds.
    Carousel(usize),
}

impl PostKind {
    /// "photo", "video", "reel", "3 items".
    pub fn label(self) -> String {
        match self {
            Self::Photo => "photo".into(),
            Self::Video => "video".into(),
            Self::Reel => "reel".into(),
            Self::Carousel(n) => format!("{n} items"),
        }
    }

    /// The word a JSON document uses: a carousel's count is beside it.
    fn word(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Reel => "reel",
            Self::Carousel(_) => "carousel",
        }
    }
}

/// One file of a post: a photo, or a video's progressive file.
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: Kind,
    /// Where the best copy is.
    pub url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The item's own media id.
    pub pk: Option<String>,
    /// The picture's description.
    pub alt: Option<String>,
    /// A video's length in seconds.
    pub duration: Option<f64>,
    /// A video's widest track and its sound, when they are wider than
    /// [`Self::url`]'s file.
    pub best: Option<Best>,
    /// A small picture of it for a view to draw: a photo's own, a video's
    /// cover ([`thumb_of`]). Not in the JSON contract.
    pub thumb: Option<String>,
}

/// One post or reel, reduced to what a listing, a view and a download need.
#[derive(Debug, Clone)]
pub struct Post {
    pub pk: MediaPk,
    /// As its address carries it.
    pub code: String,
    /// As Instagram spells it.
    pub owner: String,
    /// What it is, for its address: a reel's is `/reel/<code>/`.
    pub opens: Opens,
    /// The page it is asked about from, which is not always its address: the
    /// app opens a post or a reel from a grid over `/p/<code>/` alike (4 reels
    /// of 4 in the capture of 2026-11-13, their `Referer` `/p/<code>/`), and a
    /// link from the page the link names.
    pub asked_from: Opens,
    pub kind: PostKind,
    pub taken_at: Epoch,
    /// One entry per line of the caption.
    pub caption: Vec<String>,
    pub location: Option<String>,
    pub likes: Option<u64>,
    /// The owner hid the counts.
    pub likes_hidden: bool,
    pub comments: Option<u64>,
    pub plays: Option<u64>,
    pub tagged: Vec<String>,
    pub mentions: Vec<String>,
    pub with: Vec<String>,
    pub liked_by: Option<LikedBy>,
    pub items: Vec<Item>,
    pub owner_pk: Option<String>,
    pub tagged_accounts: Vec<AccountRef>,
    pub with_accounts: Vec<AccountRef>,
    /// The likers the viewer knows that the app names under it, at most
    /// three ([`snob_ig::model::post::FACEPILE_SHOWN`]).
    pub likers_known: Vec<AccountRef>,
    pub reposts: Option<u64>,
    /// `None` for a photo, and when the owner turned the count off.
    pub shares: Option<u64>,
    pub plays_instagram: Option<u64>,
    pub plays_facebook: Option<u64>,
    /// A video's length in seconds.
    pub duration: Option<f64>,
    pub music: Option<Music>,
    pub place: Option<Place>,
    pub pinned: bool,
    pub paid_partnership: bool,
    pub caption_edited: bool,
    /// The notes other accounts wrote on it where it was reposted, as a
    /// reposts tab shows them.
    pub notes: Vec<String>,
}

impl Post {
    /// The post `media` describes, owned by `owner` when the answer does not
    /// say whose it is.
    pub fn from_media(media: &Media, owner: &str) -> Self {
        let code = media
            .code
            .clone()
            .filter(|code| shortcode::pk_of(code).is_some())
            .unwrap_or_else(|| shortcode::code_of(media.pk));
        let reel = media.product_type.as_deref() == Some("clips");
        let items: Vec<Item> = media
            .items()
            .into_iter()
            .map(|item| {
                let video = item.is_video();
                let (width, height) = if video {
                    let best = item
                        .video_versions
                        .iter()
                        .max_by_key(|v| (v.width.unwrap_or(0), v.height.unwrap_or(0)));
                    (best.and_then(|v| v.width), best.and_then(|v| v.height))
                } else {
                    let largest = item.image_versions2.as_ref().and_then(|c| {
                        c.candidates
                            .iter()
                            .max_by_key(|v| (v.width.unwrap_or(0), v.height.unwrap_or(0)))
                    });
                    (
                        item.original_width.or(largest.and_then(|v| v.width)),
                        item.original_height.or(largest.and_then(|v| v.height)),
                    )
                };
                Item {
                    kind: if video { Kind::Video } else { Kind::Photo },
                    // **A video offered no video's file has no copy**, not
                    // its cover's: a tab's reels come that way (captures of
                    // 2026-11-13 and 2026-11-14), and a cover saved under a
                    // video's name would be a wrong file, where none is a
                    // post to read whole first (`filled`).
                    url: (!video || !item.video_versions.is_empty())
                        .then(|| item.best_url().map(str::to_string))
                        .flatten(),
                    width,
                    height,
                    pk: digits(&item.pk.to_string()),
                    alt: item
                        .accessibility_caption
                        .as_deref()
                        .map(printable)
                        .filter(|alt| !alt.trim().is_empty()),
                    duration: seconds(item.video_duration),
                    best: best_of(item, width),
                    // A carousel the grid handed over without its items is
                    // one item, the post itself, and so its own pictures.
                    thumb: thumb_of(item),
                }
            })
            .collect();
        let kind = if !media.carousel_media.is_empty() {
            PostKind::Carousel(items.len())
        } else if reel {
            PostKind::Reel
        } else if media.is_video() {
            PostKind::Video
        } else {
            PostKind::Photo
        };
        Self {
            pk: media.pk,
            opens: if reel { Opens::Reel } else { Opens::Post },
            asked_from: Opens::Post,
            code,
            owner: printable(media.owner().unwrap_or(owner)),
            kind,
            taken_at: media.taken_at,
            caption: media
                .caption_text()
                .lines()
                .map(printable)
                .filter(|line| !line.trim().is_empty())
                .collect(),
            location: media
                .location
                .as_ref()
                .and_then(|l| l.name.as_deref())
                .map(printable)
                .filter(|name| !name.is_empty()),
            likes: media.like_count,
            likes_hidden: media.like_and_view_counts_disabled == Some(true),
            comments: media.comment_count,
            plays: media.plays(),
            tagged: media.tagged().iter().map(|n| printable(n)).collect(),
            mentions: media.mentioned(),
            with: media.with().iter().map(|n| printable(n)).collect(),
            liked_by: media.liked_by().map(|l| LikedBy {
                name: printable(&l.name),
                others: l.others,
            }),
            items,
            owner_pk: media
                .user
                .as_ref()
                .and_then(|u| u.pk.as_deref())
                .and_then(digits),
            tagged_accounts: media.tagged_accounts().iter().map(account).collect(),
            with_accounts: media.with_accounts().iter().map(account).collect(),
            likers_known: media.known_likers().iter().map(account).collect(),
            reposts: media.media_repost_count,
            shares: media
                .reshare_count
                .filter(|_| media.share_count_disabled != Some(true)),
            plays_instagram: media.ig_play_count,
            plays_facebook: media.fb_play_count,
            duration: seconds(media.video_duration),
            music: media.music().map(|music| Music {
                kind: if music.licensed { "song" } else { "original" }.to_string(),
                title: music.title.as_deref().map(printable),
                artist: music.artist.as_deref().map(printable),
                artist_username: music.artist_username.as_deref().map(printable),
            }),
            place: media.location.as_ref().map(|l| Place {
                pk: l.pk.as_deref().and_then(digits),
                name: l.name.as_deref().map(printable),
                lat: l.lat.filter(|lat| lat.is_finite()),
                lng: l.lng.filter(|lng| lng.is_finite()),
            }),
            pinned: media.pinned(),
            paid_partnership: media.is_paid_partnership == Some(true),
            caption_edited: media.caption_is_edited == Some(true),
            notes: media.notes().into_iter().map(printable).collect(),
        }
    }

    /// When it was posted, as a person reads it, or a dash where the answer
    /// did not say: a tab's posts carry no date (captures of 2026-11-13 and
    /// 2026-11-14), and the JSON's `taken_at` is then 0, as it always was
    /// for a post with none.
    pub fn posted(&self) -> String {
        if self.taken_at == Epoch::default() {
            "-".to_string()
        } else {
            crate::sentences::dated(self.taken_at)
        }
    }

    /// The page the app opens it on: what it is asked about from.
    pub fn page(&self) -> String {
        shortcode::page_of(&self.code, self.asked_from)
    }

    /// Its address, for a listing to print.
    pub fn link(&self) -> String {
        format!("{SITE}{}", shortcode::page_of(&self.code, self.opens))
    }

    /// The first line of its caption, for a row.
    pub fn headline(&self) -> &str {
        self.caption.first().map_or("", String::as_str)
    }

    /// "Liked by <name> and N others", as the app writes it under a post, or "N
    /// likes" when nobody the viewer knows liked it.
    pub fn liked_line(&self) -> Option<String> {
        let names: Vec<&str> = self
            .likers_known
            .iter()
            .map(|a| a.username.as_str())
            .collect();
        let Some((last, first)) = names.split_last() else {
            if self.likes_hidden {
                return None;
            }
            return self.likes.map(|n| match n {
                1 => "1 like".to_string(),
                n => format!("{} likes", grouped(n)),
            });
        };
        let listed = names.join(", ");
        // Never read under hidden counts: the number is then a placeholder.
        let others = if self.likes_hidden {
            None
        } else {
            self.likes.map(|n| n.saturating_sub(names.len() as u64))
        };
        Some(match others {
            Some(0) if first.is_empty() => format!("Liked by {last}"),
            Some(0) => format!("Liked by {} and {last}", first.join(", ")),
            Some(1) => format!("Liked by {listed} and 1 other"),
            Some(n) => format!("Liked by {listed} and {} others", grouped(n)),
            None => format!("Liked by {listed} and others"),
        })
    }

    /// The name every file of item `index` (zero-based) is saved under:
    /// `<owner>-<code>`, and `-<n>` after it when the post holds more than
    /// one, so that `-d` and the views write the very same names.
    ///
    /// **Eleven characters of the code**, which are the post's pk
    /// ([`shortcode`]): a private account's post has a code of 39, and with a
    /// thirty-character name and the item's number the name would pass what
    /// `files::default_path` accepts.
    pub fn stem(&self, index: usize) -> String {
        let code = self.code.get(..11).unwrap_or(&self.code);
        let base = format!("{}-{code}", printable(&self.owner));
        if self.items.len() > 1 {
            format!("{base}-{}", index + 1)
        } else {
            base
        }
    }

    /// Each item by its number and the address of its small picture, for
    /// `media::small_pictures`.
    pub fn thumbs(&self) -> Vec<(usize, Option<String>)> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (index + 1, item.thumb.clone()))
            .collect()
    }

    /// Item `index` (zero-based) as a file to download; an index past the
    /// post's items is a file with nothing to fetch, which a save refuses.
    pub fn file(&self, index: usize) -> Named {
        let item = self.items.get(index);
        Named {
            number: index + 1,
            stem: self.stem(index),
            url: item.and_then(|item| item.url.clone()),
            what: format!("item {}", index + 1),
            cap: MAX_POST_BYTES,
            best: item.and_then(|item| item.best.clone()),
            ffmpeg: None,
        }
    }
}

/// The picture of `item` a view draws small: of its candidates (a video's
/// are its cover), the one [`crate::media::thumb_among`] picks.
fn thumb_of(item: &Media) -> Option<String> {
    crate::media::thumb_among(&item.image_versions2.as_ref()?.candidates)
}

/// `item`'s best tracks, read from its DASH manifest, when its widest video
/// is wider than its progressive file (`width`), as it is on 165 of 180
/// post videos of the capture of 2026-11-13.
fn best_of(item: &Media, width: Option<u32>) -> Option<Best> {
    let tracks = dash::tracks(item.video_dash_manifest.as_deref()?)?;
    let wider = tracks.video.width.unwrap_or(0) > width.unwrap_or(0);
    wider.then_some(Best {
        video: tracks.video,
        audio: tracks.audio,
    })
}

/// `1234567` as `1,234,567`.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A video's length, `0:42` or `1:02:03`, from seconds; a part of a second
/// is dropped, as the app's player shows it.
pub fn length(seconds: f64) -> String {
    // Saturates on anything past u64, which `seconds` from a post is not.
    let whole = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (whole / 3600, whole / 60 % 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The line that says what a post plays: "Music: <title> · <artist>" for a
/// song, "Sound: <title> · <artist>" for a video's own; `None` when it says
/// neither.
pub fn music_line(music: &Music) -> Option<String> {
    let what = if music.kind == "song" {
        "Music"
    } else {
        "Sound"
    };
    let parts: Vec<&str> = [music.title.as_deref(), music.artist.as_deref()]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect();
    (!parts.is_empty()).then(|| format!("{what}: {}", parts.join(" · ")))
}

/// `pk` when it is one: digits, and nothing else Instagram might put there.
fn digits(pk: &str) -> Option<String> {
    (!pk.is_empty() && pk.bytes().all(|b| b.is_ascii_digit())).then(|| pk.to_string())
}

/// A length in seconds, when it is one.
fn seconds(duration: Option<f64>) -> Option<f64> {
    duration.filter(|d| d.is_finite() && *d >= 0.0)
}

/// An account a post names, ready for a terminal: its name filtered, its pk
/// held to digits.
fn account(named: &NamedAccount) -> AccountRef {
    AccountRef {
        pk: named.pk.as_deref().and_then(digits),
        username: printable(&named.username),
    }
}

/// `posts`, each read again by its info, a step apart: what a tab's posts take
/// before they are looked into or saved, since a tab hands out no video's
/// address and a carousel's items as covers alone.
pub async fn filled(client: &IgClient, posts: Vec<Post>) -> Result<Option<Vec<Post>>> {
    let mut filled = Vec::with_capacity(posts.len());
    for post in posts {
        if client.step_between(&Pace::default()).await {
            return Ok(None);
        }
        let fresh = client.media_info(post.pk, &post.page()).await?;
        filled.push(match fresh {
            Some(media) => {
                let mut fresh = Post::from_media(&media, &post.owner);
                fresh.opens = post.opens;
                fresh.asked_from = post.asked_from;
                fresh.notes = post.notes;
                fresh
            }
            None => post,
        });
    }
    Ok(Some(filled))
}

/// A post opened the way the app opens one: its info read again and its first
/// page of comments, the pair the app sends.
pub async fn open(client: &IgClient, post: &Post) -> Result<(Post, Comments)> {
    let page = post.page();
    let fresh = client.media_info(post.pk, &page).await?;
    let post = fresh
        .as_ref()
        .map(|media| {
            let mut fresh = Post::from_media(media, &post.owner);
            fresh.opens = post.opens;
            fresh.asked_from = post.asked_from;
            fresh
        })
        .unwrap_or_else(|| post.clone());
    let mut comments = Comments::default();
    comments.take(&client.media_comments(post.pk, None, &page).await?);
    Ok((post, comments))
}

/// The post `link` names, read the way the app reads one it opens: its info,
/// from the page it opens on.
pub async fn fetch_link(client: &IgClient, link: &Shortcode) -> Result<Post> {
    let media = client.media_info(link.pk(), &link.page()).await?;
    let media = media.ok_or_else(|| {
        ExitError::new(
            ExitCode::Error,
            format!(
                "no post answers to {}: it was deleted, or it is not shown to you",
                link.code()
            ),
        )
    })?;
    let mut post = Post::from_media(&media, "");
    // Where it was asked from is where it opens, whatever kind it is.
    post.opens = link.opens();
    post.asked_from = link.opens();
    post.code = link.code().to_string();
    Ok(post)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::Ffmpeg;
    use serde_json::Value;

    fn media(json: &str) -> Media {
        serde_json::from_str(json).unwrap()
    }

    /// A one-item post whose pictures are `candidates`, a JSON array.
    fn thumb_among(candidates: &str) -> Option<String> {
        let json = format!(
            r#"{{"pk":"9","media_type":1,"image_versions2":{{"candidates":{candidates}}}}}"#
        );
        Post::from_media(&media(&json), "x").items[0].thumb.clone()
    }

    #[test]
    fn the_thumbnail_is_the_narrowest_picture_wide_enough() {
        let picked = thumb_among(
            r#"[{"url":"https://x.cdninstagram.com/1440.jpg","width":1440},
                {"url":"https://x.cdninstagram.com/640.jpg","width":640},
                {"url":"https://x.cdninstagram.com/320.jpg","width":320},
                {"url":"https://x.cdninstagram.com/150.jpg","width":150}]"#,
        );
        assert_eq!(
            picked.as_deref(),
            Some("https://x.cdninstagram.com/640.jpg")
        );
    }

    #[test]
    fn with_none_wide_enough_the_thumbnail_is_the_widest() {
        let picked = thumb_among(
            r#"[{"url":"https://x.cdninstagram.com/150.jpg","width":150},
                {"url":"https://x.cdninstagram.com/240.jpg","width":240}]"#,
        );
        assert_eq!(
            picked.as_deref(),
            Some("https://x.cdninstagram.com/240.jpg")
        );
    }

    #[test]
    fn of_two_thumbnails_alike_the_first_listed_is_taken() {
        let wide = thumb_among(
            r#"[{"url":"https://x.cdninstagram.com/a.jpg","width":800},
                {"url":"https://x.cdninstagram.com/b.jpg","width":800}]"#,
        );
        assert_eq!(wide.as_deref(), Some("https://x.cdninstagram.com/a.jpg"));
        let narrow = thumb_among(
            r#"[{"url":"https://x.cdninstagram.com/c.jpg","width":200},
                {"url":"https://x.cdninstagram.com/d.jpg","width":200}]"#,
        );
        assert_eq!(narrow.as_deref(), Some("https://x.cdninstagram.com/c.jpg"));
    }

    #[test]
    fn a_carousel_handed_over_without_its_items_is_drawn_from_its_own_pictures() {
        let post = Post::from_media(
            &media(
                r#"{"pk":"6","media_type":8,"carousel_media_count":3,
                "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/post.jpg","width":640}]}}"#,
            ),
            "x",
        );
        assert_eq!(post.items.len(), 1);
        assert_eq!(
            post.items[0].thumb.as_deref(),
            Some("https://x.cdninstagram.com/post.jpg")
        );
    }

    #[test]
    fn a_videos_thumbnail_is_its_cover_and_each_item_has_its_own() {
        let post = Post::from_media(
            &media(
                r#"{"pk":"5","media_type":8,
                "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/post.jpg","width":640}]},
                "carousel_media":[
                  {"pk":"51","media_type":1,
                   "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/one.jpg","width":640}]}},
                  {"pk":"52","media_type":2,
                   "video_versions":[{"url":"https://x.cdninstagram.com/two.mp4","width":720}],
                   "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/cover.jpg","width":640}]}},
                  {"pk":"53","media_type":1}
                ]}"#,
            ),
            "x",
        );
        let thumbs: Vec<Option<&str>> = post.items.iter().map(|i| i.thumb.as_deref()).collect();
        assert_eq!(
            thumbs,
            [
                Some("https://x.cdninstagram.com/one.jpg"),
                Some("https://x.cdninstagram.com/cover.jpg"),
                // Never another item's picture, the post's cover included.
                None,
            ]
        );
    }

    #[test]
    fn the_thumbnail_stays_out_of_the_json() {
        for (post, number) in golden_posts() {
            let json = serde_json::to_string(&post_json(&post, number)).unwrap();
            assert!(!json.contains("thumb"));
        }
    }

    #[test]
    fn a_carousel_is_its_items_and_names_each_file() {
        let post = Post::from_media(
            &media(
                r#"{"pk":"3995556159532654737","code":"DdzEShhEWCRnxelCuqsxG5vhMRx3FuF81XXoEM0","media_type":8,
                "caption":{"text":"first line\n\nsecond with @friend"},"like_count":80,"top_likers":["close"],
                "carousel_media":[
                  {"pk":"1","media_type":1,"original_width":1080,"original_height":1350,
                   "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/a.jpg","width":1080,"height":1350}]}},
                  {"pk":"2","media_type":2,"video_versions":[{"url":"https://x.cdninstagram.com/b.mp4","width":720,"height":1280}]}
                ]}"#,
            ),
            "someone",
        );
        assert_eq!(post.kind, PostKind::Carousel(2));
        assert_eq!(post.kind.label(), "2 items");
        assert_eq!(post.owner, "someone");
        assert_eq!(post.caption, ["first line", "second with @friend"]);
        assert_eq!(post.mentions, ["friend"]);
        assert_eq!(post.stem(0), "someone-DdzEShhEWCR-1");
        assert_eq!(post.stem(1), "someone-DdzEShhEWCR-2");
        assert_eq!(post.items[1].kind, Kind::Video);
        assert_eq!(post.items[1].width, Some(720));
        assert_eq!(
            post.liked_line().as_deref(),
            Some("Liked by close and 79 others")
        );
        assert_eq!(
            post.link(),
            "https://www.instagram.com/p/DdzEShhEWCRnxelCuqsxG5vhMRx3FuF81XXoEM0/"
        );
    }

    #[test]
    fn a_reel_is_one_file_named_without_a_number() {
        let post = Post::from_media(
            &media(
                r#"{"pk":"3947056156557494178","product_type":"clips","media_type":2,"like_count":2000,
                "like_and_view_counts_disabled":false,"play_count":4017638,
                "user":{"username":"thegrefg"},
                "video_versions":[{"url":"https://x.cdninstagram.com/r.mp4","width":720,"height":1280}]}"#,
            ),
            "",
        );
        assert_eq!(post.kind, PostKind::Reel);
        assert_eq!(post.code, "DbGwql4IMei", "made from the pk when absent");
        // Asked about from `/p/`, as the app opens a reel from a grid, and
        // still a reel's address when it is printed.
        assert_eq!(post.page(), "/p/DbGwql4IMei/");
        assert_eq!(post.link(), "https://www.instagram.com/reel/DbGwql4IMei/");
        assert_eq!(post.stem(0), "thegrefg-DbGwql4IMei");
        assert_eq!(post.liked_line().as_deref(), Some("2,000 likes"));
        assert_eq!(post.plays, Some(4_017_638));
        let json = post_json(&post, Some(3));
        assert_eq!(json["number"], 3);
        assert_eq!(json["kind"], "reel");
        assert_eq!(json["items"][0]["kind"], "video");
    }

    #[test]
    fn hidden_likes_are_not_counted() {
        let post = Post::from_media(
            &media(r#"{"pk":"1","like_count":5,"like_and_view_counts_disabled":true}"#),
            "x",
        );
        assert_eq!(post.liked_line(), None);
        let named = Post::from_media(
            &media(
                r#"{"pk":"1","like_count":5,"like_and_view_counts_disabled":true,"top_likers":["a"]}"#,
            ),
            "x",
        );
        assert_eq!(named.liked_line().as_deref(), Some("Liked by a and others"));
        assert_eq!(post_json(&post, None)["likes"], Value::Null);
    }

    /// The four shapes a post's JSON takes, made of invented accounts: a
    /// photo with a place and a liker, a carousel with a video, a reel with
    /// plays and hidden counts, and a post with no caption and no counts.
    fn golden_posts() -> [(Post, Option<usize>); 4] {
        let photo = Post::from_media(
            &media(
                r#"{"pk":"3995556159532654737","code":"DdzEShhEWCR","media_type":1,"taken_at":1790000000,
                "caption":{"text":"a line\nwith @friend"},"like_count":80,"comment_count":4,
                "top_likers":["close"],"location":{"name":"Somewhere"},
                "user":{"username":"owner.one"},
                "usertags":{"in":[{"user":{"username":"tagged.one"}}]},
                "coauthor_producers":[{"username":"with.one"}],
                "original_width":1080,"original_height":1350,
                "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/a.jpg","width":1080,"height":1350}]}}"#,
            ),
            "",
        );
        let carousel = Post::from_media(
            &media(
                r#"{"pk":"2","media_type":8,"taken_at":1790000001,"like_count":5,
                "carousel_media":[
                  {"pk":"21","media_type":1,"original_width":1080,"original_height":1080,
                   "image_versions2":{"candidates":[{"url":"https://x.cdninstagram.com/c.jpg","width":1080,"height":1080}]}},
                  {"pk":"22","media_type":2,"video_versions":[{"url":"https://x.cdninstagram.com/d.mp4","width":720,"height":1280}]}
                ]}"#,
            ),
            "owner.two",
        );
        let reel = Post::from_media(
            &media(
                r#"{"pk":"3","product_type":"clips","media_type":2,"taken_at":1790000002,"like_count":3,
                "like_and_view_counts_disabled":true,"play_count":1234,"comment_count":0,
                "top_likers":["close"],
                "video_versions":[{"url":"https://x.cdninstagram.com/r.mp4","width":720,"height":1280}]}"#,
            ),
            "owner.three",
        );
        let bare = Post::from_media(&media(r#"{"pk":"4"}"#), "owner.four");
        [
            (photo, Some(1)),
            (carousel, Some(2)),
            (reel, None),
            (bare, None),
        ]
    }

    /// `printed` with only the keys `golden` has, at every depth: what the
    /// keys a document always had say, whatever keys were added beside them.
    fn only_the_keys_of(printed: &Value, golden: &Value) -> Value {
        match (printed, golden) {
            (Value::Object(printed), Value::Object(golden)) => Value::Object(
                golden
                    .iter()
                    .map(|(key, old)| {
                        let new = printed.get(key).cloned().unwrap_or(Value::Null);
                        (key.clone(), only_the_keys_of(&new, old))
                    })
                    .collect(),
            ),
            (Value::Array(printed), Value::Array(golden)) => Value::Array(
                printed
                    .iter()
                    .zip(golden.iter().chain(std::iter::repeat(&Value::Null)))
                    .map(|(new, old)| only_the_keys_of(new, old))
                    .collect(),
            ),
            (printed, _) => printed.clone(),
        }
    }

    /// The keys a post's JSON has always had, byte for byte: the contract
    /// of `snob posts` and `snob post`, which typed documents and the keys
    /// added since must not change. Taken from the output of 2026-11-13,
    /// before either.
    #[test]
    fn a_post_serializes_to_the_keys_it_always_had() {
        let golden = [
            r#"{"caption":"a line\nwith @friend","code":"DdzEShhEWCR","comments":4,"items":[{"height":1350,"kind":"photo","number":1,"url":"https://x.cdninstagram.com/a.jpg","width":1080}],"kind":"photo","liked_by":{"name":"close","others":79},"likes":80,"location":"Somewhere","mentions":["friend"],"number":1,"owner":"owner.one","pk":"3995556159532654737","plays":null,"tagged":["tagged.one"],"taken_at":1790000000,"url":"https://www.instagram.com/p/DdzEShhEWCR/","with":["with.one"]}"#,
            r#"{"caption":"","code":"AAAAAAAAAAC","comments":null,"items":[{"height":1080,"kind":"photo","number":1,"url":"https://x.cdninstagram.com/c.jpg","width":1080},{"height":1280,"kind":"video","number":2,"url":"https://x.cdninstagram.com/d.mp4","width":720}],"kind":"carousel","liked_by":null,"likes":5,"location":null,"mentions":[],"number":2,"owner":"owner.two","pk":"2","plays":null,"tagged":[],"taken_at":1790000001,"url":"https://www.instagram.com/p/AAAAAAAAAAC/","with":[]}"#,
            r#"{"caption":"","code":"AAAAAAAAAAD","comments":0,"items":[{"height":1280,"kind":"video","number":1,"url":"https://x.cdninstagram.com/r.mp4","width":720}],"kind":"reel","liked_by":{"name":"close","others":null},"likes":null,"location":null,"mentions":[],"owner":"owner.three","pk":"3","plays":1234,"tagged":[],"taken_at":1790000002,"url":"https://www.instagram.com/reel/AAAAAAAAAAD/","with":[]}"#,
            r#"{"caption":"","code":"AAAAAAAAAAE","comments":null,"items":[{"height":null,"kind":"photo","number":1,"url":null,"width":null}],"kind":"photo","liked_by":null,"likes":null,"location":null,"mentions":[],"owner":"owner.four","pk":"4","plays":null,"tagged":[],"taken_at":0,"url":"https://www.instagram.com/p/AAAAAAAAAAE/","with":[]}"#,
        ];
        for ((post, number), golden) in golden_posts().into_iter().zip(golden) {
            let old: Value = serde_json::from_str(golden).unwrap();
            let kept = only_the_keys_of(&post_json(&post, number), &old);
            assert_eq!(serde_json::to_string(&kept).unwrap(), golden);
        }
    }

    /// A comment's keys, byte for byte, its shown replies under it.
    #[test]
    fn a_comment_serializes_to_the_keys_it_always_had() {
        let reply = CommentLine {
            author: "replier".into(),
            text: "a reply".into(),
            at: Epoch::new(1_790_000_010),
            likes: 0,
            replies_count: 0,
            replies: Vec::new(),
            pk: Some("18000000000000002".into()),
            author_pk: Some("2".into()),
            more_replies: None,
        };
        let comment = CommentLine {
            author: "commenter".into(),
            text: "a comment".into(),
            at: Epoch::new(1_790_000_005),
            likes: 7,
            replies_count: 3,
            replies: vec![reply],
            pk: Some("18000000000000001".into()),
            author_pk: None,
            more_replies: None,
        };
        let golden = concat!(
            r#"{"author":"commenter","created_at":1790000005,"likes":7,"replies":3,"#,
            r#""shown_replies":[{"author":"replier","created_at":1790000010,"likes":0,"#,
            r#""replies":0,"shown_replies":[],"text":"a reply"}],"text":"a comment"}"#
        );
        let printed = comment_json(&comment);
        let old: Value = serde_json::from_str(golden).unwrap();
        assert_eq!(
            serde_json::to_string(&only_the_keys_of(&printed, &old)).unwrap(),
            golden
        );
        // And the keys added beside them.
        assert_eq!(printed["pk"], "18000000000000001");
        assert_eq!(printed["author_pk"], Value::Null);
        assert_eq!(printed["shown_replies"][0]["author_pk"], "2");
    }

    /// The line under a post names the likers the viewer knows, up to three,
    /// and counts the rest from the likes.
    #[test]
    fn the_liked_line_names_up_to_three_and_counts_the_rest() {
        let liked = |likes: u64, faces: &str| {
            Post::from_media(
                &media(&format!(
                    r#"{{"pk":"1","like_count":{likes},"facepile_top_likers":[{faces}]}}"#
                )),
                "x",
            )
            .liked_line()
        };
        let three =
            r#"{"pk":"1","username":"a"},{"pk":"2","username":"b"},{"pk":"3","username":"c"}"#;
        assert_eq!(
            liked(1_003, three).as_deref(),
            Some("Liked by a, b, c and 1,000 others")
        );
        assert_eq!(
            liked(4, three).as_deref(),
            Some("Liked by a, b, c and 1 other")
        );
        assert_eq!(liked(3, three).as_deref(), Some("Liked by a, b and c"));
        assert_eq!(
            liked(1, r#"{"pk":"1","username":"a"}"#).as_deref(),
            Some("Liked by a")
        );
        assert_eq!(liked(5, "").as_deref(), Some("5 likes"));
    }

    /// Under hidden counts the like count is a placeholder, 3 on every one
    /// of 21 such posts in the capture of 2026-11-13: never said, in a line
    /// or in the JSON.
    #[test]
    fn a_hidden_like_count_is_never_read_as_three() {
        let post = Post::from_media(
            &media(
                r#"{"pk":"1","like_count":3,"like_and_view_counts_disabled":true,
                "facepile_top_likers":[{"pk":"2","username":"a"}]}"#,
            ),
            "x",
        );
        assert_eq!(post.liked_line().as_deref(), Some("Liked by a and others"));
        let json = post_json(&post, None);
        assert_eq!(json["likes"], Value::Null);
        assert_eq!(json["likes_hidden"], true);
        assert!(!json.to_string().contains(":3"), "{json}");
    }

    /// What a post says beyond its counts goes into its JSON beside the keys
    /// it always had: the accounts by pk, the reposts and shares, the plays
    /// by network, the length, the music, the place with where it is, and
    /// its marks; and per item its own id and description.
    #[test]
    fn a_post_carries_what_its_answer_says_beside_the_old_keys() {
        let post = Post::from_media(
            &media(
                r#"{"pk":"5","product_type":"clips","media_type":2,"like_count":10,
                "user":{"pk":"42","username":"owner"},
                "coauthor_producers":[{"pk":"7","username":"with.one"}],
                "usertags":{"in":[{"user":{"pk":8,"username":"tagged.one"}}]},
                "facepile_top_likers":[{"pk":"9","username":"liker"}],
                "media_repost_count":4,"reshare_count":6,"share_count_disabled":false,
                "play_count":100,"ig_play_count":90,"fb_play_count":10,"video_duration":42.7,
                "clips_metadata":{"original_sound_info":{"original_audio_title":"Original audio",
                  "ig_artist":{"username":"owner"}}},
                "location":{"pk":"77","name":"Somewhere","lat":28.1,"lng":-15.4},
                "timeline_pinned_user_ids":[42],"is_paid_partnership":true,"caption_is_edited":true,
                "accessibility_caption":"A view",
                "video_versions":[{"url":"https://x.cdninstagram.com/r.mp4","width":720,"height":1280}]}"#,
            ),
            "",
        );
        let json = post_json(&post, None);
        assert_eq!(json["owner_pk"], "42");
        assert_eq!(
            json["with_accounts"],
            serde_json::json!([{"pk": "7", "username": "with.one"}])
        );
        assert_eq!(
            json["tagged_accounts"],
            serde_json::json!([{"pk": "8", "username": "tagged.one"}])
        );
        assert_eq!(
            json["likers_known"],
            serde_json::json!([{"pk": "9", "username": "liker"}])
        );
        assert_eq!(json["reposts"], 4);
        assert_eq!(json["shares"], 6);
        assert_eq!(json["plays"], 100);
        assert_eq!(json["plays_instagram"], 90);
        assert_eq!(json["plays_facebook"], 10);
        assert_eq!(json["duration"], 42.7);
        assert_eq!(json["music"]["kind"], "original");
        assert_eq!(json["music"]["title"], "Original audio");
        assert_eq!(json["place"]["pk"], "77");
        assert_eq!(json["place"]["lat"], 28.1);
        assert_eq!(json["location"], "Somewhere");
        assert_eq!(json["pinned"], true);
        assert_eq!(json["paid_partnership"], true);
        assert_eq!(json["caption_edited"], true);
        assert_eq!(json["items"][0]["pk"], "5");
        assert_eq!(json["items"][0]["alt"], "A view");
        assert_eq!(json["items"][0]["duration"], 42.7);
        assert_eq!(json["likes_hidden"], false);

        // Shares turned off are no count, and a photo says none.
        let off = Post::from_media(
            &media(r#"{"pk":"6","reshare_count":6,"share_count_disabled":true}"#),
            "x",
        );
        assert_eq!(post_json(&off, None)["shares"], Value::Null);
        // A pk that is not digits is not passed on.
        let odd = Post::from_media(&media(r#"{"pk":"6","user":{"pk":"4a2"}}"#), "x");
        assert_eq!(post_json(&odd, None)["owner_pk"], Value::Null);
    }

    #[test]
    fn a_length_reads_as_the_player_shows_it() {
        assert_eq!(length(0.0), "0:00");
        assert_eq!(length(42.7), "0:42");
        assert_eq!(length(62.0), "1:02");
        assert_eq!(length(3_723.0), "1:02:03");
        assert_eq!(length(-5.0), "0:00");
    }

    #[test]
    fn a_music_line_says_what_plays() {
        let music = |kind: &str, title: Option<&str>, artist: Option<&str>| Music {
            kind: kind.into(),
            title: title.map(str::to_string),
            artist: artist.map(str::to_string),
            artist_username: None,
        };
        assert_eq!(
            music_line(&music("song", Some("A Song"), Some("Singer"))).as_deref(),
            Some("Music: A Song · Singer")
        );
        assert_eq!(
            music_line(&music("original", Some("Original audio"), None)).as_deref(),
            Some("Sound: Original audio")
        );
        assert_eq!(music_line(&music("song", None, Some(" "))), None);
    }

    /// A video's best tracks are read from its manifest only when wider than
    /// its progressive file; compat drops them, best keeps them with the
    /// ffmpeg that joins them.
    #[test]
    fn a_video_is_saved_at_its_best_only_when_asked_and_wider() {
        let manifest = |width: u32| {
            format!(
                concat!(
                    r#"<MPD><Representation bandwidth="1" mimeType="video/mp4" width="{w}" height="1920">"#,
                    r#"<BaseURL>https://x.fbcdn.net/v</BaseURL></Representation>"#,
                    r#"<Representation bandwidth="1" mimeType="audio/mp4">"#,
                    r#"<BaseURL>https://x.fbcdn.net/a</BaseURL></Representation></MPD>"#
                ),
                w = width
            )
        };
        let reel = |width: u32| {
            Post::from_media(
                &media(
                    &serde_json::json!({
                        "pk": "5", "media_type": 2,
                        "video_versions": [{"url": "https://x.cdninstagram.com/r.mp4", "width": 720, "height": 1280}],
                        "video_dash_manifest": manifest(width),
                    })
                    .to_string(),
                ),
                "x",
            )
        };
        let wider = reel(1080);
        let best = wider.items[0].best.as_ref().expect("wider than 720");
        assert_eq!(best.video.url, "https://x.fbcdn.net/v");
        assert_eq!(
            best.audio.as_ref().map(|a| a.url.as_str()),
            Some("https://x.fbcdn.net/a")
        );
        assert!(reel(720).items[0].best.is_none(), "not wider");

        let compat = Videos::Compat { say_how: false }.prepare(vec![wider.file(0)]);
        assert!(compat[0].best.is_none() && compat[0].ffmpeg.is_none());
        let ffmpeg =
            std::sync::Arc::new(Ffmpeg::find(Some(&std::env::current_exe().unwrap())).unwrap());
        let at_best = Videos::Best(ffmpeg).prepare(vec![wider.file(0)]);
        assert!(at_best[0].best.is_some() && at_best[0].ffmpeg.is_some());
    }

    #[test]
    fn numbers_are_grouped_in_threes() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(4_017_638), "4,017,638");
    }
}
