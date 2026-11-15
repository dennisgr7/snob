//! Every test and integration binary named in `.config/nextest.toml` exists.
//!
//! A rename would silently drop a test from `keyring` or `agent`, so this
//! checks every `test(=name)` and `binary(name)` in `.config/nextest.toml`
//! against `crates/`. **Only the last segment of a name is checked**: a test
//! moved to another module under the same name still passes here, and drops
//! out of its group.

mod common;
use common::{relative, repo_root, source_files};
use std::collections::BTreeSet;

fn missing(toml: &str, fns: &BTreeSet<String>, bins: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for line in toml.lines() {
        let code = line.split('#').next().unwrap_or("");
        for (pat, kind, known) in [("test(=", "test", fns), ("binary(", "binary", bins)] {
            let mut rest = code;
            while let Some(at) = rest.find(pat) {
                let tail = &rest[at + pat.len()..];
                let Some(end) = tail.find(')') else {
                    break;
                };
                let name = tail[..end].trim().rsplit("::").next().unwrap_or("");
                if !known.contains(name) {
                    out.push(format!("{kind} {name}"));
                }
                rest = &tail[end + 1..];
            }
        }
    }
    out.sort();
    out
}

#[test]
fn every_test_and_binary_named_in_nextest_exists() {
    let Some(root) = repo_root() else {
        return; // packaged build, nothing to read
    };
    let toml = std::fs::read_to_string(root.join(".config/nextest.toml")).unwrap();
    let (mut fns, mut bins) = (BTreeSet::new(), BTreeSet::new());
    for file in source_files(&root, &["rs"]) {
        let rel = relative(&root, &file);
        if !rel.starts_with("crates/") {
            continue;
        }
        if let Some((_, tail)) = rel.split_once("/tests/")
            && let Some(stem) = tail.strip_suffix(".rs")
            && !stem.contains('/')
        {
            bins.insert(stem.to_string());
        }
        if let Ok(src) = std::fs::read_to_string(&file) {
            for after in src.split("fn ").skip(1) {
                let name = after.split('(').next().unwrap_or("").trim();
                let is_ident =
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if is_ident {
                    fns.insert(name.to_string());
                }
            }
        }
    }
    let bad = missing(&toml, &fns, &bins);
    assert!(bad.is_empty(), "stale in nextest.toml:\n{}", bad.join("\n"));
}

#[test]
fn a_renamed_test_or_binary_in_nextest_is_caught() {
    let fns = BTreeSet::from(["kept".into()]);
    let bins = BTreeSet::from(["sandbox".into()]);
    let toml =
        "# test(=ignored)\nbinary(sandbox) | binary(gone) | test(=a::kept) | test(=a::moved)";
    assert_eq!(missing(toml, &fns, &bins), ["binary gone", "test moved"]);
}
