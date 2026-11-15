//! The crates depend on each other one way:
//! `snob-core <- snob-ig, snob-store <- snob-engine <- snob-mcp <- snob-cli`.
//!
//! **What it checks**, reading the manifests: `snob-engine` depends on no
//! crate that draws on or reads from a terminal, nor on `snob-mcp` or
//! `snob-cli`, so an app can link it in process; `snob-mcp` does not depend on
//! `snob-cli`; and `snob-ig` does not depend on `snob-store`. A dependency is
//! one of `[dependencies]`, `[build-dependencies]` or a target's, and a
//! dev-dependency is held to the same: no test points up either.
//!
//! **And the fake Instagram stays out of what ships**: `snob-fake-ig` depends
//! on nothing above `snob-ig`, and every crate that names it does so under
//! `[dev-dependencies]`. The desktop app (`apps/desktop`, a workspace of its
//! own) is held to the engine's rule and to that one, and its version is
//! 0.0.0 (not shipped) or the workspace's: the desktop edition's two binaries
//! are one version (`snob_engine::update::edition`), and `release.yml`
//! ships the app only when it says the tag's.

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

const FAKE: &str = "snob-fake-ig";

/// The crates of snob above `snob-ig`: what the fake may not depend on.
const ABOVE_IG: [&str; 4] = ["snob-store", "snob-engine", "snob-mcp", "snob-cli"];

/// What the fake's own manifest breaks, and what a crate naming it breaks.
fn fake_broken(fake: &Named, others: &[(&str, &Named)]) -> Vec<String> {
    let mut broken = Vec::new();
    for name in fake.build.iter().chain(&fake.dev) {
        if ABOVE_IG.contains(&name.as_str()) {
            broken.push(format!("{FAKE} depends on {name}"));
        }
    }
    for (krate, named) in others {
        if named.build.iter().any(|name| name == FAKE) {
            broken.push(format!("{krate} links {FAKE} outside its tests"));
        }
    }
    broken
}

/// What the desktop app's manifest breaks: the engine's rule, and no
/// `snob-mcp` or `snob-cli`, which it starts as a program instead.
fn app_broken(app: &Named) -> Vec<String> {
    let mut broken = Vec::new();
    for name in app.build.iter().chain(&app.dev) {
        if TERMINAL.contains(&name.as_str()) {
            broken.push(format!(
                "the desktop app depends on {name}, a terminal crate"
            ));
        }
        if name == "snob-mcp" || name == "snob-cli" {
            broken.push(format!("the desktop app depends on {name}"));
        }
    }
    broken
}

#[test]
fn the_fake_instagram_is_linked_by_tests_only() {
    let Some(root) = repo_root() else {
        return; // packaged build, nothing to read
    };
    let crates = [
        "snob-core",
        "snob-ig",
        "snob-store",
        "snob-engine",
        "snob-mcp",
        "snob-cli",
    ];
    let named: Vec<Named> = crates.iter().map(|k| manifest_of(&root, k)).collect();
    let mut others: Vec<(&str, &Named)> = crates.iter().copied().zip(&named).collect();
    let app = std::fs::read_to_string(root.join("apps/desktop/Cargo.toml"))
        .ok()
        .map(|text| named_in(&text));
    let mut broken = Vec::new();
    if let Some(app) = &app {
        others.push(("the desktop app", app));
        broken.extend(app_broken(app));
    }
    broken.extend(fake_broken(&manifest_of(&root, FAKE), &others));
    assert!(broken.is_empty(), "{}", broken.join("\n"));
}

/// The first `version = "..."` of a manifest.
fn version_in(manifest: &str) -> Option<&str> {
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|rest| rest.split('"').next())
}

#[test]
fn the_app_is_unshipped_or_the_version_of_the_command_line() {
    let Some(root) = repo_root() else {
        return; // packaged build, nothing to read
    };
    let Ok(app) = std::fs::read_to_string(root.join("apps/desktop/Cargo.toml")) else {
        return;
    };
    let workspace = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let (app, workspace) = (version_in(&app), version_in(&workspace));
    assert!(workspace.is_some());
    assert!(
        app == Some("0.0.0") || app == workspace,
        "apps/desktop/Cargo.toml says {app:?}; it is 0.0.0 or {workspace:?}"
    );
}

#[test]
fn a_fake_linked_into_a_build_is_caught() {
    let of = |build: &[&str], dev: &[&str]| Named {
        build: build.iter().map(|s| s.to_string()).collect(),
        dev: dev.iter().map(|s| s.to_string()).collect(),
    };
    let fake = of(&["snob-core", "snob-ig", "wiremock"], &[]);
    let tested = of(&["snob-ig"], &[FAKE]);
    assert!(fake_broken(&fake, &[("snob-cli", &tested)]).is_empty());

    let linked = of(&[FAKE], &[]);
    assert_eq!(
        fake_broken(&fake, &[("snob-cli", &linked)]),
        ["snob-cli links snob-fake-ig outside its tests"]
    );
    assert_eq!(
        fake_broken(&of(&["snob-store"], &[]), &[]),
        ["snob-fake-ig depends on snob-store"]
    );

    assert!(app_broken(&of(&["snob-engine", "gpui-kit"], &[FAKE])).is_empty());
    assert_eq!(
        app_broken(&of(&["snob-cli"], &["ratatui"])),
        [
            "the desktop app depends on snob-cli",
            "the desktop app depends on ratatui, a terminal crate"
        ]
    );
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
