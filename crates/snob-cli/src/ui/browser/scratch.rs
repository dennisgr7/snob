//! The directory a browsing session opens media from, and its lifecycle.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use crate::files::ABANDONED_AFTER;

/// The scratch directory, removed when the browsing session ends.
///
/// **What it can and cannot promise, measured on Windows 11 in August 2026
/// rather than assumed.**
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(dir: PathBuf) -> Result<Self> {
        // Fresh, not adopted: the name carries the process id, which anybody on
        // the machine can guess ahead of time.
        snob_store::paths::create_fresh_scratch_dir(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        Ok(Self(dir))
    }

    pub fn dir(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Silent by design: a file a greedy viewer is still holding is not an
        // error the user can do anything about, and a warning printed every
        // time somebody looked at a story would train them to ignore warnings.
        //
        // **This does not run on a panic.**
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
