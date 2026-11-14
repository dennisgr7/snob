//! Unpacking a release and putting it in place.
//!
//! **The archive is unpacked by the system's own `tar`**, as `install.sh` does:
//! every Linux and macOS has one, and Windows 10 and 11 ship bsdtar as
//! `System32\tar.exe`, which reads a `.zip`.
//!
//! **Replacing the binary never leaves half of one.**

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

/// Unpacks `archive` into `into`, made empty first, and returns the binary
/// it holds: `<top>/snob`, `top` being the archive's own directory
/// (`snob-v<version>-<target>`).
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
    let binary = into.join(top).join(EXE);
    let regular = std::fs::symlink_metadata(&binary).is_ok_and(|meta| meta.file_type().is_file());
    anyhow::ensure!(regular, "the archive holds no {EXE} under {top}");
    Ok(binary)
}

/// Puts `new` in place of the binary at `exe` (its real path, every link
/// followed).
#[cfg(unix)]
pub(crate) fn replace(exe: &Path, new: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    let beside = exe.with_file_name(format!(".{name}.update-{}", std::process::id()));
    let placed = (|| {
        std::fs::copy(new, &beside)?;
        std::fs::set_permissions(&beside, std::fs::Permissions::from_mode(0o755))?;
        std::fs::File::open(&beside)?.sync_all()?;
        std::fs::rename(&beside, exe)
    })();
    if placed.is_err() {
        let _ = std::fs::remove_file(&beside);
    }
    placed
}

/// The `.old` a replacement on Windows leaves beside `exe`.
#[cfg(windows)]
fn old_of(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

#[cfg(windows)]
pub(crate) fn replace(exe: &Path, new: &Path) -> std::io::Result<()> {
    let old = old_of(exe);
    // One left by an earlier update, which a snob still running from it
    // keeps; then this one cannot be renamed there either, and waits.
    if old.exists() {
        std::fs::remove_file(&old)?;
    }
    // Written whole beside the binary and renamed in: a copy cut short (a
    // kill, the power) leaves the old binary where it was, never half of the
    // new one under its name.
    let mut staged = exe.file_name().unwrap_or_default().to_os_string();
    staged.push(".new");
    let staged = exe.with_file_name(staged);
    let placed = (|| {
        std::fs::copy(new, &staged)?;
        // `FlushFileBuffers` wants a handle opened for writing.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&staged)?
            .sync_all()?;
        std::fs::rename(exe, &old)?;
        std::fs::rename(&staged, exe).inspect_err(|_| {
            let _ = std::fs::rename(&old, exe);
        })
    })();
    if placed.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    placed
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

    /// A new binary that cannot be copied leaves the old one in place, under
    /// its own name, and nothing staged beside it.
    #[test]
    fn a_replacement_that_cannot_be_made_leaves_the_old_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(EXE);
        std::fs::write(&exe, "old").unwrap();

        assert!(replace(&exe, &tmp.path().join("missing")).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        let left: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, [EXE.to_string()]);
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

        replace(&exe, &new).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        sweep_old(&exe);
        let mut left: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let mut expected = [EXE.to_string(), "new".to_string()];
        expected.sort();
        assert_eq!(left, expected);
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
        let binary = unpack(&archive, &into, top).unwrap();
        assert_eq!(std::fs::read_to_string(binary).unwrap(), "binary");
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
