//! Which edition of snob an install is: the desktop app with the command
//! line and the MCP server beside it, or the command line and the server
//! alone, as a server or a container wants them.
//!
//! **An edition is what is beside the binary, never a setting**: the app's
//! file in `snob`'s own directory is the desktop edition, and nothing else
//! is asked.
//!
//! **Both binaries of the desktop edition are one version**: they come as
//! one archive and are put in place together (`apply::replace_all`). The
//! app links the engine and starts the `snob` beside it for the owner of
//! the browsers and for jobs, so one of them alone is never updated.

use std::path::{Path, PathBuf};

use super::version::Version;

/// What a release holds for one machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    /// The app, the command line and the MCP server.
    Desktop,
    /// The command line and the MCP server.
    Cli,
}

/// The app's file name, beside `snob`.
pub const APP: &str = if cfg!(windows) {
    "snob-desktop.exe"
} else {
    "snob-desktop"
};

/// The release target of the desktop edition here. glibc on Linux, which
/// the app's toolkit needs; the `snob` inside is the static one all the
/// same. None on macOS, where the app waits for a bundle.
pub(crate) const APP_TARGET: Option<&str> = if cfg!(all(windows, target_arch = "x86_64")) {
    Some("x86_64-pc-windows-msvc")
} else if cfg!(all(windows, target_arch = "aarch64")) {
    Some("aarch64-pc-windows-msvc")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("x86_64-unknown-linux-gnu")
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    Some("aarch64-unknown-linux-gnu")
} else {
    None
};

impl Edition {
    /// The edition of the install whose `snob` is at `exe` (its real path).
    pub fn of(exe: &Path) -> Self {
        let regular =
            std::fs::symlink_metadata(app_beside(exe)).is_ok_and(|meta| meta.file_type().is_file());
        if regular {
            Edition::Desktop
        } else {
            Edition::Cli
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Edition::Desktop => "desktop",
            Edition::Cli => "cli",
        }
    }

    /// The directory this edition's archive of `version` holds, and the
    /// archive's name.
    pub(crate) fn archive(self, version: Version) -> anyhow::Result<(String, String)> {
        let top = match self {
            Edition::Cli => super::TARGET.map(|target| format!("snob-v{version}-{target}")),
            Edition::Desktop => {
                APP_TARGET.map(|target| format!("snob-desktop-v{version}-{target}"))
            }
        }
        .ok_or_else(|| match self {
            Edition::Cli => anyhow::anyhow!("snob has no release for this machine"),
            Edition::Desktop => anyhow::anyhow!("snob has no release of the app for this machine"),
        })?;
        let name = format!("{top}.{}", if cfg!(windows) { "zip" } else { "tar.gz" });
        Ok((top, name))
    }
}

/// Where the app is when the `snob` at `exe` is the desktop edition's.
pub(crate) fn app_beside(exe: &Path) -> PathBuf {
    exe.with_file_name(APP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_beside_the_binary_is_the_desktop_edition() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join(super::super::apply::EXE);
        std::fs::write(&exe, "snob").unwrap();
        assert_eq!(Edition::of(&exe), Edition::Cli);

        // A directory of that name is no app.
        std::fs::create_dir(tmp.path().join(APP)).unwrap();
        assert_eq!(Edition::of(&exe), Edition::Cli);
        std::fs::remove_dir(tmp.path().join(APP)).unwrap();

        std::fs::write(tmp.path().join(APP), "app").unwrap();
        assert_eq!(Edition::of(&exe), Edition::Desktop);
    }

    /// The command line's archive keeps the name every released snob looks
    /// for; the desktop edition's can never be taken for it.
    #[test]
    fn each_edition_has_an_archive_of_its_own() {
        let version = Version::parse("9.9.9").unwrap();
        if let Ok((top, name)) = Edition::Cli.archive(version) {
            assert!(top.starts_with("snob-v9.9.9-"), "{top}");
            assert!(name.starts_with(&top), "{name}");
        }
        if let Ok((top, name)) = Edition::Desktop.archive(version) {
            assert!(top.starts_with("snob-desktop-v9.9.9-"), "{top}");
            assert!(!top.contains("musl"), "{top}");
            assert!(name.starts_with(&top), "{name}");
        }
    }
}
