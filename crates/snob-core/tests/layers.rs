//! The crates depend on each other one way:
//! `snob-core <- snob-ig, snob-store <- snob-engine <- snob-mcp <- snob-cli`.
//!
//! **What it checks**, reading the manifests: `snob-engine` depends on no
//! crate that draws on or reads from a terminal, nor on `snob-mcp` or
//! `snob-cli`, so an app can link it in process; `snob-mcp` does not depend on
//! `snob-cli`; and `snob-ig` does not depend on `snob-store`. A dependency is
//! one of `[dependencies]`, `[build-dependencies]` or a target's, and a
//! dev-dependency is held to the same: no test points up either.

mod common;
use common::repo_root;

/// The crates that draw on or read from a terminal.
const TERMINAL: [&str; 6] = [
    "clap",
    "console",
    "indicatif",
    "ratatui",
    "crossterm",
    "dialoguer",
];

/// The dependencies a manifest names, by section kind: the build's
/// (`dependencies`, `build-dependencies`, a target's) and the tests'.
#[derive(Debug, Default, PartialEq, Eq)]
struct Named {
    build: Vec<String>,
    dev: Vec<String>,
}

fn named_in(manifest: &str) -> Named {
    #[derive(Clone, Copy)]
    enum Section {
        Build,
        Dev,
        Other,
    }
    let mut named = Named::default();
    let mut section = Section::Other;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_end_matches(']');
            let last = header.rsplit('.').next().unwrap_or(header);
            section = match last {
                "dependencies" | "build-dependencies" => Section::Build,
                "dev-dependencies" => Section::Dev,
                _ => Section::Other,
            };
            continue;
        }
        let Some(name) = line
            .split(['=', '.', ' '])
            .next()
            .map(|name| name.trim_matches('"'))
            .filter(|name| {
                line.contains('=')
                    && !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
        else {
            continue;
        };
        match section {
            Section::Build => named.build.push(name.to_string()),
            Section::Dev => named.dev.push(name.to_string()),
            Section::Other => {}
        }
    }
    named
}

fn manifest_of(root: &std::path::Path, krate: &str) -> Named {
    let path = root.join("crates").join(krate).join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    named_in(&text)
}

/// What a manifest breaks of the layering, as sentences.
fn broken(engine: &Named, mcp: &Named, ig: &Named) -> Vec<String> {
    let mut broken = Vec::new();
    for name in engine.build.iter().chain(&engine.dev) {
        if TERMINAL.contains(&name.as_str()) {
            broken.push(format!("snob-engine depends on {name}, a terminal crate"));
        }
        if name == "snob-mcp" || name == "snob-cli" {
            broken.push(format!("snob-engine depends on {name}"));
        }
    }
    if mcp
        .build
        .iter()
        .chain(&mcp.dev)
        .any(|name| name == "snob-cli")
    {
        broken.push("snob-mcp depends on snob-cli".to_string());
    }
    if ig
        .build
        .iter()
        .chain(&ig.dev)
        .any(|name| name == "snob-store")
    {
        broken.push("snob-ig depends on snob-store".to_string());
    }
    broken
}

#[test]
fn the_crates_depend_on_each_other_one_way() {
    let Some(root) = repo_root() else {
        return; // packaged build, nothing to read
    };
    let broken = broken(
        &manifest_of(&root, "snob-engine"),
        &manifest_of(&root, "snob-mcp"),
        &manifest_of(&root, "snob-ig"),
    );
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// The reading finds a dependency in every spelling a manifest has, in every
/// kind of section, and passes over what is no dependency.
#[test]
fn every_spelling_of_a_dependency_is_read() {
    let named = named_in(
        r#"
[package]
name = "x"

[features]
mcp = ["dep:snob-mcp"]

[dependencies]
snob-core.workspace = true
# clap = "4"
tokio = { workspace = true, features = ["net"] }
"console" = "0.16"

[target.'cfg(windows)'.dependencies]
windows-sys.workspace = true

[dev-dependencies]
snob-cli.workspace = true
"#,
    );
    assert_eq!(
        named.build,
        ["snob-core", "tokio", "console", "windows-sys"]
    );
    assert_eq!(named.dev, ["snob-cli"]);
}

/// Each way up is caught, a test's included.
#[test]
fn a_dependency_that_points_up_is_caught() {
    let of = |build: &[&str], dev: &[&str]| Named {
        build: build.iter().map(|s| s.to_string()).collect(),
        dev: dev.iter().map(|s| s.to_string()).collect(),
    };
    let clean = || {
        (
            of(&["snob-core"], &[]),
            of(&["snob-engine"], &["tempfile"]),
            of(&[], &[]),
        )
    };
    let (engine, mcp, ig) = clean();
    assert!(broken(&engine, &mcp, &ig).is_empty());
    for (engine, mcp, ig, said) in [
        (of(&["clap"], &[]), clean().1, clean().2, "terminal crate"),
        (
            of(&[], &["ratatui"]),
            clean().1,
            clean().2,
            "terminal crate",
        ),
        (
            of(&["snob-mcp"], &[]),
            clean().1,
            clean().2,
            "snob-engine depends on snob-mcp",
        ),
        (
            clean().0,
            of(&["snob-cli"], &[]),
            clean().2,
            "snob-mcp depends on snob-cli",
        ),
        (
            clean().0,
            of(&[], &["snob-cli"]),
            clean().2,
            "snob-mcp depends on snob-cli",
        ),
        (
            clean().0,
            clean().1,
            of(&["snob-store"], &[]),
            "snob-ig depends on snob-store",
        ),
    ] {
        let broken = broken(&engine, &mcp, &ig);
        assert!(
            broken.iter().any(|b| b.contains(said)),
            "{said}: {broken:?}"
        );
    }
}
