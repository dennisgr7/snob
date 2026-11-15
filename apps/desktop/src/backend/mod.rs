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
use snob_ig::pace::CancelToken;
use tokio::sync::{oneshot, watch};

use crate::state::flow::{Found, Named, Update};

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

    /// Signs in with a pasted session ID.
    fn sign_in_with(&self, session_id: Secret) -> Answer<SignIn>;

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
}

/// An answer already known.
pub fn answered<T>(value: T) -> Answer<T> {
    let (answer, answered) = oneshot::channel();
    // The receiver is right here.
    let _ = answer.send(value);
    answered
}
