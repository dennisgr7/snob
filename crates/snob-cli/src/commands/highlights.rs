//! `snob highlights`: the reels an account chose to keep on its profile, and how
//! to keep a copy.
//!
//! AGENTS.md says why the two are not one command with a flag.
//!
//! **Reading a highlight does not mark it as seen.**
//!
//! **A private account the viewer does not follow is told apart from an account
//! with nothing kept.**

use std::path::Path;

use anyhow::{Result, anyhow};
use comfy_table::Cell;
use snob_core::Epoch;
use snob_core::model::printable;
use snob_engine::app::{App, Viewer};
use snob_engine::exit::{ExitCode, ExitError};
use snob_engine::media::{self, Story, download_many, was_canceled};
use snob_ig::client::IgClient;
use snob_ig::pace::Pace;
use snob_store::paths::AccountPaths;
use snob_store::secrets::SecretStore;

use crate::cli::{DownloadSelection, Format, HighlightsArgs, StoryFormat};
use crate::commands::common::{self, Switched};
use crate::commands::stories::story_from;
use crate::output::Presentation;
use crate::output::media::empty_document;
use crate::ui;

/// One tray entry: the cover the profile shows, not yet its items.
#[derive(Debug, Clone)]
pub struct Entry {
    /// `highlight:<id>`, the spelling the items are fetched with.
    pub id: String,
    /// Filtered: it came off somebody else's profile and it is going to a
    /// terminal. Empty when the highlight has none, which happens.
    pub title: String,
    /// The count the tray declares.
    pub declared_items: Option<u64>,
    pub created_at: Option<Epoch>,
    /// When something was last added to it.
    pub updated_at: Option<Epoch>,
    /// Its cover's address, for a view to draw ([`cover_of`]).
    pub cover: Option<String>,
}

/// The address of `highlight`'s cover, the square picture the profile
/// shows above its title, as the tray hands it out
/// (`cover_media.cropped_image_version`, on the REST and GraphQL trays
/// alike).
pub(crate) fn cover_of(highlight: &snob_ig::model::Highlight) -> Option<String> {
    highlight
        .cover_media
        .as_ref()?
        .cropped_image_version
        .as_ref()
        .map(|v| v.url.clone())
        .filter(|url| !url.is_empty())
}

/// The tray, gathered before anything is printed.
#[derive(Debug, Clone)]
pub struct Tray {
    /// As Instagram spells it, not as it was typed.
    pub username: String,
    pub entries: Vec<Entry>,
}

/// What the tray request could see.
pub enum Fetched {
    Tray(Tray),
    /// Private, and the viewer does not follow it.
    Hidden {
        username: String,
    },
}

pub async fn run(
    args: HighlightsArgs,
    store: SecretStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    args.action.refuse_stdout_download_early()?;
    if args.highlight.is_none()
        && args.action.selection().is_some()
        && args
            .action
            .output
            .as_deref()
            .is_some_and(snob_engine::files::is_stdout)
    {
        return Err(whole_highlights_to_stdout());
    }
    let app = common::reader(&store, paths, args.action.interactive, false)?;
    let typed = common::target_or_own(&app, args.target.as_deref()).await?;

    let known = snob_engine::engine::target::known_pk_of(&app, args.target.as_deref(), &typed)?;
    let tray = match fetch_tray(app.client(), &typed, app.viewer().pk, known).await? {
        Fetched::Tray(tray) => tray,
        Fetched::Hidden { username } => {
            // An answer, not a failure: the account was found and this is
            // what it shows the viewer. The wording is `profile`'s.
            ui::info(&format!(
                "the highlights of @{} are not visible: the account is private and you do not \
                 follow it",
                printable(&username)
            ));
            // A document for a script reading JSON, as the empty tray below
            // writes one, with `null` where `profile` puts it for the same
            // answer: not "has none", but "not shown".
            if args.action.selection().is_none() && !args.action.interactive {
                let destination = args.action.output.as_deref();
                let format =
                    common::checked_format(args.list.format, destination, "a highlight listing")?;
                empty_document(format, destination, || {
                    Ok(serde_json::to_string_pretty(&serde_json::json!({
                        "username": username,
                        "highlights": null,
                    }))?)
                })?;
            }
            return Ok(ExitCode::Ok);
        }
    };

    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }

    if tray.entries.is_empty() {
        ui::info(&format!(
            "@{} has no highlights.",
            printable(&tray.username)
        ));
        if args.action.selection().is_none() && !args.action.interactive {
            let destination = args.action.output.as_deref();
            let format =
                common::checked_format(args.list.format, destination, "a highlight listing")?;
            empty_document(format, destination, || tray_json(&tray, format))?;
        }
        return Ok(ExitCode::Ok);
    }

    // Decided once for both levels: the browser is the default for a person at
    // a terminal, and every explicit flag beats detection.
    let browses = args.action.browses(
        args.list.format.is_some(),
        ui::a_human_would_watch_the_listing_scroll_by(),
    );

    // Checked against the tray before any further request, so `snob
    // highlights someone 9` against a tray of five costs the two requests
    // already spent and no more.
    if let Some(number) = args.highlight
        && number as usize > tray.entries.len()
    {
        return Err(no_such_highlight(&tray, number as usize));
    }

    if browses {
        let start = args.highlight.map(|number| number as usize - 1);
        return browse(app, tray, start, &store, paths).await;
    }
    match args.highlight {
        None => at_the_tray(&app, &tray, &args).await,
        Some(number) => inside_one(&app, &tray, number as usize, &args).await,
    }
}

/// The tray, as the account the command runs as and then as each account the
/// view switches to, fetched again as that one: a private account keeps its
/// highlights from those who do not follow it.
async fn browse(
    app: Box<App>,
    tray: Tray,
    start: Option<usize>,
    store: &SecretStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    struct Session {
        app: Box<App>,
        tray: Tray,
        start: Option<usize>,
    }
    common::switching(
        Session { app, tray, start },
        async |s: &mut Session, note: String| {
            let (client, viewer) = (s.app.client(), s.app.viewer());
            let start = s.start.take();
            crate::ui::highlights::browse(client, &s.tray, start, viewer, paths, store, note).await
        },
        async |s: &Session, to: Viewer| {
            let name = s.tray.username.clone();
            let from = s.app.viewer();
            let switched = common::switch(store, paths, from, &to, false, async |app: &mut App| {
                let known = snob_engine::engine::target::known_pk(app, &name)?;
                match fetch_tray(app.client(), &name, app.viewer().pk, known).await? {
                    Fetched::Tray(tray) if !tray.entries.is_empty() => Ok(tray),
                    Fetched::Tray(tray) => {
                        Err(anyhow!("@{} has no highlights", printable(&tray.username)))
                    }
                    Fetched::Hidden { username } => Err(anyhow!(
                        "@{} is private and {} does not follow it",
                        printable(&username),
                        to.label()
                    )),
                }
            })
            .await?;
            Ok(match switched {
                Switched::To((app, tray)) => Switched::To(Session {
                    app,
                    tray,
                    start: None,
                }),
                Switched::Refused(note) => Switched::Refused(note),
            })
        },
        |_: &Session, _: bool| {},
    )
    .await
}

/// The commands that act on the tray listing: download whole entries by
/// their numbers, or print it.
async fn at_the_tray(app: &App, tray: &Tray, args: &HighlightsArgs) -> Result<ExitCode> {
    if let Some(selection) = args.action.selection() {
        return download_entries(app, tray, selection, args.action.output.as_deref()).await;
    }

    // A highlight's picture is its cover, the one the profile shows.
    let pictures = match args.list.small_pictures {
        true => {
            let covers = tray
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (index + 1, entry.cover.clone()))
                .collect();
            Some(media::small_pictures(app.client(), covers).await?)
        }
        false => None,
    };
    list_tray(
        tray,
        pictures,
        args.list.format,
        args.action.output.as_deref(),
    )
}

/// The commands that act inside one entry, named by its tray number, which
/// `run` has checked against the tray.
async fn inside_one(
    app: &App,
    tray: &Tray,
    number: usize,
    args: &HighlightsArgs,
) -> Result<ExitCode> {
    let items = items_of_entry(app.client(), tray, &tray.entries[number - 1]).await?;
    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }
    if items.is_empty() {
        ui::info(&empty_highlight(tray, number));
        if args.action.selection().is_none() {
            let destination = args.action.output.as_deref();
            let format =
                common::checked_format(args.list.format, destination, "a highlight's listing")?;
            empty_document(format, destination, || {
                items_json(tray, number, &items, format)
            })?;
        }
        return Ok(ExitCode::Ok);
    }

    if let Some(selection) = args.action.selection() {
        return crate::output::media::download_selected(
            app.client_shared(),
            stem_of(tray, number),
            &items,
            selection,
            args.action.output.as_deref(),
            |n| no_such_item(tray, number, &items, n),
        )
        .await;
    }

    let pictures = match args.list.small_pictures {
        true => {
            let thumbs = crate::commands::stories::thumbs(&items);
            Some(media::small_pictures(app.client(), thumbs).await?)
        }
        false => None,
    };
    list_items(
        tray,
        number,
        &items,
        pictures,
        args.list.format,
        args.action.output.as_deref(),
    )
}

/// The network half of the tray, kept apart from the session and the filesystem
/// so a test can drive it against a mock server.
pub async fn fetch_tray(
    client: &IgClient,
    typed: &str,
    viewer: snob_core::Pk,
    known: Option<snob_core::Pk>,
) -> Result<Fetched> {
    let info = client
        .profile_named(snob_engine::engine::target::clean(typed), known)
        .await?;

    // The same three-way rule as `profile`: a private account serves its
    // reels only to its followers, and asking would spend a request on an
    // empty answer and then say "none" for an account that has some.
    let own = info.id == viewer;
    let is_private = info.is_private.unwrap_or(false);
    let you_follow = info.followed_by_viewer.unwrap_or(false);
    if !(own || !is_private || you_follow) {
        return Ok(Fetched::Hidden {
            username: info.username,
        });
    }

    // Zero is an answer the profile already gave; see `profile`.
    let tray = if info.highlight_reel_count == Some(0) {
        Vec::new()
    } else {
        client.highlights_tray(info.id, &info.username).await?
    };
    Ok(Fetched::Tray(Tray {
        username: info.username,
        entries: tray
            .into_iter()
            .map(|h| Entry {
                cover: cover_of(&h),
                id: h.id,
                title: printable(h.title.as_deref().unwrap_or("")),
                declared_items: h.media_count,
                created_at: h.created_at,
                updated_at: h.updated_timestamp,
            })
            .collect(),
    }))
}

/// The items of one entry of `tray`.
pub async fn items_of_entry(client: &IgClient, tray: &Tray, entry: &Entry) -> Result<Vec<Story>> {
    let ids: Vec<String> = tray.entries.iter().map(|e| e.id.clone()).collect();
    let Some(reel) = client.highlight(&entry.id, &ids, &tray.username).await? else {
        return Ok(Vec::new());
    };
    Ok(reel.items.iter().map(story_from).collect())
}

/// The sentence for a tray number nobody has, shared by every path that
/// takes one so they cannot drift.
fn no_such_highlight(tray: &Tray, number: usize) -> anyhow::Error {
    anyhow!(
        "there is no highlight {number}: @{} has {}",
        printable(&tray.username),
        match tray.entries.len() {
            1 => "one".to_string(),
            n => format!("{n} highlights"),
        }
    )
}

/// And the one for an entry that answered with nothing.
fn empty_highlight(tray: &Tray, number: usize) -> String {
    format!(
        "highlight {number} of @{} {}is empty.",
        printable(&tray.username),
        titled(&tray.entries[number - 1])
    )
}

/// `("Trip") `, or nothing for an untitled entry — a parenthetical that can
/// sit in the middle of a sentence either way.
fn titled(entry: &Entry) -> String {
    if entry.title.is_empty() {
        String::new()
    } else {
        format!("(\"{}\") ", entry.title)
    }
}

/// `someone-2`, the stem every file of entry 2 is named under, so that
/// `-d 3` inside it and D in the browser write the very same name.
pub(crate) fn stem_of(tray: &Tray, number: usize) -> String {
    format!("{}-{number}", printable(&tray.username))
}

/// Why `-o -` takes no whole highlight: it is as many files as it holds.
fn whole_highlights_to_stdout() -> anyhow::Error {
    anyhow::anyhow!(
        "-o - writes one file to standard output, and a whole highlight may hold \
         several; name one of its items, as in \"snob highlights someone 2 -d 1 -o -\""
    )
}

/// Downloads whole entries: each one fetched and then saved the way `stories -d
/// all` saves a reel, into one directory.
async fn download_entries(
    app: &snob_engine::app::App,
    tray: &Tray,
    selection: DownloadSelection,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    let numbers = crate::output::media::numbers_of(selection, tray.entries.len(), |number| {
        no_such_highlight(tray, number)
    })?;
    // A whole highlight is as many files as it holds, which is not known until
    // it is read: refused before that read rather than after it.
    if destination.is_some_and(snob_engine::files::is_stdout) {
        return Err(whole_highlights_to_stdout());
    }

    let mut failed: Vec<String> = Vec::new();
    let mut not_tried: &[usize] = &[];
    let mut stopped_by = None;
    for (at, &number) in numbers.iter().enumerate() {
        if app.cancel().is_canceled() {
            return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
        }
        // A step between entries, as between the pages of a list: each entry
        // is one request, and a download of every highlight is one action.
        if at > 0 && app.client().step_between(&Pace::default()).await {
            return Err(ExitError::new(ExitCode::Interrupted, "stopped").into());
        }
        let entry = &tray.entries[number - 1];
        let items = match items_of_entry(app.client(), tray, entry).await {
            Ok(items) => items,
            // The user, not the server: stopped as the downloads below stop.
            Err(e) if was_canceled(&e) => return Err(e),
            Err(e) => {
                failed.push(format!("highlight {number}: {e}"));
                // **Past a failure, not past a refusal.**
                if is_a_refusal(&e) {
                    stopped_by = Some(snob_engine::exit::exit_code_for(&e));
                    not_tried = &numbers[at + 1..];
                    break;
                }
                continue;
            }
        };
        if items.is_empty() {
            ui::info(&empty_highlight(tray, number));
            continue;
        }
        ui::info(&format!(
            "Highlight {number} {}- {}",
            titled(entry),
            match items.len() {
                1 => "1 item".to_string(),
                n => format!("{n} items"),
            }
        ));
        let all: Vec<usize> = (1..=items.len()).collect();
        if let Err(e) = download_many(
            app.client_shared(),
            stem_of(tray, number),
            items,
            all,
            destination,
        )
        .await
        {
            if was_canceled(&e) {
                return Err(e);
            }
            failed.push(format!("highlight {number}: {e}"));
        }
    }

    if failed.is_empty() {
        return Ok(ExitCode::Ok);
    }
    Err(ExitError::new(
        stopped_by.unwrap_or(ExitCode::Error),
        not_saved(&failed, not_tried, numbers.len()),
    )
    .into())
}

/// Whether an entry's failure answers every entry after it: anything but a
/// failure worth retrying.
fn is_a_refusal(e: &anyhow::Error) -> bool {
    e.chain()
        .find_map(|cause| cause.downcast_ref::<snob_ig::error::IgError>())
        .is_some_and(|ig| ig.reaction() != snob_ig::error::Reaction::Retry)
}

/// The closing sentence of a download that did not save everything: the
/// entries that failed, and those never tried after a refusal, both counted.
fn not_saved(failed: &[String], not_tried: &[usize], asked: usize) -> String {
    let mut lines = failed.to_vec();
    if !not_tried.is_empty() {
        let rest: Vec<String> = not_tried.iter().map(ToString::to_string).collect();
        lines.push(format!("not tried after that: {}", rest.join(", ")));
    }
    format!(
        "{} of {asked} highlights could not be fully saved:\n{}",
        failed.len() + not_tried.len(),
        lines.join("\n")
    )
}

/// The sentence for an item number the entry does not hold.
fn no_such_item(tray: &Tray, number: usize, items: &[Story], asked: usize) -> anyhow::Error {
    anyhow!(
        "there is no item {asked}: highlight {number} of @{} holds {}",
        printable(&tray.username),
        match items.len() {
            1 => "one".to_string(),
            n => format!("{n} items"),
        }
    )
}

/// Prints the tray.
fn list_tray(
    tray: &Tray,
    pictures: Option<Vec<serde_json::Value>>,
    format: Option<StoryFormat>,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    let format = common::checked_format(format, destination, "a highlight listing")?;
    let text = match format {
        Format::Json | Format::Ndjson => media::with_pictures(tray_json(tray, format)?, pictures)?,
        _ => tray_table(tray, Presentation::detect(destination)),
    };
    let hint = format!(
        "snob highlights {} <number> to look inside one, -d <number> to save one whole, or -i \
         to browse",
        printable(&tray.username)
    );
    crate::output::media::print_listing(text, destination, &hint)
}

/// Prints what one entry holds. The numbers here are what `-d` takes.
fn list_items(
    tray: &Tray,
    number: usize,
    items: &[Story],
    pictures: Option<Vec<serde_json::Value>>,
    format: Option<StoryFormat>,
    destination: Option<&Path>,
) -> Result<ExitCode> {
    let format = common::checked_format(format, destination, "a highlight's listing")?;
    let text = match format {
        Format::Json | Format::Ndjson => {
            media::with_pictures(items_json(tray, number, items, format)?, pictures)?
        }
        _ => items_table(items, Presentation::detect(destination)),
    };
    crate::output::media::print_listing(
        text,
        destination,
        "--download <number> to save one, or --interactive to move through them",
    )
}

fn tray_table(tray: &Tray, presentation: Presentation) -> String {
    let mut table =
        crate::output::media::listing_table(&["#", "Title", "Items", "Updated"], presentation);

    for (index, entry) in tray.entries.iter().enumerate() {
        table.add_row([
            Cell::new(index + 1),
            // Already filtered in `fetch_tray`; the dash keeps an untitled
            // entry's row from reading as a rendering defect.
            Cell::new(if entry.title.is_empty() {
                "-"
            } else {
                &entry.title
            }),
            Cell::new(
                entry
                    .declared_items
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "-".into()),
            ),
            Cell::new(
                entry
                    .updated_at
                    .map(snob_engine::sentences::dated)
                    .unwrap_or_else(|| "-".into()),
            ),
        ]);
    }
    table.to_string()
}

fn items_table(items: &[Story], presentation: Presentation) -> String {
    // No "Gone in": nothing in a highlight is going anywhere, which is what a
    // highlight is.
    let mut table =
        crate::output::media::listing_table(&["#", "Kind", "Posted", "Mentions"], presentation);

    for (index, story) in items.iter().enumerate() {
        table.add_row([
            Cell::new(index + 1),
            Cell::new(story.kind.label()),
            Cell::new(snob_engine::sentences::dated(story.taken_at)),
            Cell::new(media::mentions_of(story)),
        ]);
    }
    table.to_string()
}

fn tray_json(tray: &Tray, format: Format) -> Result<String> {
    let rows: Vec<serde_json::Value> = tray
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            serde_json::json!({
                "number": index + 1,
                // The tray's own spelling, prefix included, the way `profile`
                // already publishes it -- one spelling of the one id.
                "id": entry.id,
                "title": entry.title,
                "items": entry.declared_items,
                "created_at": entry.created_at,
                "updated_at": entry.updated_at,
            })
        })
        .collect();

    crate::output::media::rows_or_envelope(rows, format, |rows| {
        serde_json::json!({
            "username": tray.username,
            "highlights": rows,
        })
    })
}

fn items_json(tray: &Tray, number: usize, items: &[Story], format: Format) -> Result<String> {
    let entry = &tray.entries[number - 1];
    let rows: Vec<serde_json::Value> = items
        .iter()
        .enumerate()
        .map(|(index, story)| {
            serde_json::json!({
                "number": index + 1,
                "kind": story.kind.label(),
                "taken_at": story.taken_at,
                "mentions": story.mentions,
                // As `stories` includes it: the signed address is what lets
                // another program fetch the file, and it expires on its own.
                "url": story.url,
            })
        })
        .collect();

    crate::output::media::rows_or_envelope(rows, format, |rows| {
        serde_json::json!({
            "username": tray.username,
            "highlight": {
                "number": number,
                "id": entry.id,
                "title": entry.title,
            },
            "items": rows,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_ig::error::IgError;

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

    /// Every entry not saved is counted, the ones never tried included.
    #[test]
    fn what_was_not_saved_counts_the_entries_not_tried() {
        let said = not_saved(&["highlight 2: refused".to_string()], &[3, 4, 5], 5);
        assert!(said.starts_with("4 of 5 highlights"), "{said}");
        assert!(said.ends_with("not tried after that: 3, 4, 5"), "{said}");
    }

    /// A dead session answers every entry after it; a network failure does
    /// not. Ctrl+C never reaches this: the loop hands it back first, so it
    /// exits 130 as Ctrl+C does everywhere else.
    #[test]
    fn a_refusal_is_anything_not_worth_retrying() {
        assert!(is_a_refusal(&IgError::SessionExpired.into()));
        assert!(is_a_refusal(&IgError::RateLimited.into()));
        assert!(!is_a_refusal(&anyhow::anyhow!("a file would not write")));
        let canceled: anyhow::Error = IgError::Canceled.into();
        assert!(was_canceled(&canceled));
        assert_eq!(
            snob_engine::exit::exit_code_for(&canceled),
            ExitCode::Interrupted
        );
    }

    fn plain() -> Presentation {
        Presentation {
            interactive: false,
            hyperlinks: false,
            color: false,
            width: Some(120),
        }
    }

    /// A titled entry with its count and date and an untitled one without,
    /// both dated midday UTC on the fifteenth so the date has one width in
    /// every zone.
    fn tray() -> Tray {
        Tray {
            username: "someone".into(),
            entries: vec![
                Entry {
                    id: "highlight:1".into(),
                    title: "Trip".into(),
                    declared_items: Some(2),
                    created_at: Some(Epoch::new(1_700_000_000)),
                    updated_at: Some(Epoch::new(1_773_576_000)),
                    cover: None,
                },
                Entry {
                    id: "highlight:2".into(),
                    title: String::new(),
                    declared_items: None,
                    created_at: None,
                    updated_at: None,
                    cover: None,
                },
            ],
        }
    }

    fn items() -> Vec<Story> {
        let story = |kind, mentions: &[&str]| Story {
            kind,
            taken_at: Epoch::new(1_773_576_000),
            expiring_at: None,
            url: Some(format!("https://cdn.example/{}", mentions.len())),
            mentions: mentions.iter().map(ToString::to_string).collect(),
            thumb: None,
        };
        vec![
            story(snob_engine::media::Kind::Photo, &["a", "b"]),
            story(snob_engine::media::Kind::Video, &[]),
        ]
    }

    #[test]
    fn the_tray_table_marks_what_it_does_not_know_with_a_dash() {
        let updated = snob_engine::sentences::dated(Epoch::new(1_773_576_000));
        assert_eq!(
            tray_table(&tray(), plain()),
            [
                "┌───┬───────┬───────┬──────────────┐".to_string(),
                "│ # ┆ Title ┆ Items ┆ Updated      │".to_string(),
                "╞═══╪═══════╪═══════╪══════════════╡".to_string(),
                format!("│ 1 ┆ Trip  ┆ 2     ┆ {updated} │"),
                "│ 2 ┆ -     ┆ -     ┆ -            │".to_string(),
                "└───┴───────┴───────┴──────────────┘".to_string(),
            ]
            .join("\n")
        );
    }

    #[test]
    fn the_items_table_dates_with_the_year_and_has_no_countdown() {
        let posted = snob_engine::sentences::dated(Epoch::new(1_773_576_000));
        assert_eq!(
            items_table(&items(), plain()),
            [
                "┌───┬───────┬──────────────┬──────────┐".to_string(),
                "│ # ┆ Kind  ┆ Posted       ┆ Mentions │".to_string(),
                "╞═══╪═══════╪══════════════╪══════════╡".to_string(),
                format!("│ 1 ┆ photo ┆ {posted} ┆ @a @b    │"),
                format!("│ 2 ┆ video ┆ {posted} ┆          │"),
                "└───┴───────┴──────────────┴──────────┘".to_string(),
            ]
            .join("\n")
        );
    }

    /// NDJSON is the rows alone, one to a line; JSON wraps them with the
    /// account, and an entry's items with the entry they are.
    #[test]
    fn the_json_is_the_rows_or_the_rows_inside_what_they_belong_to() {
        let tray = tray();
        let entries = [
            serde_json::json!({"number": 1, "id": "highlight:1", "title": "Trip", "items": 2,
                "created_at": 1_700_000_000, "updated_at": 1_773_576_000}),
            serde_json::json!({"number": 2, "id": "highlight:2", "title": "", "items": null,
                "created_at": null, "updated_at": null}),
        ];
        let lines = |text: String| -> Vec<serde_json::Value> {
            text.lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        };
        assert_eq!(lines(tray_json(&tray, Format::Ndjson).unwrap()), entries);
        let document: serde_json::Value =
            serde_json::from_str(&tray_json(&tray, Format::Json).unwrap()).unwrap();
        assert_eq!(
            document,
            serde_json::json!({"username": "someone", "highlights": entries})
        );

        let rows = [
            serde_json::json!({"number": 1, "kind": "photo", "taken_at": 1_773_576_000,
                "mentions": ["a", "b"], "url": "https://cdn.example/2"}),
            serde_json::json!({"number": 2, "kind": "video", "taken_at": 1_773_576_000,
                "mentions": [], "url": "https://cdn.example/0"}),
        ];
        let items = items();
        assert_eq!(
            lines(items_json(&tray, 1, &items, Format::Ndjson).unwrap()),
            rows
        );
        let document: serde_json::Value =
            serde_json::from_str(&items_json(&tray, 1, &items, Format::Json).unwrap()).unwrap();
        assert_eq!(
            document,
            serde_json::json!({
                "username": "someone",
                "highlight": {"number": 1, "id": "highlight:1", "title": "Trip"},
                "items": rows,
            })
        );
    }
}
