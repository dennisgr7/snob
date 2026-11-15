//! Comma-separated rows.

use anyhow::{Context, Result};
use snob_core::model::{User, printable};

use crate::output::USER_COLUMNS as HEADER;

/// One row per account.
pub(crate) fn rows(users: &[User]) -> Result<String> {
    let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
    writer
        .write_record(HEADER)
        .context("could not write the csv header")?;

    for user in users {
        let fields: [String; 6] = [
            user.pk.to_string(),
            clean(&user.username),
            optional(user.full_name.as_deref()),
            flag(user.is_private),
            flag(user.is_verified),
            optional(user.pfp_url.as_deref()),
        ];
        writer
            .write_record(&fields)
            .with_context(|| format!("could not write the row for {}", user.safe_username()))?;
    }

    finish(writer)
}

/// A header and one row, for the summary, which is counts rather than
/// accounts.
pub(crate) fn single_row(header: &[&str], row: &[String]) -> Result<String> {
    let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
    writer
        .write_record(header)
        .context("could not write the csv header")?;
    let fields: Vec<String> = row.iter().map(|field| clean(field)).collect();
    writer
        .write_record(&fields)
        .context("could not write the csv row")?;
    finish(writer)
}

/// A header and any number of rows, for what is not accounts
/// (`output::grid`).
pub(crate) fn many_rows(header: &[&str], rows: &[Vec<String>]) -> Result<String> {
    let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
    writer
        .write_record(header)
        .context("could not write the csv header")?;
    for row in rows {
        let fields: Vec<String> = row.iter().map(|field| clean(field)).collect();
        writer
            .write_record(&fields)
            .context("could not write a csv row")?;
    }
    finish(writer)
}

fn finish(writer: csv::Writer<Vec<u8>>) -> Result<String> {
    let bytes = writer
        .into_inner()
        .context("could not finish the csv")?
        .to_vec();
    String::from_utf8(bytes).context("the csv came out as invalid UTF-8")
}

/// An unknown attribute is an empty field, never `false`: not knowing whether
/// an account is private is not the same as knowing it is not.
fn flag(value: Option<bool>) -> String {
    match value {
        Some(true) => "true".into(),
        Some(false) => "false".into(),
        None => String::new(),
    }
}

fn optional(value: Option<&str>) -> String {
    value.map(clean).unwrap_or_default()
}

/// A field somebody else wrote, made safe to open: through `printable`, and
/// with an apostrophe ahead of one a spreadsheet would run as a formula, which
/// is one that begins with `=`, `+`, `-` or `@`.
///
/// **The first non-blank character decides, not the first**: a spreadsheet
/// skips leading spaces before it looks, and `printable` has turned a leading
/// tab or carriage return into one.
fn clean(field: &str) -> String {
    let field = printable(field);
    match field.chars().find(|c| !c.is_whitespace()) {
        Some('=' | '+' | '-' | '@') => format!("'{field}"),
        _ => field,
    }
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
                full_name: Some("One Person".into()),
                is_private: Some(false),
                is_verified: Some(true),
                pfp_url: Some("https://example.test/a.jpg".into()),
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

    fn parse(text: &str) -> Vec<Vec<String>> {
        csv::ReaderBuilder::new()
            .has_headers(false)
            .from_reader(text.as_bytes())
            .records()
            .map(|record| record.unwrap().iter().map(str::to_string).collect())
            .collect()
    }

    #[test]
    fn the_header_names_the_six_frozen_keys() {
        let out = rows(&users()).unwrap();
        assert_eq!(
            out.lines().next().unwrap(),
            "pk,username,full_name,is_private,is_verified,pfp_url"
        );
    }

    #[test]
    fn an_unknown_attribute_is_empty_not_false() {
        let records = parse(&rows(&users()).unwrap());
        assert!(records.iter().all(|r| r.len() == 6), "{records:?}");

        let known = &records[1];
        assert_eq!(known[3], "false");
        assert_eq!(known[4], "true");

        let unknown = &records[2];
        assert_eq!(unknown[2], "");
        assert_eq!(unknown[3], "");
        assert_eq!(unknown[5], "");
    }

    /// Commas and quotes are the writer's problem and it handles them. A
    /// newline is not: quoted or not, it draws a second row on a terminal, and
    /// one account must not be able to look like two. It becomes a space here
    /// exactly as it does in the drawn table and the markdown one. Whoever
    /// needs the name byte-for-byte has `--format json`, where a newline
    /// travels as `\n` and no terminal acts on it.
    #[test]
    fn commas_and_quotes_survive_but_a_newline_becomes_a_space() {
        let awkward = "Comma, \"quote\" and\na newline";
        let user = User {
            full_name: Some(awkward.into()),
            ..users()[0].clone()
        };
        let out = rows(&[user]).unwrap();
        assert_eq!(out.lines().count(), 2, "one account, one row: {out:?}");

        let records = parse(&out);
        assert_eq!(records[1][2], "Comma, \"quote\" and a newline");
    }

    #[test]
    fn a_name_that_looks_like_a_formula_is_defused() {
        for dangerous in ["=1+1", "+1", "-1", "@SUM(A1)"] {
            let user = User {
                full_name: Some(dangerous.into()),
                ..users()[0].clone()
            };
            let records = parse(&rows(&[user]).unwrap());
            assert_eq!(records[1][2], format!("'{dangerous}"), "{dangerous}");
        }
    }

    /// `--format csv` with no `-o` writes to the terminal, so this file is an
    /// output path like any other and the same filter has to apply.
    #[test]
    fn a_hostile_name_cannot_drive_the_terminal() {
        let user = User {
            username: "someone".into(),
            full_name: Some(format!(
                "{esc}]8;;http://evil.test{esc}\\Official{esc}]8;;{esc}\\",
                esc = '\x1b'
            )),
            pfp_url: Some(format!("https://example.test/a.jpg{}[2K", '\x1b')),
            ..users()[0].clone()
        };
        let out = rows(&[user]).unwrap();
        assert!(!out.contains('\x1b'), "{out:?}");
        // The address survives as text — it is characters in a name now — but
        // not as a sequence the terminal obeys.
        assert!(out.contains("evil.test"), "{out}");
    }

    /// The summary shares the writer, and its cells come from the same places.
    #[test]
    fn the_single_row_is_filtered_too() {
        let out = single_row(&["a"], &[format!("x{}[2K", '\x1b')]).unwrap();
        assert!(!out.contains('\x1b'), "{out:?}");
    }

    /// A formula hiding behind a control character. Defusing before filtering
    /// would clear it as harmless and then strip the character that was hiding
    /// it, which is how `=1+1` reaches a spreadsheet unmarked.
    #[test]
    fn a_formula_behind_a_control_character_is_still_defused() {
        let user = User {
            full_name: Some(format!("{}=1+1", '\x1b')),
            ..users()[0].clone()
        };
        let records = parse(&rows(&[user]).unwrap());
        assert_eq!(records[1][2], "'=1+1");
    }

    /// The half the test above cannot see. An escape is *removed* by
    /// `printable`, so the `=` ends up first; a tab or a carriage return is
    /// turned into a **space**, on purpose, and leaves the `=` in second place.
    /// Both spreadsheets trim leading whitespace on import, so what they
    /// evaluate is a formula unless the first non-blank character is checked.
    #[test]
    fn a_formula_behind_whitespace_is_defused_too() {
        for hidden in ['\t', '\r', '\n', ' '] {
            let user = User {
                full_name: Some(format!("{hidden}=1+1")),
                ..users()[0].clone()
            };
            let records = parse(&rows(&[user]).unwrap());
            assert_eq!(
                records[1][2], "' =1+1",
                "a formula behind {hidden:?} reached the spreadsheet"
            );
        }
    }

    #[test]
    fn an_ordinary_name_is_left_alone() {
        let records = parse(&rows(&users()).unwrap());
        assert_eq!(records[1][2], "One Person");
    }

    #[test]
    fn an_empty_list_is_just_the_header() {
        let out = rows(&[]).unwrap();
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn a_single_row_carries_its_own_header() {
        let out = single_row(&["a", "b"], &["1".into(), "2".into()]).unwrap();
        assert_eq!(out.lines().collect::<Vec<_>>(), vec!["a,b", "1,2"]);
    }
}
