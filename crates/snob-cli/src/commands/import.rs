//! `snob import dyi`: reads the archive Instagram hands over under "Download
//! your information" (`snob_engine::import`) and prints the same
//! relationships the live commands do. **It stores nothing.**

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;
use snob_engine::exit::ExitCode;
use snob_engine::import::Analysis;

use crate::cli::Format;
use crate::output::{self, Rendered};
use crate::ui;

/// `snob import`'s subcommands. Next to the one function that takes them.
#[derive(Subcommand, Debug)]
pub enum ImportCommand {
    /// Import Instagram's "Download your information" archive
    Dyi {
        /// The archive Instagram sent, or the folder it was extracted into
        path: PathBuf,
    },
}

pub fn run(command: ImportCommand) -> Result<ExitCode> {
    let ImportCommand::Dyi { path } = command;
    let (shape, analysis) = snob_engine::import::read(&path)?;

    ui::info(&format!(
        "Read {} followers and {} following from the {} export at {}",
        analysis.followers,
        analysis.following,
        shape.as_str(),
        path.display()
    ));
    ui::warn(
        "an export describes the moment Instagram built it, not this one; \
         anything that changed since is not in here",
    );

    let format = output::effective_format(None, None);
    output::write_rendered(&render(&analysis, format)?, None)?;
    Ok(ExitCode::Ok)
}

fn render(analysis: &Analysis, format: Format) -> Result<Rendered> {
    // Without `--format` or `-o` the choice is only ever a table on a terminal
    // or JSON down a pipe; the other formats cannot be reached from here.
    if format == Format::Table {
        let mut out = String::new();
        for (label, count) in [
            ("Followers:", analysis.followers),
            ("Following:", analysis.following),
            ("Friends:", analysis.friends.len()),
            ("Fans:", analysis.fans.len()),
            ("Unfollowers:", analysis.unfollowers.len()),
        ] {
            out.push_str(&format!("{label:<14}{count}\n"));
        }
        if !analysis.unfollowers.is_empty() {
            out.push('\n');
            for name in &analysis.unfollowers {
                // Off a file somebody else could have written, and drawn on a
                // terminal: the same rule as every name off the wire.
                out.push_str(&snob_core::model::printable(name));
                out.push('\n');
            }
        }
        return Ok(Rendered::Text(out));
    }

    // The keys match `snob scan`, so a script reading one reads the other.
    let object = serde_json::json!({
        "source": "dyi",
        "counts": {
            "followers": analysis.followers,
            "following": analysis.following,
            "friends": analysis.friends.len(),
            "fans": analysis.fans.len(),
            "unfollowers": analysis.unfollowers.len(),
        },
        "unfollowers": analysis.unfollowers,
        "fans": analysis.fans,
        "friends": analysis.friends,
    });
    let mut text = serde_json::to_string_pretty(&object)?;
    text.push('\n');
    Ok(Rendered::Text(text))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::Value;

    use super::*;

    fn one_of_each() -> Analysis {
        Analysis {
            followers: 2,
            following: 2,
            friends: vec!["bob".into()],
            fans: vec!["ann".into()],
            unfollowers: vec!["dan".into()],
        }
    }

    fn rendered_text(analysis: &Analysis, format: Format) -> String {
        let Rendered::Text(text) = render(analysis, format).unwrap() else {
            panic!("every import form is text");
        };
        text
    }

    #[test]
    fn the_json_output_names_where_it_came_from() {
        let value: Value =
            serde_json::from_str(&rendered_text(&one_of_each(), Format::Json)).unwrap();
        assert_eq!(value["source"], "dyi");
        assert_eq!(value["counts"]["unfollowers"], 1);
        assert_eq!(value["unfollowers"][0], "dan");
        assert_eq!(value["fans"][0], "ann");
        assert_eq!(value["friends"][0], "bob");
    }

    #[test]
    fn the_table_lists_the_unfollowers_under_the_counts() {
        let table = rendered_text(&one_of_each(), Format::Table);
        assert!(table.contains("Followers:    2\n"), "{table}");
        assert!(table.contains("Unfollowers:  1\n"), "{table}");
        assert!(table.ends_with("\ndan\n"), "{table}");
    }

    /// The whole reader, end to end: an extracted export in, the table out.
    #[test]
    fn an_export_reads_through_to_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let lists = dir.path().join("connections/followers_and_following");
        std::fs::create_dir_all(&lists).unwrap();
        let entry = |name: &str| {
            format!(
                r#"{{"string_list_data":[{{"href":"https://www.instagram.com/{name}","value":"{name}"}}]}}"#
            )
        };
        let mut followers = std::fs::File::create(lists.join("followers_1.json")).unwrap();
        write!(followers, "[{},{}]", entry("ann"), entry("bob")).unwrap();
        let mut following = std::fs::File::create(lists.join("following.json")).unwrap();
        write!(
            following,
            r#"{{"relationships_following":[{},{}]}}"#,
            entry("bob"),
            entry("dan")
        )
        .unwrap();
        let (_, analysis) = snob_engine::import::read(dir.path()).unwrap();
        let table = rendered_text(&analysis, Format::Table);
        assert!(table.contains("Followers:    2\n"), "{table}");
        assert!(table.ends_with("\ndan\n"), "{table}");
    }
}
