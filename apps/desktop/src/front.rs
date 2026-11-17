//! What the app hands the engine (`snob_engine::host`): where its notices go,
//! and the binary it starts for its own processes. **The engine asks nobody
//! and draws nothing itself**; a run's progress and its consent question are
//! handed in per run (`host::Front`), which no view does yet.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use snob_core::model::printable;
use snob_engine::host::Notices;

/// The engine's notices, kept for a view to show. Newest last.
#[derive(Clone, Default)]
pub struct Lines(Arc<Mutex<Vec<String>>>);

/// The most notices kept: a window left open for days holds no more.
const KEPT: usize = 200;

impl Lines {
    pub fn all(&self) -> Vec<String> {
        self.lock().clone()
    }

    fn push(&self, line: String) {
        let mut lines = self.lock();
        if lines.len() == KEPT {
            lines.remove(0);
        }
        lines.push(line);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Notices for Lines {
    fn info(&self, line: &str) {
        self.push(printable(line));
    }

    fn warn(&self, line: &str) {
        self.push(format!("warning: {}", printable(line)));
    }
}

/// The `snob` that ships beside this app, when it is there.
///
/// **The engine starts a binary for the owner of the browsers, a job and an
/// update, and that binary is the command line, never this app**
/// (`snob_engine::system::exe::start_as`): the app has none of its hidden commands.
pub fn snob_beside_the_app() -> Option<PathBuf> {
    let name = if cfg!(windows) { "snob.exe" } else { "snob" };
    let beside = std::env::current_exe().ok()?.parent()?.join(name);
    beside.is_file().then_some(beside)
}

/// Points the engine at what the app hands it, once, before anything runs.
/// Returns the notices and whether a `snob` to start was found.
pub fn hand_over() -> (Lines, bool) {
    let lines = Lines::default();
    snob_engine::host::tell_notices_to(Box::new(lines.clone()));
    let found = match snob_beside_the_app() {
        Some(program) => {
            snob_engine::system::exe::start_as(program);
            true
        }
        None => false,
    };
    (lines, found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_is_kept_without_what_a_view_would_act_on() {
        let lines = Lines::default();
        lines.info("read\u{1b}[31m 12 accounts");
        lines.warn("slow\u{202e} answers");
        assert_eq!(
            lines.all(),
            ["read[31m 12 accounts", "warning: slow answers"]
        );
    }

    #[test]
    fn only_the_newest_notices_are_kept() {
        let lines = Lines::default();
        for n in 0..KEPT + 5 {
            lines.info(&n.to_string());
        }
        let kept = lines.all();
        assert_eq!(kept.len(), KEPT);
        assert_eq!(kept.first().map(String::as_str), Some("5"));
    }
}
