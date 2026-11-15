//! Writing the result.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use snob_core::model::User;

use crate::cli::Format;
use crate::ui;

pub use snob_engine::files::{
    Replacing, StdoutSink, create_new, default_path, file_named, is_stdout, one_file_to_stdout,
    stdout_download_ahead, usable_stem, write_bytes,
};

/// The six JSON keys of `User`, in the order the struct declares them: the
/// column set every row format shares.
pub(crate) const USER_COLUMNS: [&str; 6] = [
    "pk",
    "username",
    "full_name",
    "is_private",
    "is_verified",
    "pfp_url",
];

pub(crate) mod csv;
pub(crate) mod grid;
pub(crate) mod md;
pub(crate) mod media;
pub(crate) mod posts;
pub(crate) mod table;
#[cfg(feature = "xlsx")]
pub(crate) mod xlsx;

/// How the result will be seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presentation {
    /// The result lands on a terminal a person is looking at: no `-o`, and
    /// standard output is not redirected.
    pub interactive: bool,
    /// That terminal understands OSC 8 hyperlinks.
    pub hyperlinks: bool,
    /// Styling is welcome there.
    pub color: bool,
    /// Width to lay a table out in.
    pub width: Option<u16>,
}

impl Presentation {
    /// Reads the environment.
    pub fn detect(destination: Option<&Path>) -> Self {
        let interactive = file_named(destination).is_none()
            && std::io::IsTerminal::is_terminal(&std::io::stdout());
        Self {
            interactive,
            hyperlinks: interactive && supports_hyperlinks::on(supports_hyperlinks::Stream::Stdout),
            color: interactive && console::colors_enabled(),
            width: None,
        }
    }
}

/// What a renderer produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendered {
    Text(String),
    Bytes(Vec<u8>),
}

impl Rendered {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

/// Picks the format: whatever was asked for, then what the destination's
/// extension implies, and failing both a table on a terminal and JSON in a
/// pipe.
pub fn effective_format(requested: Option<Format>, destination: Option<&Path>) -> Format {
    let destination = file_named(destination);
    if let Some(format) = requested {
        return format;
    }
    if let Some(format) = destination.and_then(format_from_extension) {
        return format;
    }
    // A file whose extension names no format gets the plain list, wherever
    // this runs: the question below is about standard output, not the file.
    if destination.is_some() {
        return Format::Table;
    }
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        Format::Table
    } else {
        Format::Json
    }
}

fn format_from_extension(path: &Path) -> Option<Format> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "csv" => Some(Format::Csv),
        "xlsx" => Some(Format::Xlsx),
        "md" | "markdown" => Some(Format::Md),
        "json" => Some(Format::Json),
        "ndjson" | "jsonl" => Some(Format::Ndjson),
        // Anything else is a plain list, not a guess: `-o notes.txt`.
        _ => None,
    }
}

/// Refuses up front what would only fail once the walk had already been paid
/// for. Called before the first request, never after.
pub fn check_destination(format: Format, destination: Option<&Path>) -> Result<()> {
    let destination = file_named(destination);
    // Before the walk, like every other refusal here: a build without the
    // feature still parses `--format xlsx` and still reads the extension, so
    // the answer has to be given here and not after the requests were spent.
    #[cfg(not(feature = "xlsx"))]
    if format == Format::Xlsx {
        return Err(anyhow!(
            "this build of snob was made without the \"xlsx\" format; use csv, json, ndjson or md"
        ));
    }
    if format == Format::Xlsx && destination.is_none() {
        return Err(anyhow!(
            "the \"xlsx\" format is a binary file; write it with -o (for example -o result.xlsx)"
        ));
    }
    Ok(())
}

pub fn write(
    users: &[User],
    format: Format,
    presentation: Presentation,
    destination: Option<&Path>,
) -> Result<()> {
    write_rendered(&render(users, format, presentation)?, destination)
}

/// Writes to a name **this program chose**, refusing to touch anything that is
/// already there.
pub fn write_new(rendered: &Rendered, path: &Path) -> Result<()> {
    create_new(path)?
        .write_all(rendered.as_bytes())
        .with_context(|| format!("could not write {}", path.display()))?;

    ui::info(&format!("Written to {}", path.display()));
    Ok(())
}

/// Writes an already-rendered result to the destination, with the same
/// file-vs-stdout behavior every command shares.
pub fn write_rendered(rendered: &Rendered, destination: Option<&Path>) -> Result<()> {
    write_bytes(rendered.as_bytes(), destination)
}

/// The users with their names through `printable`, for the JSON formats.
///
/// **JSON is read by programs that hand it on, and one of them is a model.**
pub(crate) fn readable(users: &[User]) -> Vec<User> {
    users
        .iter()
        .map(|user| User {
            username: user.safe_username(),
            full_name: user.safe_full_name(),
            ..user.clone()
        })
        .collect()
}

fn render(users: &[User], format: Format, presentation: Presentation) -> Result<Rendered> {
    if format == Format::Xlsx {
        #[cfg(feature = "xlsx")]
        return Ok(Rendered::Bytes(xlsx::workbook(users)?));
        #[cfg(not(feature = "xlsx"))]
        anyhow::bail!("this build of snob was made without the \"xlsx\" format");
    }
    Ok(Rendered::Text(match format {
        // The drawn table is for someone watching it appear.
        Format::Table if presentation.interactive => table::table(users, presentation),
        Format::Table => table::plain(users),
        Format::Json => {
            let mut s = serde_json::to_string_pretty(&readable(users))?;
            s.push('\n');
            s
        }
        Format::Ndjson => {
            let mut s = String::new();
            for u in readable(users) {
                s.push_str(&serde_json::to_string(&u)?);
                s.push('\n');
            }
            s
        }
        Format::Csv => csv::rows(users)?,
        Format::Md => md::table(users),
        // Handled above: it is the one format that is not text.
        Format::Xlsx => unreachable!(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Pk;

    fn users() -> Vec<User> {
        vec![
            User {
                pk: Pk::new(1),
                username: "one".into(),
                full_name: Some("One".into()),
                is_private: None,
                is_verified: Some(true),
                pfp_url: None,
            },
            User {
                pk: Pk::new(2),
                username: "two".into(),
                full_name: None,
                is_private: None,
                is_verified: None,
                pfp_url: None,
            },
        ]
    }

    /// Everything off: what a pipe and a file get.
    fn plain() -> Presentation {
        Presentation {
            interactive: false,
            hyperlinks: false,
            color: false,
            width: None,
        }
    }

    fn text(users: &[User], format: Format) -> String {
        match render(users, format, plain()).unwrap() {
            Rendered::Text(text) => text,
            Rendered::Bytes(_) => panic!("expected text, got bytes"),
        }
    }

    #[test]
    fn the_table_is_one_name_per_line() {
        assert_eq!(text(&users(), Format::Table), "one\ntwo\n");
    }

    #[test]
    fn ndjson_is_one_object_per_line() {
        let out = text(&users(), Format::Ndjson);
        assert_eq!(out.lines().count(), 2);
        assert!(out.lines().all(|l| l.starts_with('{')));
    }

    #[test]
    fn json_omits_the_empty_fields() {
        let out = text(&users(), Format::Json);
        assert!(out.contains("\"is_verified\": true"));
        assert!(!out.contains("null"));
    }

    /// A right-to-left override and a tag-block payload, which `serde_json`
    /// would write as they are, reach neither JSON format.
    #[test]
    fn the_json_formats_carry_no_invisible_characters_in_a_name() {
        let mut list = users();
        list[0].full_name = Some("\u{202e}enO\u{e0049}\u{e0067}".into());
        for format in [Format::Json, Format::Ndjson] {
            let out = text(&list, format);
            assert!(out.contains("\"enO\""), "{out}");
            assert!(
                !out.contains('\u{202e}') && !out.contains('\u{e0049}'),
                "{out}"
            );
        }
    }

    #[test]
    fn an_empty_list_writes_nothing() {
        assert_eq!(text(&[], Format::Table), "");
        assert_eq!(text(&[], Format::Ndjson), "");
    }

    /// Every format a list can be asked for produces something.
    #[cfg(feature = "xlsx")]
    #[test]
    fn a_list_renders_in_every_format() {
        for f in [
            Format::Table,
            Format::Json,
            Format::Ndjson,
            Format::Csv,
            Format::Xlsx,
            Format::Md,
        ] {
            assert!(render(&users(), f, plain()).is_ok(), "{f:?} should render");
        }
    }

    #[cfg(feature = "xlsx")]
    #[test]
    fn a_spreadsheet_reaches_the_file_as_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.xlsx");
        write(&users(), Format::Xlsx, plain(), Some(&path)).unwrap();

        let written = std::fs::read(&path).unwrap();
        assert_eq!(&written[..4], b"PK\x03\x04", "a workbook is a zip archive");
    }

    #[test]
    fn csv_reaches_the_file_it_was_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.csv");
        write(&users(), Format::Csv, plain(), Some(&path)).unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(written, text(&users(), Format::Csv));
        assert!(written.starts_with("pk,username"), "{written}");
    }

    #[test]
    fn an_explicit_format_beats_the_extension() {
        let path = Path::new("list.csv");
        assert_eq!(
            effective_format(Some(Format::Json), Some(path)),
            Format::Json
        );
    }

    #[test]
    fn the_extension_picks_the_format_when_nothing_was_asked_for() {
        for (name, expected) in [
            ("list.csv", Format::Csv),
            ("LIST.CSV", Format::Csv),
            ("list.xlsx", Format::Xlsx),
            ("list.md", Format::Md),
            ("list.markdown", Format::Md),
            ("list.json", Format::Json),
            ("list.ndjson", Format::Ndjson),
            ("list.jsonl", Format::Ndjson),
        ] {
            let got = effective_format(None, Some(Path::new(name)));
            assert_eq!(got, expected, "{name}");
        }
    }

    /// An extension nobody claimed is not a guess: `-o notes.txt` is a plain
    /// list, whether or not standard output is a terminal.
    #[test]
    fn an_unknown_extension_does_not_guess() {
        for name in ["notes.txt", "notes"] {
            let got = effective_format(None, Some(Path::new(name)));
            assert_eq!(got, Format::Table, "{name}");
        }
    }

    /// Without the feature the refusal comes first, before a walk is paid for,
    /// and names the formats that are there.
    #[cfg(not(feature = "xlsx"))]
    #[test]
    fn a_build_without_xlsx_refuses_the_format_up_front() {
        let error = check_destination(Format::Xlsx, Some(Path::new("x.xlsx"))).unwrap_err();
        assert!(
            error.to_string().contains("without the \"xlsx\" format"),
            "{error}"
        );
    }

    #[cfg(feature = "xlsx")]
    #[test]
    fn a_spreadsheet_needs_a_file_to_go_to() {
        let error = check_destination(Format::Xlsx, None).unwrap_err();
        assert!(error.to_string().contains("-o"), "{error}");
        assert!(check_destination(Format::Xlsx, Some(Path::new("x.xlsx"))).is_ok());
    }

    #[test]
    fn the_text_formats_are_happy_on_standard_output() {
        for f in [Format::Table, Format::Json, Format::Ndjson, Format::Csv] {
            assert!(check_destination(f, None).is_ok(), "{f:?}");
        }
    }

    #[test]
    fn a_file_is_never_interactive() {
        let presentation = Presentation::detect(Some(Path::new("out.csv")));
        assert!(!presentation.interactive);
        assert!(!presentation.hyperlinks);
        assert!(!presentation.color);
    }

    #[test]
    fn a_destination_that_cannot_be_written_says_which_one() {
        let dir = tempfile::tempdir().unwrap();
        // A directory is the everyday version of this: `-o` pointed at a
        // folder rather than a file inside it.
        let error = write_rendered(&Rendered::Text("x".into()), Some(dir.path())).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("could not write"), "{message}");
    }

    #[test]
    fn bytes_reach_the_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        let payload = vec![0x50, 0x4b, 0x03, 0x04, 0x00, 0xff];
        write_rendered(&Rendered::Bytes(payload.clone()), Some(&path)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), payload);
    }

    /// The columns are a projection of `User`'s serialized shape, so the
    /// authority is the struct itself: serialize one with every field set and
    /// compare the key sets. The *order* cannot be read back -- serde_json
    /// sorts an object's keys -- so order stays a reviewed fact; what this
    /// holds is that a field added to `User` fails here first, which is the
    /// reminder to decide its column everywhere at once.
    #[test]
    fn the_shared_columns_are_the_struct_keys() {
        let user = snob_core::model::User {
            pk: snob_core::Pk::new(1),
            username: "a".to_string(),
            full_name: Some("b".to_string()),
            is_private: Some(true),
            is_verified: Some(false),
            pfp_url: Some("https://example.test/p.jpg".to_string()),
        };
        let value = serde_json::to_value(&user).expect("a user serializes");
        let keys: Vec<&str> = value
            .as_object()
            .expect("a user is an object")
            .keys()
            .map(String::as_str)
            .collect();
        let mut columns = USER_COLUMNS.to_vec();
        columns.sort_unstable();
        assert_eq!(keys, columns, "serde_json answers the keys sorted");
    }
}
