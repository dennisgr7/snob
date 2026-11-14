//! How a listing of stories, highlights or posts is written, and the
//! downloads it names by number.

use std::path::Path;

use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets};
use snob_ig::client::IgClient;

use crate::cli::{DownloadSelection, Format};
use crate::exit::ExitCode;
use crate::media::{Saved, Story, bytes_of, download_many, save_story};
use crate::output::{self, Presentation, Rendered};
use crate::ui;

/// Writes a listing and, at a terminal, the hint about what to do with it.
pub(crate) fn print_listing(
    mut text: String,
    destination: Option<&Path>,
    hint: &str,
) -> Result<ExitCode> {
    // Every rendering ends in a newline, like `output::render`'s do.
    text.push('\n');
    output::write_rendered(&Rendered::Text(text), destination)?;

    // On standard error, so it does not land in a redirect.
    if destination.is_none() && Presentation::detect(None).interactive {
        ui::info(hint);
    }
    Ok(ExitCode::Ok)
}

/// The table a listing is drawn in, with its header and nothing else yet.
pub(crate) fn listing_table(headers: &[&str], presentation: Presentation) -> Table {
    let mut table = Table::new();
    table.load_style(presets::UTF8_FULL_CONDENSED);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    if let Some(width) = presentation.width {
        table.set_width(width);
    }
    table.set_header(output::table::header_cells(headers, presentation));
    if presentation.color {
        table.enforce_styling();
    }
    table
}

/// A listing's rows as JSON: NDJSON is the rows alone, one to a line, and
/// JSON is the document `envelope` wraps them in.
pub(crate) fn rows_or_envelope(
    rows: Vec<serde_json::Value>,
    format: Format,
    envelope: impl FnOnce(Vec<serde_json::Value>) -> serde_json::Value,
) -> Result<String> {
    Ok(match format {
        Format::Ndjson => rows
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?
            .join("\n"),
        _ => serde_json::to_string_pretty(&envelope(rows))?,
    })
}

/// An empty tray is still an answer, and a document asked for is written even
/// when there is nothing in it: `{"username": …, "stories": []}`, the way an
/// empty list command prints `[]`, so a script reading JSON from a pipe has
/// something to parse.
pub(crate) fn empty_document(
    format: Format,
    destination: Option<&Path>,
    json: impl FnOnce() -> Result<String>,
) -> Result<()> {
    if format != Format::Json {
        return Ok(());
    }
    let mut text = json()?;
    text.push('\n');
    output::write_rendered(&Rendered::Text(text), destination)
}

/// Turns a parsed selection into item numbers and refuses any the listing
/// does not have -- before the first request, so `-d 3,9` against a listing
/// of five downloads nothing rather than three items and an error.
pub(crate) fn numbers_of(
    selection: DownloadSelection,
    len: usize,
    no_such: impl Fn(usize) -> anyhow::Error,
) -> Result<Vec<usize>> {
    let numbers: Vec<usize> = match selection {
        DownloadSelection::All => (1..=len).collect(),
        DownloadSelection::These(numbers) => numbers,
    };
    match numbers.iter().find(|&&number| number > len) {
        Some(&number) => Err(no_such(number)),
        None => Ok(numbers),
    }
}

/// Downloads items of one listing by the numbers it printed.
pub(crate) async fn download_selected(
    client: std::sync::Arc<IgClient>,
    stem_base: String,
    items: &[Story],
    selection: DownloadSelection,
    destination: Option<&Path>,
    no_such: impl Fn(usize) -> anyhow::Error,
) -> Result<ExitCode> {
    let numbers = numbers_of(selection, items.len(), &no_such)?;
    let to_stdout = destination.is_some_and(output::is_stdout);
    if to_stdout {
        output::one_file_to_stdout(numbers.len())?;
    }
    let [number] = numbers[..] else {
        return download_many(client, stem_base, items.to_vec(), numbers, destination).await;
    };
    // One-based because that is what the listing shows.
    let story = number
        .checked_sub(1)
        .and_then(|i| items.get(i))
        .ok_or_else(|| no_such(number))?;
    match destination {
        // `-o -`: the file itself, and nothing said about where it went.
        Some(_) if to_stdout => {
            let bytes = bytes_of(&client, story).await?;
            output::write_bytes(&bytes, None)?;
        }
        // A directory that is there already: the file goes into it under the
        // listing's name, as several would.
        Some(dir) if dir.is_dir() => {
            match save_story(&client, &stem_base, items, number, dir).await? {
                Saved::Now(path) => ui::info(&format!("Saved {}", path.display())),
                Saved::Already(path) => ui::info(&format!("Already saved {}", path.display())),
            }
        }
        // The user named it, so replacing what is there is their call -- and so
        // is downloading it again.
        Some(path) => {
            let bytes = bytes_of(&client, story).await?;
            output::write_bytes(&bytes, Some(path))?;
            ui::info(&format!("Saved {}", path.display()));
        }
        None => match save_story(&client, &stem_base, items, number, Path::new(".")).await? {
            Saved::Now(path) => ui::info(&format!("Saved {}", path.display())),
            Saved::Already(path) => ui::info(&format!("Already saved {}", path.display())),
        },
    }
    Ok(ExitCode::Ok)
}
