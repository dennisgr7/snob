//! Workspace maintenance tasks (`cargo xtask ...`).
//!
//! Kept `std`-only so it compiles fast without adding crates to `Cargo.lock`.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

/// Nextest filter matching the source-reading guards that walk the repository.
const GUARDS_FILTER: &str = "package(snob-core) & kind(test)";

/// Root paths whose changes affect how the whole workspace builds or tests.
const BUILD_WIDE_FILES: [&str; 5] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "deny.toml",
    ".config/nextest.toml",
];

/// Directory prefixes whose changes invalidate the whole workspace.
const BUILD_WIDE_PREFIXES: [&str; 2] = [".cargo/", "xtask/"];

struct AffectedArgs {
    base: Option<String>,
    print: bool,
    extra: Vec<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some((cmd, rest)) = args.split_first() else {
        eprintln!("usage: cargo xtask affected [--base <rev>] [--print] [-- <nextest args>]");
        return ExitCode::from(2);
    };
    if cmd != "affected" {
        eprintln!("unknown xtask `{cmd}` (expected `affected`)");
        return ExitCode::from(2);
    }

    let parsed = match parse_args(rest) {
        Ok(parsed) => parsed,
        Err(msg) => {
            eprintln!("error: {msg}");
            return ExitCode::from(2);
        }
    };

    match run_affected(&parsed) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<AffectedArgs, String> {
    let mut base = None;
    let mut print = false;
    let mut extra = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            extra.extend(args[i + 1..].iter().cloned());
            break;
        }
        if arg == "--print" {
            print = true;
            i += 1;
        } else if arg == "--base" {
            let val = args
                .get(i + 1)
                .ok_or_else(|| "--base requires a revision".to_string())?;
            base = Some(val.clone());
            i += 2;
        } else if let Some(val) = arg.strip_prefix("--base=") {
            if val.is_empty() {
                return Err("--base requires a revision".to_string());
            }
            base = Some(val.to_string());
            i += 1;
        } else if arg == "-h" || arg == "--help" {
            return Err(
                "usage: cargo xtask affected [--base <rev>] [--print] [-- <nextest args>]"
                    .to_string(),
            );
        } else {
            return Err(format!(
                "unknown argument `{arg}` (pass nextest flags after `--`)"
            ));
        }
    }

    Ok(AffectedArgs { base, print, extra })
}

fn run_affected(args: &AffectedArgs) -> Result<ExitCode, String> {
    // Anchor on the workspace root rather than cwd so invoking from a subdir works.
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask manifest directory has no parent")?;

    let packages = crate_packages(root)?;
    let changed = changed_files(root, args.base.as_deref())?;
    let refs: Vec<&str> = changed.iter().map(String::as_str).collect();

    let Some(filter) = filter_for(&refs, &packages) else {
        println!("nothing changed; nothing to run");
        return Ok(ExitCode::SUCCESS);
    };

    if args.print {
        println!("{}", format_command(&filter, &args.extra));
        return Ok(ExitCode::SUCCESS);
    }

    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root)
        .args([
            "nextest",
            "run",
            "--workspace",
            "--all-features",
            "--locked",
            "-E",
            &filter,
        ])
        .args(&args.extra);

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cargo nextest: {e}"))?;
    Ok(exit_code_from(status.code()))
}

fn exit_code_from(code: Option<i32>) -> ExitCode {
    match code {
        Some(0) => ExitCode::SUCCESS,
        Some(n) if (1..=255).contains(&n) => ExitCode::from(n as u8),
        _ => ExitCode::FAILURE,
    }
}

fn format_command(filter: &str, extra: &[String]) -> String {
    let mut parts = vec![
        "cargo".to_string(),
        "nextest".to_string(),
        "run".to_string(),
        "--workspace".to_string(),
        "--all-features".to_string(),
        "--locked".to_string(),
        "-E".to_string(),
        format!("'{filter}'"),
    ];
    parts.extend(extra.iter().cloned());
    parts.join(" ")
}

/// Reads `crates/<dir>/Cargo.toml` for each workspace crate directory to map
/// `<dir>` -> `[package].name` without assuming directory and package names match.
fn crate_packages(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    let crates_dir = root.join("crates");
    let entries = fs::read_dir(&crates_dir)
        .map_err(|e| format!("cannot read {}: {e}", crates_dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("reading crates dir: {e}"))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("Cargo.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let dir = entry
            .file_name()
            .into_string()
            .map_err(|_| "non-UTF8 directory under crates/".to_string())?;
        let manifest = fs::read_to_string(&manifest_path)
            .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
        let name = package_name(&manifest)
            .ok_or_else(|| format!("missing [package].name in {}", manifest_path.display()))?;
        map.insert(dir, name);
    }
    Ok(map)
}

/// Extracts `name` from the `[package]` table of a `Cargo.toml`.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for raw in manifest.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_package = line[1..line.len() - 1].trim() == "package";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some((key, val)) = line.split_once('=')
            && key.trim() == "name"
        {
            let trimmed = val.trim();
            if let Some(unquoted) = trimmed
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| {
                    trimmed
                        .strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                })
            {
                return Some(unquoted.to_string());
            }
        }
    }
    None
}

/// Collects every changed path relative to the workspace root (committed since
/// the merge base, staged, unstaged, and untracked).
fn changed_files(root: &Path, base: Option<&str>) -> Result<Vec<String>, String> {
    let merge_base = resolve_merge_base(root, base)?;
    let range = format!("{merge_base}..HEAD");

    let mut files = BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "--no-renames", &range],
        vec!["diff", "--name-only", "--no-renames", "--cached"],
        vec!["diff", "--name-only", "--no-renames"],
        vec!["ls-files", "--others", "--exclude-standard"],
    ] {
        let out = run_git(root, &args)?;
        for line in out.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                files.insert(trimmed.replace('\\', "/"));
            }
        }
    }
    Ok(files.into_iter().collect())
}

fn resolve_merge_base(root: &Path, explicit: Option<&str>) -> Result<String, String> {
    if let Some(rev) = explicit {
        return git_merge_base(root, rev);
    }
    git_merge_base(root, "origin/dev").or_else(|_| git_merge_base(root, "dev"))
}

fn git_merge_base(root: &Path, rev: &str) -> Result<String, String> {
    let out = run_git(root, &["merge-base", "HEAD", rev])?;
    let sha = out.trim();
    if sha.is_empty() {
        return Err(format!("empty merge-base for {rev}"));
    }
    Ok(sha.to_string())
}

fn run_git<S: AsRef<OsStr>>(root: &Path, args: &[S]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["-c", "core.quotepath=false"])
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git failed: {}", stderr.trim()));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("non-UTF8 git output: {e}"))
}

/// Decides the nextest `-E` filter expression for a list of repository-relative paths.
/// Returns `None` when nothing changed.
fn filter_for(paths: &[&str], packages: &BTreeMap<String, String>) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    let mut affected = BTreeSet::new();
    for &path in paths {
        if BUILD_WIDE_FILES.contains(&path)
            || BUILD_WIDE_PREFIXES.iter().any(|p| path.starts_with(p))
        {
            return Some("all()".to_string());
        }
        // A test in snob-cli reads the README (the monitor's payload it
        // publishes).
        if path == "README.md"
            && let Some(cli) = packages.get("snob-cli")
        {
            affected.insert(cli.as_str());
            continue;
        }
        if let Some(rest) = path.strip_prefix("crates/") {
            let Some((dir, _)) = rest.split_once('/') else {
                // Top-level files inside crates/ do not belong to a known crate.
                return Some("all()".to_string());
            };
            let Some(pkg) = packages.get(dir) else {
                // Unknown or removed crate directory: fall back safely.
                return Some("all()".to_string());
            };
            affected.insert(pkg.as_str());
        }
    }

    if affected.is_empty() {
        return Some(GUARDS_FILTER.to_string());
    }

    let mut terms: Vec<String> = affected
        .into_iter()
        .map(|p| format!("rdeps({p})"))
        .collect();
    terms.push(format!("({GUARDS_FILTER})"));
    Some(terms.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_packages() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("snob-core".into(), "snob-core".into()),
            ("snob-ig".into(), "snob-ig".into()),
            ("snob-cli".into(), "snob-cli".into()),
            ("dir-alias".into(), "renamed-pkg".into()),
        ])
    }

    #[test]
    fn no_changed_files_returns_no_filter() {
        assert_eq!(filter_for(&[], &sample_packages()), None);
    }

    #[test]
    fn build_wide_files_select_the_whole_workspace() {
        let pkgs = sample_packages();
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "deny.toml",
            ".config/nextest.toml",
            ".cargo/config.toml",
            "xtask/src/main.rs",
        ] {
            assert_eq!(filter_for(&[path], &pkgs).as_deref(), Some("all()"));
        }
    }

    #[test]
    fn crate_changes_select_reverse_dependencies_and_source_guards() {
        let pkgs = sample_packages();
        assert_eq!(
            filter_for(&["crates/snob-ig/src/lib.rs"], &pkgs).as_deref(),
            Some("rdeps(snob-ig) | (package(snob-core) & kind(test))")
        );
    }

    #[test]
    fn directory_maps_to_declared_package_name() {
        let pkgs = sample_packages();
        assert_eq!(
            filter_for(&["crates/dir-alias/src/lib.rs"], &pkgs).as_deref(),
            Some("rdeps(renamed-pkg) | (package(snob-core) & kind(test))")
        );
    }

    #[test]
    fn unknown_crate_directory_falls_back_to_all() {
        let pkgs = sample_packages();
        assert_eq!(
            filter_for(&["crates/deleted-crate/src/lib.rs"], &pkgs).as_deref(),
            Some("all()")
        );
    }

    #[test]
    fn docs_and_non_crate_files_run_only_source_reading_guards() {
        let pkgs = sample_packages();
        assert_eq!(
            filter_for(
                &["AGENTS.md", ".github/workflows/ci.yml", "web/index.html"],
                &pkgs
            )
            .as_deref(),
            Some("package(snob-core) & kind(test)")
        );
    }

    #[test]
    fn mixed_crate_and_doc_changes_deduplicate_rdeps_and_keep_guards() {
        let pkgs = sample_packages();
        assert_eq!(
            filter_for(
                &[
                    "crates/snob-cli/src/main.rs",
                    "crates/snob-cli/README.md",
                    "crates/snob-ig/src/pace.rs",
                    "AGENTS.md",
                ],
                &pkgs,
            )
            .as_deref(),
            Some("rdeps(snob-cli) | rdeps(snob-ig) | (package(snob-core) & kind(test))")
        );
    }

    #[test]
    fn the_readme_reaches_the_test_that_reads_it() {
        let pkgs = sample_packages();
        assert_eq!(
            filter_for(&["README.md"], &pkgs).as_deref(),
            Some("rdeps(snob-cli) | (package(snob-core) & kind(test))")
        );
    }

    #[test]
    fn package_name_reads_only_the_package_table() {
        let manifest = r#"
[workspace]
name = "wrong"

[package] # comment
# name = "ignored"
version = "0.1.0"
name = "snob-example" # inline

[dependencies]
name = "also-wrong"
"#;
        assert_eq!(package_name(manifest).as_deref(), Some("snob-example"));
    }

    #[test]
    fn args_parser_handles_base_print_and_extra_nextest_args() {
        let raw: Vec<String> = [
            "affected",
            "--base",
            "0144733",
            "--print",
            "--",
            "--profile",
            "agent",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let parsed = parse_args(&raw[1..]).unwrap();
        assert_eq!(parsed.base.as_deref(), Some("0144733"));
        assert!(parsed.print);
        assert_eq!(parsed.extra, ["--profile", "agent"]);
    }
}
