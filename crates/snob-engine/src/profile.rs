//! An account the way its page shows it (`snob profile`, the app's profile
//! page, a list's card), and its picture at the best size the web serves
//! (`snob pfp`): one read for every front.
//!
//! **What it costs**: one request for the profile, which is the one every
//! command makes to turn a name into an id; one for the highlights and one for
//! the stories, both skipped when the account is private and not followed,
//! because neither is served then, and when the profile already says there
//! are none; and one per twelve accounts you follow that follow them, only
//! when asked for ([`MutualPolicy`]). A picture is one download from the CDN,
//! which Instagram's pace does not count.
//!
//! **It asks no consent** (AGENTS.md, "Consent"): the one list it walks is the
//! viewer's own following. **Reading a highlight's tray or a reel tells nobody
//! you looked**: `crates/snob-core/tests/no_seen.rs` holds it.

use anyhow::{Result, anyhow};
use snob_core::model::{User, printable};
use snob_core::{Epoch, Pk};
use snob_ig::client::IgClient;
use snob_ig::model::{Via, WebProfileInfo};
use snob_ig::pace::Pace;

use crate::app::App;
use crate::engine::target;
use crate::media::{self, Story};

/// How many pages of the mutual list are walked before the rest is counted
/// rather than named.
///
/// Twelve names a page is what the endpoint serves, so this is 120 names for
/// ten requests.
const MUTUAL_PAGE_CAP: usize = 10;

/// What the page shows.
#[derive(Debug)]
pub struct Profile {
    /// The account's id, kept so a view can page and fetch without
    /// resolving again the name it already paid to resolve.
    pub pk: Pk,
    /// As Instagram spells it, not as it was typed.
    pub username: String,
    pub full_name: Option<String>,
    pub biography: Option<String>,
    pub external_url: Option<String>,
    pub is_private: bool,
    pub is_verified: bool,
    /// The category a business or creator account shows under its name.
    pub category: Option<String>,
    pub followers: Option<u64>,
    pub following: Option<u64>,
    pub posts: Option<u64>,
    /// How the viewer and this account stand to each other; `None` for the
    /// viewer's own.
    pub relation: Option<Relation>,
    /// The accounts you follow that follow this one; `None` for the viewer's
    /// own.
    pub mutual: Option<Mutual>,
    pub highlights: Visibility<Vec<Highlight>>,
    /// The address of the profile picture at the size the page serves:
    /// the fallback [`picture`] takes, so looking at the picture from a view
    /// costs one request, not two.
    pub pfp_url: Option<String>,
    /// The address of the full-size picture, when the profile read carries
    /// it: from the browser it does, and looking at the picture then costs
    /// no request at all.
    pub pfp_full_size: Option<String>,
    /// The address of the small picture a list row shows.
    pub pfp_small: Option<String>,
    /// The account keeps the default avatar, which is no picture to show.
    pub pfp_anonymous: bool,
    /// Whether a story is up, as the profile says it for no request.
    pub story_up: bool,
    /// The stories up right now; `None` when they were not asked for
    /// ([`Reads::stories`]).
    pub stories: Option<Visibility<Vec<Story>>>,
    /// The date the summary describes.
    pub read_at: Epoch,
}

/// Which way the follows go between the viewer and the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Relation {
    pub you_follow: bool,
    pub follows_you: bool,
    /// You asked and they have not answered.
    pub you_requested: bool,
    /// They asked and you have not answered.
    pub they_requested: bool,
}

/// "Followed by a, b and 31 others", and as much of the list as was walked.
#[derive(Debug, Default)]
pub struct Mutual {
    /// The count the page shows, which is Instagram's and not a length here.
    pub count: u64,
    /// The three names the page itself puts in the sentence.
    pub preview: Vec<String>,
    /// The names, in the order the mutual tab lists them.
    pub people: Vec<User>,
    /// Whether `people` is the whole list.
    pub complete: bool,
}

/// A part of the page that a private account keeps from those who do not
/// follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility<T> {
    Shown(T),
    /// Private, and the viewer does not follow it.
    Hidden,
}

/// One highlight as the tray lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// `highlight:<id>`, the spelling the items are fetched with.
    pub id: String,
    /// Filtered: it came off somebody else's profile. Empty when the
    /// highlight has none, which happens.
    pub title: String,
    pub items: Option<u64>,
    /// When something was last added to it.
    pub updated_at: Option<Epoch>,
    /// Its cover's address, for a view to draw ([`cover_of`]). Not in the
    /// JSON contract.
    pub cover: Option<String>,
}

impl Profile {
    /// How many stories are up, when they were read.
    pub fn stories_up(&self) -> Option<Visibility<usize>> {
        Some(match self.stories.as_ref()? {
            Visibility::Shown(items) => Visibility::Shown(items.len()),
            Visibility::Hidden => Visibility::Hidden,
        })
    }
}

/// Whether a read walks the mutual list or leaves it for a later click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutualPolicy {
    WalkNow,
    Defer,
}

/// What a read asks for besides the profile and the highlights' tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reads {
    pub mutual: MutualPolicy,
    /// The reel of stories up; without it, [`Profile::story_up`] alone says
    /// whether there are any.
    pub stories: bool,
}

impl Reads {
    /// What `snob profile` reads: the stories too.
    pub fn all(mutual: MutualPolicy) -> Self {
        Self {
            mutual,
            stories: true,
        }
    }
}

/// The profile named `typed`, read through `app`, which keeps what the
/// profile answered in the account's cache (`App::profile_named`).
pub async fn read(app: &App, typed: &str, reads: Reads, known: Option<Pk>) -> Result<Profile> {
    let info = app.profile_named(target::clean(typed), known).await?;
    let profile = of(app.client(), info, app.viewer().pk, reads).await?;
    // The tray this read asked for, or knew it had none of: kept for a view
    // to show at once next time (`cache`).
    if let Some(keeper) = app.cache() {
        keeper.tray(profile.pk, &profile.highlights);
    }
    Ok(profile)
}

/// [`read`] on a client alone, which keeps nothing: for a test against a
/// mock server.
pub async fn fetch(
    client: &IgClient,
    typed: &str,
    viewer: Pk,
    reads: Reads,
    known: Option<Pk>,
) -> Result<Profile> {
    let info = client.profile_named(target::clean(typed), known).await?;
    of(client, info, viewer, reads).await
}

/// The rest of the page, once the profile is read.
async fn of(client: &IgClient, info: WebProfileInfo, viewer: Pk, reads: Reads) -> Result<Profile> {
    let own = info.id == viewer;
    let is_private = info.is_private.unwrap_or(false);
    let you_follow = info.followed_by_viewer.unwrap_or(false);

    // A private account serves its reels, the highlights and the stories,
    // only to its followers.
    let visible = own || !is_private || you_follow;

    let relation = (!own).then(|| Relation {
        you_follow,
        follows_you: info.follows_viewer.unwrap_or(false),
        you_requested: info.requested_by_viewer.unwrap_or(false),
        they_requested: info.has_requested_viewer.unwrap_or(false),
    });

    let mutual_count = info.mutual.as_ref().map(|m| m.count).unwrap_or(0);
    let mutual_preview: Vec<String> = info
        .mutual
        .as_ref()
        .map(|m| m.names().map(printable).collect())
        .unwrap_or_default();
    let mutual = if own {
        None
    } else {
        Some(match reads.mutual {
            MutualPolicy::WalkNow => {
                mutuals(
                    client,
                    info.id,
                    &info.username,
                    mutual_count,
                    mutual_preview,
                )
                .await?
            }
            // Nothing spent and nothing claimed: an empty, incomplete list is
            // "not walked yet", which is exactly what it is.
            MutualPolicy::Defer => Mutual {
                count: mutual_count,
                preview: mutual_preview,
                people: Vec::new(),
                complete: false,
            },
        })
    };

    // The profile already says how many there are; a count of zero is the
    // answer, and asking the tray for it spent a request on an empty list.
    let highlights = if visible && info.highlight_reel_count == Some(0) {
        Visibility::Shown(Vec::new())
    } else if visible {
        let tray = client.highlights_tray(info.id, &info.username).await?;
        Visibility::Shown(
            tray.into_iter()
                .map(|h| Highlight {
                    cover: cover_of(&h),
                    id: h.id,
                    title: printable(h.title.as_deref().unwrap_or("")),
                    items: h.media_count,
                    updated_at: h.updated_timestamp,
                })
                .collect(),
        )
    } else {
        Visibility::Hidden
    };

    // The profile says when the newest story was posted, and zero is none
    // up: the same answer the reel would give, for no request.
    let story_up = visible && info.latest_reel_media.is_some_and(|at| at > 0);
    let stories = if !reads.stories {
        None
    } else if visible && info.latest_reel_media == Some(0) {
        Some(Visibility::Shown(Vec::new()))
    } else if visible {
        let reel = client.stories(info.id, &info.username).await?;
        Some(Visibility::Shown(
            reel.map(|r| r.items.iter().map(media::story_from).collect())
                .unwrap_or_default(),
        ))
    } else {
        Some(Visibility::Hidden)
    };

    let said = |text: Option<&str>| text.map(printable).filter(|s| !s.is_empty());
    Ok(Profile {
        pk: info.id,
        username: printable(&info.username),
        full_name: said(info.full_name.as_deref()),
        // Filtered a line at a time: `printable` turns a newline into a space,
        // and a bio is the one text here whose line breaks are its author's.
        biography: info
            .biography
            .as_deref()
            .map(|bio| bio.lines().map(printable).collect::<Vec<_>>().join("\n"))
            .filter(|s| !s.trim().is_empty()),
        // A URL is server-supplied text, and a control character in it
        // reaches a terminal all the same.
        external_url: said(info.external_url.as_deref()),
        is_private,
        is_verified: info.is_verified.unwrap_or(false),
        category: said(info.category_name.as_deref()),
        followers: info.follower_count(),
        following: info.following_count(),
        posts: info.posts.map(|e| e.count),
        relation,
        mutual,
        highlights,
        pfp_url: info
            .profile_pic_url_hd
            .clone()
            .or(info.profile_pic_url.clone()),
        pfp_full_size: full_size_of(&info),
        pfp_small: info.profile_pic_url.clone(),
        pfp_anonymous: info.anonymous_picture == Some(true),
        story_up,
        stories,
        read_at: snob_core::clock::now(),
    })
}

/// The accounts you follow that follow this one, walked a page at a time.
pub async fn mutuals(
    client: &IgClient,
    pk: Pk,
    username: &str,
    count: u64,
    preview: Vec<String>,
) -> Result<Mutual> {
    if count == 0 {
        return Ok(Mutual {
            count,
            preview,
            people: Vec::new(),
            complete: true,
        });
    }

    // **A rest a sitting owes is paid first**, before the list is opened,
    // as a list walk pays it: these pages are part of the account's sitting.
    if rest_before_a_page(client).await? {
        return Err(snob_ig::error::IgError::Canceled.into());
    }
    // **Opened as a person opens it**: a look at the profile first, as long
    // as a list walk waits before its first page (`Pace::dwell`), and then
    // the app's navigation to the mutual tab, which has an address of its
    // own (`IgClient::open_list`).
    if client.dwell_before_a_list(&Pace::default()).await {
        return Err(snob_ig::error::IgError::Canceled.into());
    }
    client.open_list(username, true).await?;

    let mut people: Vec<User> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut complete = false;
    for page_number in 0..MUTUAL_PAGE_CAP {
        // **A step between pages, as a person scrolling the list makes**, the
        // same one a list walk takes.
        if page_number > 0
            && (rest_before_a_page(client).await? || client.step_between(&Pace::default()).await)
        {
            return Err(snob_ig::error::IgError::Canceled.into());
        }
        let page = client
            .mutual_followers_page(pk, username, cursor.as_deref())
            .await?;
        people.extend(page.users.iter().map(User::from));
        // Part of the sitting a list walk rests after: a page that fills it
        // owes the rest to the next page anybody walks, this list's next
        // one included.
        client
            .pacer()
            .page_walked(&Pace::default(), snob_core::clock::now_ms())
            .await?;
        match page.next_max_id.filter(|c| !c.is_empty()) {
            Some(next) => cursor = Some(next),
            None => {
                complete = true;
                break;
            }
        }
    }
    Ok(Mutual {
        count,
        preview,
        people,
        complete,
    })
}

/// Pays a rest the account's sitting owes before a page of the mutual list,
/// saying so (`host::info`): no bar draws for this walk, and a front would
/// otherwise stand still for minutes without a word.
async fn rest_before_a_page(client: &IgClient) -> Result<bool> {
    Ok(client
        .rest_before_a_page(&Pace::default(), |left| {
            crate::host::info(&format!(
                "Resting {} between sittings before the next page of the list.",
                crate::sentences::rest_left(left)
            ));
        })
        .await?)
}

/// The address of `highlight`'s cover, the square picture the profile
/// shows above its title, as the tray hands it out
/// (`cover_media.cropped_image_version`, on the REST and GraphQL trays
/// alike).
pub fn cover_of(highlight: &snob_ig::model::Highlight) -> Option<String> {
    highlight
        .cover_media
        .as_ref()?
        .cropped_image_version
        .as_ref()
        .map(|v| v.url.clone())
        .filter(|url| !url.is_empty())
}

/// A profile picture, downloaded.
pub struct Picture {
    /// As Instagram spells it, not as it was typed.
    pub username: String,
    pub url: String,
    pub bytes: Vec<u8>,
    /// Which of the two pictures arrived.
    pub source: Source,
}

impl Picture {
    /// Names the file after what actually arrived rather than after the URL.
    pub fn extension(&self) -> &'static str {
        media::extension_of(&self.bytes)
    }
}

/// Reports the size rather than the contents: a derived one would spell out a
/// megabyte of pixels into whatever printed it.
impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture")
            .field("username", &self.username)
            .field("url", &self.url)
            .field("bytes", &self.bytes.len())
            .field("source", &self.source)
            .finish()
    }
}

/// Which of the two pictures this is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The profile query carried it, with no size, or the by-id endpoint
    /// answered. The size is what it declared, not what was measured:
    /// nothing here decodes the image.
    FullSize { size: Option<(u32, u32)> },
    /// What the profile page offers, and why the full size is not there.
    Smaller { why: Option<String> },
}

/// The picture of the account named `typed`, at the best size the web
/// serves: the profile, then the picture.
pub async fn picture_named(client: &IgClient, typed: &str, known: Option<Pk>) -> Result<Picture> {
    let profile = client.profile_named(target::clean(typed), known).await?;
    if profile.anonymous_picture == Some(true) {
        return Err(no_picture(&profile.username));
    }
    let full_size = full_size_of(&profile);
    picture(
        client,
        profile.id,
        &profile.username,
        profile.profile_pic_url_hd.or(profile.profile_pic_url),
        full_size,
    )
    .await
}

/// The full-size picture's address, when the profile read carries it: the web
/// client's profile query answers it (`hd_profile_pic_url_info`), with no size.
pub fn full_size_of(profile: &WebProfileInfo) -> Option<String> {
    if profile.via == Via::Graph {
        profile.profile_pic_url_hd.clone()
    } else {
        None
    }
}

/// What is said for an account with no picture of its own: the default
/// avatar is not worth downloading.
pub fn no_picture(username: &str) -> anyhow::Error {
    anyhow!("@{} has no profile picture", printable(username))
}

/// The half after the name is resolved: the choice of address and the
/// picture itself.
pub async fn picture(
    client: &IgClient,
    pk: Pk,
    username: &str,
    fallback: Option<String>,
    full_size: Option<String>,
) -> Result<Picture> {
    if let Some(url) = full_size {
        let bytes = client.download(&url).await?;
        return Ok(Picture {
            username: username.to_string(),
            url,
            bytes,
            source: Source::FullSize { size: None },
        });
    }
    // Otherwise, without a browser, the full-size picture comes from the
    // by-id endpoint, so this spends a request on it.
    let (full_size, why_not) = if client.has_page() {
        (
            None,
            Some("the profile named no full-size picture".to_string()),
        )
    } else {
        match client.user_info(pk).await {
            Ok(info) => (info.and_then(|i| i.hd_profile_pic_url_info), None),
            Err(e) => (None, Some(e.to_string())),
        }
    };

    // Accounts that never set a picture have none of these, and the default
    // avatar is not worth downloading.
    let (url, source) = match full_size {
        Some(p) => (
            p.url,
            Source::FullSize {
                size: p.width.zip(p.height),
            },
        ),
        None => match fallback {
            Some(url) => (url, Source::Smaller { why: why_not }),
            None => return Err(no_picture(username)),
        },
    };

    // The CDN is not Instagram's API and does not count against its budget.
    let bytes = client.download(&url).await?;
    Ok(Picture {
        username: username.to_string(),
        url,
        bytes,
        source,
    })
}
