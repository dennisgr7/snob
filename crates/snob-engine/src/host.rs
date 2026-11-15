//! What the program running the engine hands it: who is told how a walk goes,
//! who is asked before somebody else's lists are read, and where a notice
//! goes. The command line hands its bars, its prompt and standard error; an
//! app hands its own views.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use snob_core::Epoch;
use snob_core::model::{ListKind, StopReason};
use snob_ig::pace::CancelToken;
use snob_ig::pager::Event;

/// What a wait the engine announces is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// The request budget is rationing.
    Budget,
    /// The link of a relayed login has not been opened yet.
    LinkToOpen,
    /// The person is logging in, in the browser.
    LogIn,
}

/// Told how the walks of a run go, as they go.
pub trait Progress: Send + Sync {
    /// A walk's event, as the pager reports it.
    fn event(&self, event: &Event);
    /// A warning for the person, said without disturbing what is drawn.
    fn warn(&self, text: &str);
    /// Nothing happens for `duration`, for `wait`.
    fn waiting(&self, wait: Wait, duration: Duration);
    /// The walk about to start, as a person names it ("@someone followers").
    fn begin(&self, subject: &str);
    /// The list the walk about to start reads.
    fn walking(&self, list: ListKind);
    /// Where a list came from once the engine has answered for it.
    fn served(
        &self,
        list: ListKind,
        from: &str,
        taken_at: Epoch,
        cooldown_until: Option<Epoch>,
        reason: StopReason,
        own_changes: usize,
    );
    /// The run is done reading from Instagram.
    fn finish(&self);
    /// Runs `f` with nothing drawn over what it writes.
    fn paused(&self, f: &mut dyn FnMut());
}

impl dyn Progress + '_ {
    /// [`Progress::paused`], for a closure that returns something.
    pub fn while_paused<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut f = Some(f);
        let mut out = None;
        self.paused(&mut || {
            if let Some(f) = f.take() {
                out = Some(f());
            }
        });
        match out {
            Some(out) => out,
            // A `paused` that never ran its closure: run it here.
            None => f.map(|f| f()).expect("the closure is run once"),
        }
    }
}

/// Asked before somebody else's lists are read.
pub trait Ask: Send + Sync {
    /// Whether somebody is there to answer a question.
    fn can_ask(&self) -> bool;
    /// A yes-or-no question whose answer, unless one is given, is no. It may
    /// block for as long as the person thinks.
    fn confirm(&self, question: &str) -> anyhow::Result<bool>;
}

/// What a run reports to and asks through, and what stops it.
#[derive(Clone)]
pub struct Front {
    pub progress: Arc<dyn Progress>,
    pub ask: Arc<dyn Ask>,
    /// What stops this run and no other: a child of the process's token
    /// (`interrupt::install().child()`), for a program that outlives its
    /// runs. `None` is the process's own, which a command line wants.
    pub cancel: Option<CancelToken>,
}

impl Front {
    /// Nothing drawn, warnings as notices, and nobody to ask.
    pub fn quiet() -> Self {
        Self {
            progress: Arc::new(Unwatched),
            ask: Arc::new(NobodyToAsk),
            cancel: None,
        }
    }
}

/// A [`Progress`] nobody watches: warnings go out as notices ([`warn`]),
/// nothing else is said.
pub struct Unwatched;

impl Progress for Unwatched {
    fn event(&self, _: &Event) {}
    fn warn(&self, text: &str) {
        warn(text);
    }
    fn waiting(&self, _: Wait, _: Duration) {}
    fn begin(&self, _: &str) {}
    fn walking(&self, _: ListKind) {}
    fn served(&self, _: ListKind, _: &str, _: Epoch, _: Option<Epoch>, _: StopReason, _: usize) {}
    fn finish(&self) {}
    fn paused(&self, f: &mut dyn FnMut()) {
        f();
    }
}

/// An [`Ask`] with nobody there.
pub struct NobodyToAsk;

impl Ask for NobodyToAsk {
    fn can_ask(&self) -> bool {
        false
    }
    fn confirm(&self, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// Where the engine's notices go: lines for the person, outside a walk.
pub trait Notices: Send + Sync {
    fn info(&self, line: &str);
    fn warn(&self, line: &str);
}

static NOTICES: OnceLock<Box<dyn Notices>> = OnceLock::new();

/// Sends every notice of this process to `notices`, once; a second call
/// changes nothing.
pub fn tell_notices_to(notices: Box<dyn Notices>) {
    let _ = NOTICES.set(notices);
}

/// A line for the person; on standard error until [`tell_notices_to`] says
/// otherwise.
pub fn info(line: &str) {
    match NOTICES.get() {
        Some(notices) => notices.info(line),
        None => eprintln!("{line}"),
    }
}

/// A warning for the person; on standard error, after `warning:`, until
/// [`tell_notices_to`] says otherwise.
pub fn warn(line: &str) {
    match NOTICES.get() {
        Some(notices) => notices.warn(line),
        None => eprintln!("warning: {line}"),
    }
}
