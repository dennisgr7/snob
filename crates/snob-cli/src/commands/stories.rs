//! `snob stories`: what an account has up right now, and how to keep a copy.
//!
//! **Downloading a story does not mark it as seen.**
//!
//! **It asks no consent** (AGENTS.md, "Consent"): the question covers walking
//! somebody's followers and following, and a story is what its account
//! shows whoever opens the profile.

use std::path::Path;

use anyhow::{Result, anyhow};
use comfy_table::Cell;
use snob_core::model::printable;
use snob_engine::app::{App, Viewer};
use snob_engine::exit::ExitCode;
use snob_engine::media::{self, Stories, Story, left_of, posted_of};
use snob_ig::client::IgClient;
use snob_store::paths::AccountPaths;
use snob_store::secrets::SecretStore;

use crate::cli::{Format, StoriesArgs, StoryFormat};
use crate::commands::common;
use crate::output::Presentation;
use crate::output::media::empty_document;
use crate::ui;

pub async fn run(args: StoriesArgs, store: SecretStore, paths: &AccountPaths) -> Result<ExitCode> {
    args.action.refuse_stdout_download_early()?;
    let app = common::reader(&store, paths, args.action.interactive, false)?;
    let typed = common::target_or_own(&app, args.target.as_deref()).await?;

    let known = snob_engine::engine::target::known_pk_of(&app, args.target.as_deref(), &typed)?;
    let stories = fetch(app.client(), &typed, known).await?;

    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }

    if stories.items.is_empty() {
        ui::info(&format!(
            "@{} has no stories up right now.",
            printable(&stories.username)
        ));
        if args.action.selection().is_none() && !args.action.interactive {
            let destination = args.action.output.as_deref();
            let format = common::checked_format(args.list.format, destination, "a story listing")?;
            empty_document(format, destination, || as_json(&stories, format))?;
        }
        return Ok(ExitCode::Ok);
    }

    // The browser is the default for a person at a terminal; every explicit
    // flag beats detection. `MediaActionArgs::browses` is the whole rule.
    if args.action.browses(
        args.list.format.is_some(),
        ui::a_human_would_watch_the_listing_scroll_by(),
    ) {
        return browse(app, stories, &store, paths).await;
    }

    if let Some(selection) = args.action.selection() {
        return crate::output::media::download_selected(
            app.client_shared(),
            printable(&stories.username),
            &stories.items,
            selection,
            args.action.output.as_deref(),
            |number| no_such_story(&stories, number),
        )
        .await;
    }

    let pictures = match args.list.small_pictures {
        true => Some(media::small_pictures(app.client(), thumbs(&stories.items)).await?),
        false => None,
    };
    list(
        &stories,
        pictures,
        args.list.format,
        args.action.output.as_deref(),
    )
}

/// Each story by its number and the address of its small picture, for
/// `media::small_pictures`.
pub(crate) fn thumbs(items: &[Story]) -> Vec<(usize, Option<String>)> {
    items
        .iter()
        .enumerate()
        .map(|(index, story)| (index + 1, story.thumb.clone()))
        .collect()
}

/// The list, as the account the command runs as and then as each account the
/// list switches to, fetched again as that one: an account's stories can be
/// hidden from one viewer and not another.
async fn browse(
    app: Box<App>,
    stories: Stories,
    store: &SecretStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    common::switching(
        (app, stories),
        async |(app, stories): &mut (Box<App>, Stories), note: String| {
            crate::ui::stories::browse(app.client(), stories, app.viewer(), paths, store, note)
                .await
        },
        async |(app, stories): &(Box<App>, Stories), to: Viewer| {
            let name = stories.username.clone();
            common::switch(
                store,
                paths,
                app.viewer(),
                &to,
                false,
                async |app: &mut App| {
                    let known = snob_engine::engine::target::known_pk(app, &name)?;
                    let stories = fetch(app.client(), &name, known).await?;
                    if stories.items.is_empty() {
                        anyhow::bail!("@{} has no stories up for it", printable(&stories.username));
                    }
                    Ok(stories)
                },
            )
            .await
        },
        |_: &(Box<App>, Stories), _: bool| {},
    )
    .await
}

/// The sentence for a number the tray does not have.
fn no_such_story(stories: &Stories, number: usize) -> anyhow::Error {
    anyhow!(
        "there is no story {number}: @{} has {}",
        printable(&stories.username),
        match stories.items.len() {
            1 => "one".to_string(),
            n => format!("{n} stories"),
        }
    )
}

/// The network half: the profile and the reel, each paid for inside the client,
/// kept apart from the session and the filesystem so a test can drive it.
pub async fn fetch(
    client: &IgClient,
    typed: &str,
    known: Option<snob_core::Pk>,
) -> Result<Stories> {
    let profile = client
        .profile_named(snob_engine::engine::target::clean(typed), known)
        .await?;

    let Some(reel) = client.stories(profile.id, &profile.username).await? else {
        return Ok(Stories {
            username: profile.username,
            items: Vec::new(),
        });
    };

    Ok(Stories {
        username: profile.username,
        items: reel.items.iter().map(media::story_from).collect(),
    })
}

/// Prints the listing. The numbers here are what `--download` takes.
fn list(
    stories: &Stories,
    pictures: Option<Vec<serde_json::Value>>,
    format: Option<StoryFormat>,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    let format = common::checked_format(format, destination, "a story listing")?;
    let text = match format {
        Format::Json | Format::Ndjson => media::with_pictures(as_json(stories, format)?, pictures)?,
        _ => as_table(stories, Presentation::detect(destination)),
    };
    crate::output::media::print_listing(
        text,
        destination,
        "--download <number> to download one, or --interactive to move through them",
    )
}

fn as_table(stories: &Stories, presentation: Presentation) -> String {
    let mut table = crate::output::media::listing_table(
        &["#", "Kind", "Posted", "Gone in", "Mentions"],
        presentation,
    );

    for (index, story) in stories.items.iter().enumerate() {
        table.add_row([
            Cell::new(index + 1),
            Cell::new(story.kind.label()),
            Cell::new(posted_of(story)),
            Cell::new(left_of(story)),
            Cell::new(media::mentions_of(story)),
        ]);
    }
    table.to_string()
}

fn as_json(stories: &Stories, format: Format) -> Result<String> {
    let rows: Vec<serde_json::Value> = stories
        .items
        .iter()
        .enumerate()
        .map(|(index, story)| {
            serde_json::json!({
                "number": index + 1,
                "kind": story.kind.label(),
                "taken_at": story.taken_at,
                "expiring_at": story.expiring_at,
                "mentions": story.mentions,
                // Deliberately included: a signed CDN address is what makes the
                // JSON usable by anything else, and it is already in the reply
                // Instagram gave this session. It expires on its own.
                "url": story.url,
            })
        })
        .collect();

    crate::output::media::rows_or_envelope(rows, format, |rows| {
        serde_json::json!({
            "username": stories.username,
            "stories": rows,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Epoch;
    use snob_engine::media::Kind;

    fn plain() -> Presentation {
        Presentation {
            interactive: false,
            hyperlinks: false,
            color: false,
            width: Some(120),
        }
    }

    /// A photo tagging two accounts and a video tagging nobody, posted at
    /// midday UTC on the fifteenth, so the drawn date has one width in every
    /// zone.
    fn two_stories() -> Stories {
        let story = |kind, mentions: &[&str]| Story {
            kind,
            taken_at: Epoch::new(1_773_576_000),
            expiring_at: None,
            url: Some(format!("https://cdn.example/{}", mentions.len())),
            mentions: mentions.iter().map(ToString::to_string).collect(),
            thumb: None,
        };
        Stories {
            username: "someone".into(),
            items: vec![story(Kind::Photo, &["a", "b"]), story(Kind::Video, &[])],
        }
    }

    #[test]
    fn the_table_draws_what_the_listing_numbers() {
        let posted = snob_engine::sentences::stored_on(Epoch::new(1_773_576_000));
        assert_eq!(
            as_table(&two_stories(), plain()),
            [
                "┌───┬───────┬─────────────────┬─────────┬──────────┐".to_string(),
                "│ # ┆ Kind  ┆ Posted          ┆ Gone in ┆ Mentions │".to_string(),
                "╞═══╪═══════╪═════════════════╪═════════╪══════════╡".to_string(),
                format!("│ 1 ┆ photo ┆ {posted} ┆ -       ┆ @a @b    │"),
                format!("│ 2 ┆ video ┆ {posted} ┆ -       ┆          │"),
                "└───┴───────┴─────────────────┴─────────┴──────────┘".to_string(),
            ]
            .join("\n")
        );
    }

    /// NDJSON is the rows alone, one to a line; JSON wraps them with the
    /// account they belong to.
    #[test]
    fn the_json_is_the_rows_or_the_rows_inside_their_account() {
        let stories = two_stories();
        let rows = [
            serde_json::json!({"number": 1, "kind": "photo", "taken_at": 1_773_576_000,
                "expiring_at": null, "mentions": ["a", "b"], "url": "https://cdn.example/2"}),
            serde_json::json!({"number": 2, "kind": "video", "taken_at": 1_773_576_000,
                "expiring_at": null, "mentions": [], "url": "https://cdn.example/0"}),
        ];
        let lines: Vec<serde_json::Value> = as_json(&stories, Format::Ndjson)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines, rows);
        let document: serde_json::Value =
            serde_json::from_str(&as_json(&stories, Format::Json).unwrap()).unwrap();
        assert_eq!(
            document,
            serde_json::json!({"username": "someone", "stories": rows})
        );
    }

    #[test]
    fn a_populated_story_listing_serializes_to_the_contract_shape() {
        let mut stories = two_stories();
        for story in &mut stories.items {
            story.expiring_at = Some(Epoch::new(1_773_662_400));
            story.thumb = Some("https://cdn.example/thumb.jpg".into());
        }
        insta::assert_snapshot!(as_json(&stories, Format::Json).unwrap());
    }
}
