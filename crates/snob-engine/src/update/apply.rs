//! Unpacking a release and putting it in place.
//!
//! **The archive is unpacked by the system's own `tar`**, as `install.sh` does:
//! every Linux and macOS has one, and Windows 10 and 11 ship bsdtar as
//! `System32\tar.exe`, which reads a `.zip`.
//!
//! **Replacing a binary never leaves half of one**, and the desktop
//! edition's two are written whole before either is swapped
//! ([`replace_all`]).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::how::Step;

/// The binary's name in a release archive.
pub(crate) const EXE: &str = if cfg!(windows) { "snob.exe" } else { "snob" };

/// The system's `tar`.
fn tar() -> PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        PathBuf::from(root).join("System32").join("tar.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("tar")
    }
}

/// Unpacks `archive` into `into`, made empty first, and returns the
/// directory it holds, `top` (`snob-v<version>-<target>`), once `snob` is
/// seen to be in it.
pub(crate) fn unpack(archive: &Path, into: &Path, top: &str) -> anyhow::Result<PathBuf> {
    if into.exists() {
        std::fs::remove_dir_all(into)?;
    }
    std::fs::create_dir_all(into)?;
    let mut command = Command::new(tar());
    command
        .arg(if cfg!(windows) { "-xf" } else { "-xzf" })
        .arg(archive)
        .arg("-C")
        .arg(into)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let out = command.output()?;
    anyhow::ensure!(
        out.status.success(),
        "tar could not unpack {}: {}",
        archive.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let held = into.join(top);
    file_in(&held, EXE)?;
    Ok(held)
}

/// The file `name` in the unpacked directory `held`, refused when it is
/// anything but a regular file.
pub(crate) fn file_in(held: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let file = held.join(name);
    let regular = std::fs::symlink_metadata(&file).is_ok_and(|meta| meta.file_type().is_file());
    anyhow::ensure!(
        regular,
        "the archive holds no {name} under {}",
        held.file_name().unwrap_or_default().to_string_lossy()
    );
    Ok(file)
}

/// One binary to put in place: where it runs from (its real path, every
/// link followed) and the new file.
pub(crate) struct Swap {
    pub exe: PathBuf,
    pub new: PathBuf,
}

/// Where a new binary is written whole before it takes `exe`'s name.
fn written_beside(exe: &Path) -> PathBuf {
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    if cfg!(windows) {
        exe.with_file_name(format!("{name}.new"))
    } else {
        exe.with_file_name(format!(".{name}.update-{}", std::process::id()))
    }
}

/// Writes `swap`'s new binary whole beside the one it replaces.
fn write_beside(swap: &Swap) -> std::io::Result<()> {
    let beside = written_beside(&swap.exe);
    #[cfg(windows)]
    {
        // An `.old` left by an earlier update, which a snob still running
        // from it keeps; then this one cannot be renamed there either, and
        // waits.
        let old = old_of(&swap.exe);
        if old.exists() {
            std::fs::remove_file(&old)?;
        }
    }
    std::fs::copy(&swap.new, &beside)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o755))?;
    }
    // `FlushFileBuffers` wants a handle opened for writing.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&beside)?
        .sync_all()
}

/// Gives the binary written beside `exe` its name.
#[cfg(unix)]
fn swap_in(exe: &Path) -> std::io::Result<()> {
    std::fs::rename(written_beside(exe), exe)
}

/// Gives the binary written beside `exe` its name: a running `.exe` cannot
/// be written over, and can be renamed.
#[cfg(windows)]
fn swap_in(exe: &Path) -> std::io::Result<()> {
    let old = old_of(exe);
    std::fs::rename(exe, &old)?;
    std::fs::rename(written_beside(exe), exe).inspect_err(|_| {
        let _ = std::fs::rename(&old, exe);
    })
}

/// Puts every new binary in place of the one it replaces.
///
/// **All are written whole beside theirs before any is swapped**: a copy cut
/// short (a kill, the power, a full disk) leaves every old binary where it
/// was, never half of a new one under its name and never one binary of two
/// updated. The swaps are then renames, in the order given.
pub(crate) fn replace_all(swaps: &[Swap]) -> std::io::Result<()> {
    let forget = || {
        for swap in swaps {
            let _ = std::fs::remove_file(written_beside(&swap.exe));
        }
    };
    if let Err(e) = swaps.iter().try_for_each(write_beside) {
        forget();
        return Err(e);
    }
    let swapped = swaps.iter().try_for_each(|swap| swap_in(&swap.exe));
    if swapped.is_err() {
        forget();
    }
    swapped
}

/// The `.old` a replacement on Windows leaves beside `exe`.
#[cfg(windows)]
fn old_of(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Removes the `.old` an update on Windows left, once nothing runs from it;
/// one still running keeps it until a later start.
pub(crate) fn sweep_old(exe: &Path) {
    #[cfg(windows)]
    {
        let old = old_of(exe);
        if old.exists() {
            let _ = std::fs::remove_file(old);
        }
    }
    #[cfg(not(windows))]
    let _ = exe;
}

/// Runs this command again from the binary now at `exe`, with the same
/// arguments, environment and streams: what was typed runs on the version
/// just applied. Returns only when that fails.
#[cfg(unix)]
pub(crate) fn run_again(exe: &Path) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    Command::new(exe).args(std::env::args_os().skip(1)).exec()
}

/// Where a step's output goes.
pub enum Output<'a> {
    /// Appended to the log, for a step nobody watches.
    Log(&'a Path),
    /// Onto standard error, for one a person watches (`snob update`):
    /// standard output carries data only, and a manager's chatter is not.
    Shown,
}

/// Runs `steps` in order, stopping at the first that fails.
pub fn run_steps(steps: &[Step], output: &Output<'_>) -> anyhow::Result<()> {
    for step in steps {
        let mut command = Command::new(&step.program);
        command.args(&step.args).stdin(Stdio::null());
        for (name, value) in &step.env {
            command.env(name, value);
        }
        match output {
            Output::Log(log) => {
                let file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log)?;
                use std::io::Write;
                writeln!(&file, "$ {}", shown(step))?;
                command.stdout(file.try_clone()?).stderr(file);
            }
            Output::Shown => {
                command.stdout(std::io::stderr()).stderr(std::io::stderr());
            }
        }
        let status = command
            .status()
            .map_err(|e| anyhow::anyhow!("{} could not run: {e}", step.program.display()))?;
        anyhow::ensure!(status.success(), "\"{}\" failed ({status})", shown(step));
    }
    Ok(())
}

/// A step as it would be typed, for the log and for a failure.
pub(crate) fn shown(step: &Step) -> String {
    std::iter::once(step.program.as_os_str())
        .chain(step.args.iter().map(OsString::as_os_str))
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swap(exe: &Path, new: &Path) -> Swap {
        Swap {
            exe: exe.to_path_buf(),
            new: new.to_path_buf(),
        }
    }

    /// The names in `dir`, sorted.
    fn left_in(dir: &Path) -> Vec<String> {
        let mut left: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        left
    }

    /// A new binary that cannot be copied leaves the old one in place, under
    /// its own name, and nothing staged beside it.
    #[test]
    fn a_replacement_that_cannot_be_made_leaves_the_old_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(EXE);
        std::fs::write(&exe, "old").unwrap();

        assert!(replace_all(&[swap(&exe, &tmp.path().join("missing"))]).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert_eq!(left_in(tmp.path()), [EXE.to_string()]);
    }

    /// The desktop edition's two binaries: when the second cannot be
    /// written, the first is not swapped either, and nothing is left beside
    /// them.
    #[test]
    fn two_binaries_are_replaced_together_or_neither_is() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let (app, exe) = (bin.join("app"), bin.join(EXE));
        let new = tmp.path().join("new");
        std::fs::write(&app, "old app").unwrap();
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();

        let missing = tmp.path().join("missing");
        assert!(replace_all(&[swap(&app, &new), swap(&exe, &missing)]).is_err());
        assert_eq!(std::fs::read_to_string(&app).unwrap(), "old app");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        let mut both = ["app".to_string(), EXE.to_string()];
        both.sort();
        assert_eq!(left_in(&bin), both);

        replace_all(&[swap(&app, &new), swap(&exe, &new)]).unwrap();
        assert_eq!(std::fs::read_to_string(&app).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        sweep_old(&app);
        sweep_old(&exe);
        assert_eq!(left_in(&bin), both);
    }

    /// The swap leaves the new binary at the name and nothing else beside it
    /// (on Windows, the `.old` until a later start sweeps it).
    #[test]
    fn a_replacement_leaves_the_new_binary_and_nothing_half_written() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(EXE);
        let new = tmp.path().join("new");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();

        replace_all(&[swap(&exe, &new)]).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        sweep_old(&exe);
        let mut expected = [EXE.to_string(), "new".to_string()];
        expected.sort();
        assert_eq!(left_in(tmp.path()), expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    /// What a release archive holds is found under its own directory, and an
    /// archive without it is refused.
    #[cfg(unix)]
    #[test]
    fn a_release_archive_is_unpacked_and_its_binary_found() {
        let tmp = tempfile::tempdir().unwrap();
        let top = "snob-v9.9.9-test";
        std::fs::create_dir(tmp.path().join(top)).unwrap();
        std::fs::write(tmp.path().join(top).join(EXE), "binary").unwrap();
        let archive = tmp.path().join("a.tar.gz");
        let made = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(tmp.path())
            .arg(top)
            .status()
            .unwrap();
        assert!(made.success());

        let into = tmp.path().join("x");
        let held = unpack(&archive, &into, top).unwrap();
        let binary = file_in(&held, EXE).unwrap();
        assert_eq!(std::fs::read_to_string(binary).unwrap(), "binary");
        assert!(file_in(&held, "snob-desktop").is_err());
        assert!(unpack(&archive, &into, "snob-v1.0.0-other").is_err());
    }

    #[test]
    fn a_failed_step_stops_the_rest_and_says_which() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("update.log");
        let step = |ok: bool| Step {
            program: if cfg!(windows) {
                "cmd".into()
            } else {
                "sh".into()
            },
            args: if cfg!(windows) {
                vec!["/c".into(), format!("exit {}", u8::from(!ok)).into()]
            } else {
                vec!["-c".into(), format!("exit {}", u8::from(!ok)).into()]
            },
            env: Vec::new(),
        };
        assert!(run_steps(&[step(true)], &Output::Log(&log)).is_ok());
        let failed = run_steps(&[step(false), step(true)], &Output::Log(&log)).unwrap_err();
        assert!(failed.to_string().contains("exit 1"), "{failed}");
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 2);
    }
}
