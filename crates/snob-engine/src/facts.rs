//! What a run knows of itself and of a list it hands back, as plain data:
//! who it acts as, until when it is held, how a yes was given ahead, where a
//! list came from. **It stands on nothing of the engine's**, so the words
//! (`sentences`) and the parts under them can name it; `app` and `engine`
//! hand these out under their own names.

use snob_core::{EpochMs, Pk};

/// The account the run is acting as, when there is one. Its methods are
/// `app`'s, which name it in the words' way (`sentences::label`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub pk: Pk,
    /// Absent until the first run resolves it. Cosmetic: nothing blocks on it.
    pub username: Option<String>,
}

/// Until when this account may send nothing, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub until_ms: EpochMs,
    /// The accounts whose push-backs stopped every account, named.
    pub braked: Vec<String>,
}

/// How a run could have been given consent before it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsentInAdvance {
    /// `-y` on the command line.
    #[default]
    Flag,
    /// An `[[account]]` in `watch.toml` carrying the answer somebody gave once,
    /// which is what `snob watch setup` writes.
    WatchConfig,
}

/// Where a returned list came from, in the sense that decides whether two of
/// them may be crossed against each other.
///
/// It is whether anything in **this run** established that it still describes
/// the account: two lists a month apart are fine to cross if both counters were
/// checked just now and neither had moved, and two lists an hour apart are not
/// fine if nobody looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// Walked in this run. It is the account as it is.
    Walked,
    /// Stored, and a counter poll in this run said it had not moved.
    CounterVerified,
    /// Stored, served during a cooldown. Nothing may be spent to check.
    Cooldown,
    /// Stored, served because the counter poll failed. No evidence either way.
    PollFailed,
    /// Stored, served because `--offline` said not to look.
    CacheFlag,
    /// Stored, served because the counter moved but the list was walked less
    /// than [`crate::engine::ListQuery::walk_at_most_every`] ago.
    WalkedRecently,
}

impl Provenance {
    /// Whether this list is known to describe the account as it is now.
    pub fn describes_now(self) -> bool {
        matches!(self, Self::Walked | Self::CounterVerified)
    }

    /// The word a program reads this by (`--progress-json`'s `served` event,
    /// which `snob mcp` hands an agent as `served`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Walked => "walked",
            Self::CounterVerified => "stored_unchanged",
            Self::Cooldown => "stored_in_cooldown",
            Self::PollFailed => "stored_after_failed_poll",
            Self::CacheFlag => "stored",
            Self::WalkedRecently => "stored_walked_recently",
        }
    }
}
