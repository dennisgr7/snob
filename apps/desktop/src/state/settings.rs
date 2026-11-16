//! What the settings hold, in sections, and which of it a search shows.
//! **A row is found by everything it is called**: its name, what it says
//! under it, its section's name and words a person might look for it by,
//! all in the app's language and handed in, so a search in Spanish finds
//! what the Spanish screen shows. Words are matched as the palette matches
//! commands (`commands::matches`), with accents folded away.

use snob_core::Pk;
use snob_engine::storage::Kind;

/// A section of the settings, with its heading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    App,
    Storage,
}

/// One setting, or one line of what is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Theme,
    Language,
    DetailedLogging,
    /// The log's folder and the diagnostic report.
    Logs,
    /// What everything kept weighs.
    StorageTotal,
    StorageKind(Kind),
    /// What an account signed in here holds.
    Account(Pk),
}

/// The sections as they read before a search: `storage`, the rows of what is
/// kept, once it was measured.
pub fn every(storage: Vec<Row>) -> Vec<(Section, Vec<Row>)> {
    let mut kept = vec![Row::StorageTotal];
    kept.extend(storage);
    vec![
        (
            Section::App,
            vec![Row::Theme, Row::Language, Row::DetailedLogging, Row::Logs],
        ),
        (Section::Storage, kept),
    ]
}

/// The sections and rows `typed` asks for: a row shows when every word
/// typed is somewhere in what it is called (`words`) or in its section's
/// name (`named`), whatever the case and the accents; a section shows when
/// one of its rows does. Nothing typed shows everything.
pub fn shown(
    sections: Vec<(Section, Vec<Row>)>,
    typed: &str,
    words: impl Fn(Row) -> String,
    named: impl Fn(Section) -> String,
) -> Vec<(Section, Vec<Row>)> {
    let wanted = fold(typed);
    sections
        .into_iter()
        .filter_map(|(section, rows)| {
            let heading = named(section);
            let rows: Vec<Row> = rows
                .into_iter()
                .filter(|row| {
                    let said = fold(&format!("{heading} {}", words(*row)));
                    super::commands::matches(&said, &wanted)
                })
                .collect();
            (!rows.is_empty()).then_some((section, rows))
        })
        .collect()
}

/// `text` in lower case and without its accents, so "lenguaje" finds
/// "Lenguaje" and "espanol" finds the word with its tilde. Only the letters
/// of the languages the app speaks.
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            '\u{e0}'..='\u{e4}' => 'a',
            '\u{e8}'..='\u{eb}' => 'e',
            '\u{ec}'..='\u{ef}' => 'i',
            '\u{f2}'..='\u{f6}' => 'o',
            '\u{f9}'..='\u{fc}' => 'u',
            '\u{f1}' => 'n',
            '\u{e7}' => 'c',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(row: Row) -> String {
        match row {
            Row::Theme => "Theme appearance dark light mode",
            Row::Language => "Language English Espa\u{f1}ol",
            Row::DetailedLogging => "Detailed logging For finding a problem",
            Row::Logs => "Open the logs folder Copy the diagnostic report",
            Row::StorageTotal => "snob keeps 18.4 MB on this computer",
            Row::StorageKind(Kind::Cache) => "Cache What a view shows at once Clear cache",
            Row::StorageKind(_) => "Databases Lists and walks",
            Row::Account(_) => "@me",
        }
        .to_string()
    }

    fn named(section: Section) -> String {
        match section {
            Section::App => "Application",
            Section::Storage => "Storage",
        }
        .to_string()
    }

    fn all() -> Vec<(Section, Vec<Row>)> {
        every(vec![
            Row::StorageKind(Kind::Databases),
            Row::StorageKind(Kind::Cache),
            Row::Account(Pk::new(42)),
        ])
    }

    #[test]
    fn nothing_typed_shows_every_section_in_its_order() {
        let shown = shown(all(), "  ", words, named);
        assert_eq!(shown, all());
        assert_eq!(shown[0].1[0], Row::Theme);
        assert_eq!(shown[1].1[0], Row::StorageTotal);
    }

    #[test]
    fn a_row_is_found_by_its_words_and_its_keywords_whatever_the_case() {
        assert_eq!(
            shown(all(), "DARK", words, named),
            [(Section::App, vec![Row::Theme])]
        );
        assert_eq!(
            shown(all(), "cach clear", words, named),
            [(Section::Storage, vec![Row::StorageKind(Kind::Cache)])]
        );
    }

    #[test]
    fn a_section_s_name_finds_every_row_in_it() {
        let found = shown(all(), "storage", words, named);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.len(), 4);
    }

    #[test]
    fn a_search_that_matches_nothing_shows_nothing() {
        assert!(shown(all(), "printer", words, named).is_empty());
        assert!(shown(all(), "theme printer", words, named).is_empty());
    }

    #[test]
    fn accents_need_not_be_typed_and_may_be() {
        assert_eq!(
            shown(all(), "espanol", words, named),
            [(Section::App, vec![Row::Language])]
        );
        assert_eq!(
            shown(all(), "ESPA\u{d1}OL", words, named),
            [(Section::App, vec![Row::Language])]
        );
        assert_eq!(fold("Caf\u{e9} \u{c9}XITO"), "cafe exito");
    }
}
