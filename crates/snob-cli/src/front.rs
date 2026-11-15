//! What the command line hands the engine (`host`): its bars, its prompt on
//! the terminal, its notices on standard error, and its terminal put back
//! before a forced exit.

use std::sync::Arc;
use std::time::Duration;

use snob_core::Epoch;
use snob_core::model::{ListKind, StopReason};
use snob_engine::host::{self, Front, Wait};
use snob_ig::pager::Event;

use crate::progress::Progress;

/// A run's front: the bars (`with_progress` says whether they draw), and a
/// question on the terminal with the bars out of its way.
pub fn front(with_progress: bool) -> Front {
    let progress: Arc<dyn host::Progress> = Arc::new(Progress::new(with_progress));
    Front {
        ask: Arc::new(Terminal {
            progress: Arc::clone(&progress),
        }),
        progress,
    }
}

/// Hands the engine this process's notices and its terminal's restoring.
pub fn install() {
    host::tell_notices_to(Box::new(Lines));
    snob_engine::interrupt::before_a_forced_exit(crate::ui::restore_terminal);
}

impl host::Progress for Progress {
    fn event(&self, event: &Event) {
        Progress::event(self, event);
    }
    fn warn(&self, text: &str) {
        Progress::warn(self, text);
    }
    fn waiting(&self, wait: Wait, duration: Duration) {
        let reason = match wait {
            Wait::Budget => "the request budget is rationing",
            Wait::LinkToOpen => "waiting for the link to be opened",
            Wait::LogIn => "waiting for you to log in",
        };
        Progress::waiting(self, reason, duration);
    }
    fn begin(&self, subject: &str) {
        Progress::begin(self, subject);
    }
    fn walking(&self, list: ListKind) {
        Progress::walking(self, list);
    }
    fn served(
        &self,
        list: ListKind,
        from: &str,
        taken_at: Epoch,
        cooldown_until: Option<Epoch>,
        reason: StopReason,
        own_changes: usize,
    ) {
        Progress::served(
            self,
            list,
            from,
            taken_at,
            cooldown_until,
            reason,
            own_changes,
        );
    }
    fn finish(&self) {
        Progress::finish(self);
    }
    fn paused(&self, f: &mut dyn FnMut()) {
        self.while_paused(f);
    }
}

/// The question on the terminal, read from standard input.
struct Terminal {
    progress: Arc<dyn host::Progress>,
}

impl host::Ask for Terminal {
    fn can_ask(&self) -> bool {
        crate::ui::can_be_asked()
    }
    fn confirm(&self, question: &str) -> anyhow::Result<bool> {
        self.progress
            .while_paused(|| crate::ui::confirm(question, false))
    }
}

/// Notices on standard error.
struct Lines;

impl host::Notices for Lines {
    fn info(&self, line: &str) {
        crate::ui::info(line);
    }
    fn warn(&self, line: &str) {
        crate::ui::warn(line);
    }
}
