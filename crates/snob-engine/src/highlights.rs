//! The reels an account kept on its profile (`snob highlights`, the row of
//! them on a profile's page): the tray and what one of its entries holds,
//! one read for every front.
//!
//! **Reading a highlight does not mark it as seen**
//! (`crates/snob-core/tests/no_seen.rs`). **A private account the viewer does
//! not follow is told apart from an account with nothing kept**: its tray is
//! never asked for ([`Fetched::Hidden`]).

use anyhow::Result;
use snob_core::model::printable;
use snob_core::{Epoch, Pk};
use snob_ig::client::IgClient;
use snob_ig::model::WebProfileInfo;

use crate::engine::target;
use crate::media::{Story, story_from};

/// One highlight as the tray lists it: the cover the profile shows, not yet
/// its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// `highlight:<id>`, the spelling the items are fetched with.
    pub id: String,
    /// Filtered: it came off somebody else's profile. Empty when the
    /// highlight has none, which happens.
    pub title: String,
    /// The count the tray declares; the browser's tray declares none.
    pub declared_items: Option<u64>,
    pub created_at: Option<Epoch>,
    /// When something was last added to it.
    pub updated_at: Option<Epoch>,
    /// Its cover's address, for a view to draw ([`cover_of`]). Not in the
    /// JSON contract.
    pub cover: Option<String>,
}

/// An account's tray.
#[derive(Debug, Clone)]
pub struct Tray {
    /// As Instagram spells it, not as it was typed.
    pub username: String,
    pub entries: Vec<Highlight>,
}

/// What the tray's read could see.
pub enum Fetched {
    Tray(Tray),
    /// Private, and the viewer does not follow it.
    Hidden {
        username: String,
    },
}

/// The tray of the account `typed` names: its profile, then its tray.
pub async fn tray(
    client: &IgClient,
    typed: &str,
    viewer: Pk,
    known: Option<Pk>,
) -> Result<Fetched> {
    let info = client.profile_named(target::clean(typed), known).await?;
    // A private account serves its reels only to its followers, and asking
    // would spend a request on an empty answer and then say "none" for an
    // account that has some.
    if !target::shown_to(&info, viewer) {
        return Ok(Fetched::Hidden {
            username: info.username,
        });
    }
    Ok(Fetched::Tray(Tray {
        entries: entries(client, &info).await?,
        username: info.username,
    }))
}

/// The tray of the account `info` is, which shows it to the viewer
/// (`target::shown_to`, the caller's to check).
///
/// **A profile that says it has none is not asked**: zero is an answer it
/// already gave, and the tray would spend a request on an empty list.
pub(crate) async fn entries(client: &IgClient, info: &WebProfileInfo) -> Result<Vec<Highlight>> {
    if info.highlight_reel_count == Some(0) {
        return Ok(Vec::new());
    }
    let tray = client.highlights_tray(info.id, &info.username).await?;
    Ok(tray
        .into_iter()
        .map(|h| Highlight {
            cover: cover_of(&h),
            id: h.id,
            title: printable(h.title.as_deref().unwrap_or("")),
            declared_items: h.media_count,
            created_at: h.created_at,
            updated_at: h.updated_timestamp,
        })
        .collect())
}

/// The items of `entry`, one of `tray`'s; none when it no longer exists.
pub async fn items(client: &IgClient, tray: &Tray, entry: &Highlight) -> Result<Vec<Story>> {
    let ids: Vec<String> = tray.entries.iter().map(|e| e.id.clone()).collect();
    let Some(reel) = client.highlight(&entry.id, &ids, &tray.username).await? else {
        return Ok(Vec::new());
    };
    Ok(reel.items.iter().map(story_from).collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A highlight's cover is the tray's cropped picture, and nothing when
    /// the tray gives none or an empty address.
    #[test]
    fn a_highlights_cover_is_its_cropped_picture() {
        let tray: snob_ig::model::HighlightsTray = serde_json::from_value(serde_json::json!({
            "tray": [
                {"id": "highlight:1", "cover_media": {"cropped_image_version":
                    {"url": "https://x.cdninstagram.com/cover.jpg", "width": 150, "height": 150}}},
                {"id": "highlight:2", "cover_media": null},
                {"id": "highlight:3", "cover_media": {"cropped_image_version": {"url": ""}}},
            ]
        }))
        .unwrap();
        let covers: Vec<Option<String>> = tray.tray.iter().map(cover_of).collect();
        assert_eq!(
            covers,
            [
                Some("https://x.cdninstagram.com/cover.jpg".to_string()),
                None,
                None
            ]
        );
    }
}
