//! What a view asks for and is answered with, apart from who answers: the
//! engine ([`engine`]), linked in process, or in the test build a script
//! ([`scripted`]) that answers what a test told it to.
//!
//! **Every call answers later, through a channel** ([`Answer`]): none of
//! them reads a file, the keyring or the network on the window's thread.
//! **No call takes a path or a credential a view made up**: an account is
//! named by its pk, and the one secret that crosses is the session ID a
//! person pasted, on its way to the engine.

use std::path::PathBuf;

use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_engine::login::Failure;
use snob_engine::storage::Kind;
use snob_ig::pace::CancelToken;
use snob_store::disk::{Size, Volume};
use tokio::sync::{oneshot, watch};

use crate::state::flow::{Found, Method, Named, Update};
use crate::state::prefs::{Appearance, Placement};

pub mod engine;
#[cfg(feature = "testing")]
pub mod scripted;

/// What a call is answered through. A sender dropped without answering
/// reads as the call having failed.
pub type Answer<T> = oneshot::Receiver<T>;

/// An account signed in on this computer, as a view shows it. Every string
/// was made printable where it was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub named: Named,
    pub full_name: Option<String>,
    pub biography: Option<String>,
    /// Its small picture on disk, when one is kept.
    pub picture: Option<PathBuf>,
    /// When that picture was written: a new one in the same place is told
    /// from the one already drawn by it.
    pub picture_kept_at: Option<std::time::SystemTime>,
    /// The account the app acts as.
    pub active: bool,
}

/// What the app found when it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub found: Found,
    /// Every account signed in here, the one in use first.
    pub accounts: Vec<Account>,
    /// The report of a crash nobody has been shown yet.
    pub crash: Option<String>,
    /// The method chosen shortly before a restart for an update.
    pub resume: Option<Method>,
}

/// The browsers a pasted session can be said to come from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Browsers {
    /// Each one installed that snob can use, by the engine's name for it, in
    /// its order of preference.
    pub found: Vec<String>,
    /// The one chosen the last time, when it is still installed.
    pub last: Option<String>,
}

/// What snob keeps on this computer, measured from its files' sizes alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Storage {
    /// The disk snob's data directory is on, when the system said.
    pub disk: Option<Volume>,
    /// What each kind weighs, every account's included, in
    /// [`Kind::ALL`]'s order.
    pub kinds: Vec<(Kind, Size)>,
    /// What each account with a directory here holds, signed in or not: the
    /// one in use first, then the others signed in, then the rest.
    pub accounts: Vec<AccountStorage>,
}

/// What one account with a directory here holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountStorage {
    pub pk: Pk,
    pub username: Option<String>,
    /// Whether a session of it is signed in here; one that is not left its
    /// files behind.
    pub signed_in: bool,
    /// Its history: its database (lists, captures, the monitor's work and
    /// what paces it), and its jobs and their results.
    pub history: Size,
    /// Its views' cache.
    pub cache: Size,
}

/// What clearing the cache freed, and whether all of it went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleared {
    pub freed: Size,
    pub whole: bool,
}

/// What forgetting history freed, and of how many accounts it was not
/// forgotten.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forgotten {
    pub freed: Size,
    /// Accounts left alone because a job or a run of theirs was going.
    pub busy: usize,
    /// Accounts the disk or the database refused, logged.
    pub failed: usize,
}

/// How a sign-in ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// The session is stored and its account is the one in use.
    Done(Account),
    /// The person closed the login window, or canceled.
    GaveUp,
    Failed {
        why: Failure,
        /// The code the failure was logged under.
        id: String,
        /// Until when Instagram asked the account to wait, when it did.
        until: Option<Epoch>,
    },
}

/// Who answers the views.
pub trait Backend: Send + Sync + 'static {
    /// What is on this computer: read from files alone, nothing sent.
    fn load(&self) -> Answer<Loaded>;

    /// The browsers installed that a session can be sent as. Nothing is
    /// sent: each is asked its version, on this computer.
    fn browsers(&self) -> Answer<Browsers>;

    /// Signs in with a pasted session ID, as the browser called `browser`:
    /// the one the session was copied from, **which its requests are sent
    /// as from then on, that browser's later versions included**
    /// (`snob_engine::browser::refresh_user_agent`). With none named, as
    /// the first one found.
    fn sign_in_with(&self, session_id: Secret, browser: Option<String>) -> Answer<SignIn>;

    /// Signs in through a window of snob's own browser, as `renewing` when
    /// that account's session expired. `stop` closes the window and ends it
    /// as given up.
    fn sign_in_in_a_window(&self, renewing: Option<Pk>, stop: CancelToken) -> Answer<SignIn>;

    /// Signs `pk` out of this computer, and answers with the accounts left.
    fn sign_out(&self, pk: Pk) -> Answer<Vec<Account>>;

    /// Makes `pk` the account in use, and answers with the accounts.
    fn switch_to(&self, pk: Pk) -> Answer<Vec<Account>>;

    /// Until when Instagram asked the account in use to wait, when it did.
    /// Read from its budget; nothing is sent.
    fn paused_until(&self) -> Answer<Option<Epoch>>;

    /// The account in use, when Instagram no longer takes its session.
    /// **Asked of Instagram once a day at most, and never during a pause**:
    /// `None` is a session that works, one found to work lately, or one
    /// that could not be asked about.
    fn expired_session(&self) -> Answer<Option<Named>>;

    /// Remembers the method chosen before a restart for an update, or with
    /// `None` forgets it.
    fn remember_method(&self, method: Option<Method>);

    /// Remembers where the window is, to open there the next time.
    fn remember_window(&self, placement: Placement);

    /// Remembers the theme and the language chosen, for the next start.
    fn remember_appearance(&self, appearance: Appearance);

    /// Whether a newer version is out.
    fn check_for_update(&self) -> Answer<Update>;

    /// Fetches the newer version, saying how far it has come as a whole
    /// number from 0 to 100 when the size is known, and answers whether it
    /// is ready to restart into.
    fn fetch_update(&self, progress: watch::Sender<Option<u8>>) -> Answer<bool>;

    /// The log's switch for the detailed level: until when it is on.
    fn detailed_until(&self) -> Option<Epoch>;

    /// Turns the log's detailed level on, for a day, or off.
    fn set_detailed(&self, on: bool);

    /// Where the log is kept.
    fn logs_dir(&self) -> PathBuf;

    /// The diagnostic report: recent log lines with accounts made anonymous,
    /// and what the system is.
    fn diagnostic_report(&self) -> Answer<String>;

    /// Marks the crash report [`Loaded::crash`] carried as shown.
    fn crash_shown(&self);

    /// What snob keeps on this computer. **From metadata only**: no
    /// database is opened.
    fn storage(&self) -> Answer<Storage>;

    /// Removes the cache of `only`, or with `None` of every account here,
    /// and nothing else (`snob_engine::storage::clear_cache`): what a view
    /// showed at once, which its requests fetch again.
    fn clear_cache(&self, only: Option<Pk>) -> Answer<Cleared>;

    /// Forgets the history of `only`, or with `None` of every account with
    /// a directory here (`snob_engine::storage::forget_history`): its lists,
    /// captures, the monitor's work, its jobs and its cache. **Never its
    /// session, its browser, nor what paces it.** An account with a job or
    /// a run going is left alone and counted.
    fn forget_history(&self, only: Option<Pk>) -> Answer<Forgotten>;
}

/// An answer already known.
pub fn answered<T>(value: T) -> Answer<T> {
    let (answer, answered) = oneshot::channel();
    // The receiver is right here.
    let _ = answer.send(value);
    answered
}
