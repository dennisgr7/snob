//! The path to the running binary that a configuration should name.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

/// Set by [`start_as`].
static PROGRAM: OnceLock<PathBuf> = OnceLock::new();

/// Makes `program` the binary the engine starts for its own processes (the
/// owner of the browsers, a job's supervisor and walk, the updater, a tool
/// call's child) and names in a configuration. Set once, before any is
/// started; unset, it is the running binary.
pub fn start_as(program: PathBuf) {
    let _ = PROGRAM.set(program);
}

/// The binary the engine starts for its own processes ([`start_as`]).
pub fn program() -> std::io::Result<PathBuf> {
    match PROGRAM.get() {
        Some(program) => Ok(program.clone()),
        None => std::env::current_exe(),
    }
}

/// The stable path to this binary, warning with `consequence` when the best
/// there is sits in a directory named after this version.
pub(crate) fn to_print(consequence: &str) -> std::io::Result<PathBuf> {
    let current = program()?;
    let path_var = std::env::var_os("PATH");
    let exe = stable_exe(
        &current,
        |path| std::fs::canonicalize(path).ok(),
        path_var.as_deref(),
    );
    if names_a_version(&exe) {
        crate::host::warn(&format!(
            "{} is in a directory named after this version of snob, so {consequence}",
            exe.display()
        ));
    }
    Ok(exe)
}

/// The path to print for the running binary `current`: one that leads to the
/// same file today and to the upgraded one after an upgrade, when the installer
/// keeps such a path.
///
/// **A candidate is taken only when it leads to the running binary itself**, so
/// a guess about a layout that turns out wrong (another prefix, a link to
/// another version, a shim that is a program and not a link) is never printed:
/// the next rule is tried, and in the end `current` is printed as it is.
pub(crate) fn stable_exe(
    current: &Path,
    lookup: impl Fn(&Path) -> Option<PathBuf>,
    path_var: Option<&OsStr>,
) -> PathBuf {
    let Some(real) = lookup(current) else {
        return current.to_path_buf();
    };
    let Some(file) = real.file_name() else {
        return current.to_path_buf();
    };
    let leads_here = |candidate: &Path| lookup(candidate).is_some_and(|there| there == real);

    let names: Vec<&OsStr> = real
        .components()
        .map(|component| component.as_os_str())
        .collect();
    let mut candidates: Vec<PathBuf> = Vec::new();
    // `<prefix>/Cellar/snob/<version>/bin/snob`, its last five names.
    if let [.., cellar, snob, _version, bin, _file] = names.as_slice()
        && *cellar == "Cellar"
        && *snob == "snob"
        && *bin == "bin"
        && let Some(prefix) = ancestor(&real, 5)
    {
        candidates.push(prefix.join("bin").join(file));
        candidates.push(prefix.join("opt").join("snob").join("bin").join(file));
    }
    // `<root>/apps/snob/<version>/snob.exe`, its last four names.
    if let [.., apps, snob, _version, _file] = names.as_slice()
        && *apps == "apps"
        && *snob == "snob"
        && let Some(app) = ancestor(&real, 2)
    {
        candidates.push(app.join("current").join(file));
    }
    if let Some(found) = candidates
        .into_iter()
        .map(without_verbatim)
        .find(|candidate| leads_here(candidate))
    {
        return found;
    }

    let typed = current.file_name().unwrap_or(file);
    path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(typed))
        .find(|candidate| leads_here(candidate))
        .unwrap_or_else(|| current.to_path_buf())
}

/// `path` with its last `levels` names taken off.
fn ancestor(path: &Path, levels: usize) -> Option<&Path> {
    (0..levels).try_fold(path, |path, _| path.parent())
}

/// A path without the `\\?\` that `canonicalize` puts in front of one on
/// Windows, when what follows is a drive.
fn without_verbatim(path: PathBuf) -> PathBuf {
    let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) else {
        return path;
    };
    let bytes = rest.as_bytes();
    if bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':') {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// Whether a directory on the way to `path` is named after a version
/// (`0.7.0`, `0.7.0_1`, `snob-v0.7.0-x86_64-pc-windows-msvc`): three numbers
/// with dots between them, which is how snob's versions are written.
pub(crate) fn names_a_version(path: &Path) -> bool {
    let versioned = |name: &OsStr| {
        name.to_string_lossy()
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .any(|run| {
                let parts: Vec<&str> = run.trim_matches('.').split('.').collect();
                parts.len() >= 3 && parts.iter().all(|part| !part.is_empty())
            })
    };
    path.parent().is_some_and(|dirs| {
        dirs.components().any(|component| match component {
            Component::Normal(name) => versioned(name),
            _ => false,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absolute path on the platform the test runs on, written with
    /// forward slashes, which both read as separators.
    fn at(rest: &str) -> PathBuf {
        PathBuf::from(if cfg!(windows) { "C:/" } else { "/" }).join(rest)
    }

    /// A file system of links: each path and where it really leads. A path
    /// not listed leads nowhere.
    fn links(table: &[(PathBuf, PathBuf)]) -> impl Fn(&Path) -> Option<PathBuf> + '_ {
        move |path| {
            table
                .iter()
                .find(|(from, _)| from == path)
                .map(|(_, to)| to.clone())
        }
    }

    fn path_of(dirs: &[PathBuf]) -> std::ffi::OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn a_homebrew_cellar_path_is_printed_as_the_linked_one() {
        let real = at("opt/homebrew/Cellar/snob/0.7.0/bin/snob");
        let table = [
            (real.clone(), real.clone()),
            (at("opt/homebrew/bin/snob"), real.clone()),
            (at("opt/homebrew/opt/snob/bin/snob"), real.clone()),
        ];
        let printed = stable_exe(&real, links(&table), None);
        assert_eq!(printed, at("opt/homebrew/bin/snob"));
        assert!(names_a_version(&real));
        assert!(!names_a_version(&printed));

        // macOS may report the link it was started by: the same answer.
        let printed = stable_exe(&at("opt/homebrew/bin/snob"), links(&table), None);
        assert_eq!(printed, at("opt/homebrew/bin/snob"));

        // Without the link in bin, the one in opt.
        let printed = stable_exe(&real, links(&table[..1]), None);
        assert_eq!(
            printed, real,
            "nothing links to it, so it is printed as it is"
        );
        let table = [table[0].clone(), table[2].clone()];
        let printed = stable_exe(&real, links(&table), None);
        assert_eq!(printed, at("opt/homebrew/opt/snob/bin/snob"));
    }

    /// While an older version is still running, the links lead to the newer
    /// one, and printing them would describe a binary other than this one.
    #[test]
    fn a_link_to_another_version_is_not_printed() {
        let real = at("home/linuxbrew/.linuxbrew/Cellar/snob/0.7.0/bin/snob");
        let newer = at("home/linuxbrew/.linuxbrew/Cellar/snob/0.8.0/bin/snob");
        let table = [
            (real.clone(), real.clone()),
            (at("home/linuxbrew/.linuxbrew/bin/snob"), newer.clone()),
            (at("home/linuxbrew/.linuxbrew/opt/snob/bin/snob"), newer),
        ];
        assert_eq!(stable_exe(&real, links(&table), None), real);
    }

    #[test]
    fn a_scoop_version_directory_is_printed_as_current() {
        let real = at("Users/someone/scoop/apps/snob/0.7.0/snob.exe");
        let current = at("Users/someone/scoop/apps/snob/current/snob.exe");
        let shims = at("Users/someone/scoop/shims");
        let table = [
            (real.clone(), real.clone()),
            (current.clone(), real.clone()),
            // The shim is a program that starts the binary, not a link to it.
            (shims.join("snob.exe"), shims.join("snob.exe")),
        ];
        let path = path_of(std::slice::from_ref(&shims));
        assert_eq!(stable_exe(&real, links(&table), Some(&path)), current);
        // Started through the shim, the binary reports the junction's path.
        assert_eq!(stable_exe(&current, links(&table), Some(&path)), current);
    }

    #[test]
    fn the_first_snob_on_the_path_that_leads_here_is_printed() {
        let real = at(
            "Users/someone/AppData/Local/Microsoft/WinGet/Packages/dennisgr7.snob_x/\
                       snob-v0.7.0-x86_64-pc-windows-msvc/snob.exe",
        );
        let elsewhere = at("Tools");
        let winget = at("Users/someone/AppData/Local/Microsoft/WinGet/Links");
        let table = [
            (real.clone(), real.clone()),
            // A relative entry, which a client would read from its own
            // directory: skipped even though it leads here today.
            (PathBuf::from("bin").join("snob.exe"), real.clone()),
            (elsewhere.join("snob.exe"), elsewhere.join("snob.exe")),
            (winget.join("snob.exe"), real.clone()),
        ];
        let path = path_of(&[PathBuf::from("bin"), elsewhere, winget.clone()]);
        let printed = stable_exe(&real, links(&table), Some(&path));
        assert_eq!(printed, winget.join("snob.exe"));
        assert!(names_a_version(&real));
        assert!(!names_a_version(&printed));
    }

    #[test]
    fn a_path_nothing_links_to_is_printed_as_it_is() {
        let real = at("dev/snob-cli/target/release/snob");
        let other = at("usr/bin");
        let table = [
            (real.clone(), real.clone()),
            (other.join("snob"), other.join("snob")),
        ];
        let path = path_of(&[other]);
        assert_eq!(stable_exe(&real, links(&table), Some(&path)), real);
        assert!(!names_a_version(&real));
        // And one that leads nowhere at all, which no rule can judge.
        let gone = at("tmp/snob");
        assert_eq!(stable_exe(&gone, links(&table), Some(&path)), gone);
    }

    #[test]
    fn a_drive_path_loses_the_prefix_canonicalize_gives_it() {
        assert_eq!(
            without_verbatim(PathBuf::from(r"\\?\C:\scoop\apps\snob\current\snob.exe")),
            PathBuf::from(r"C:\scoop\apps\snob\current\snob.exe")
        );
        let share = PathBuf::from(r"\\?\UNC\server\share\snob.exe");
        assert_eq!(without_verbatim(share.clone()), share);
    }
}
