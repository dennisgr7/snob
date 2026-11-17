//! What an account has up right now (`snob stories`, the stories of a
//! profile's page): one read for every front. What a story is, and how one
//! is kept, is `media`.
//!
//! **Reading a story does not mark it as seen**
//! (`crates/snob-core/tests/no_seen.rs`). **It asks no consent** (AGENTS.md,
//! "Consent"): a story is what its account shows whoever opens the profile.

use anyhow::Result;
use snob_core::Pk;
use snob_ig::client::IgClient;

use crate::engine::target;
use crate::media::{Stories, Story, story_from};

/// The stories of the account `typed` names: its profile, then its reel.
pub async fn named(client: &IgClient, typed: &str, known: Option<Pk>) -> Result<Stories> {
    let profile = client.profile_named(target::clean(typed), known).await?;
    Ok(Stories {
        items: of(client, profile.id, &profile.username).await?,
        username: profile.username,
    })
}

/// The stories `pk` has up, an account already resolved; none when its reel
/// answers nothing.
pub(crate) async fn of(client: &IgClient, pk: Pk, username: &str) -> Result<Vec<Story>> {
    let reel = client.stories(pk, username).await?;
    Ok(reel
        .map(|reel| reel.items.iter().map(story_from).collect())
        .unwrap_or_default())
}
