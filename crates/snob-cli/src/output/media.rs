//! How a listing of stories, highlights or posts is written, and the
//! downloads it names by number.

use std::path::Path;

use anyhow::Result;
use comfy_table::{ContentArrangement, Table, presets};
use snob_engine::exit::ExitCode;
use snob_engine::media::{Saved, Story, bytes_of, download_many, save_story};
use snob_ig::client::IgClient;

use crate::cli::{DownloadSelection, Format};
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
    let to_stdout = destination.is_some_and(snob_engine::files::is_stdout);
    if to_stdout {
        snob_engine::files::one_file_to_stdout(numbers.len())?;
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
            snob_engine::files::write_bytes(&bytes, None)?;
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
            snob_engine::files::write_bytes(&bytes, Some(path))?;
            ui::info(&format!("Saved {}", path.display()));
        }
        None => match save_story(&client, &stem_base, items, number, Path::new(".")).await? {
            Saved::Now(path) => ui::info(&format!("Saved {}", path.display())),
            Saved::Already(path) => ui::info(&format!("Already saved {}", path.display())),
        },
    }
    Ok(ExitCode::Ok)
}

/// `-d --json`: the items `selection` names saved into `destination`, a
/// directory (the working one when none is given), each under a name made of
/// what it is ([`snob_engine::media::stable_stem`], so it keeps it when the
/// tray changes), and what became of each printed as one document: `about`
/// first, then what [`snob_engine::media::save_telling`] says.
pub(crate) struct Telling<'a> {
    pub about: serde_json::Map<String, serde_json::Value>,
    pub username: &'a str,
    /// `story` or `highlight`: the word in each file's name.
    pub kind: &'a str,
    pub items: &'a [Story],
    pub selection: DownloadSelection,
    pub destination: Option<&'a Path>,
    pub bounds: snob_engine::posts::Bounds,
}

impl Telling<'_> {
    pub(crate) async fn run(
        self,
        client: &IgClient,
        no_such: impl Fn(usize) -> anyhow::Error,
    ) -> Result<ExitCode> {
        use snob_engine::media::{MAX_STORY_BYTES, Named, Told, save_telling, stable_stem};

        let numbers = numbers_of(self.selection, self.items.len(), no_such)?;
        let files: Vec<Told> = numbers
            .iter()
            .filter_map(|&number| {
                let story = self.items.get(number.checked_sub(1)?)?;
                Some(Told {
                    file: Named {
                        number,
                        stem: stable_stem(self.username, self.kind, &story.pk),
                        url: story.url.clone(),
                        what: format!("{} {number}", self.kind),
                        cap: MAX_STORY_BYTES,
                        best: None,
                        ffmpeg: None,
                    },
                    pk: story.pk.clone(),
                    kind: story.kind,
                })
            })
            .collect();
        let total = files.len();
        let dir = self.destination.unwrap_or(Path::new("."));
        let (told, failed) = save_telling(client, files, dir, self.bounds).await?;
        let mut document = self.about;
        document.extend(told);
        ui::say!(
            "{}",
            serde_json::to_string_pretty(&serde_json::Value::Object(document))?
        );
        if failed > 0 {
            return Err(snob_engine::exit::ExitError::new(
                ExitCode::Error,
                format!("{failed} of {total} files could not be downloaded"),
            )
            .into());
        }
        Ok(ExitCode::Ok)
    }
}
