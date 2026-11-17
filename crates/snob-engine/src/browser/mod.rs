//! The browser every request leaves from (AGENTS.md, "Talking to
//! Instagram"), from the binary installed on the machine up to the process
//! that runs one for every command. Each part stands on the one before it:
//!
//! - the browser installed, and the User-Agent it sends ([`detect`],
//!   [`refresh_user_agent`]);
//! - one started on a pipe (`system::pipe`) and driven over the DevTools
//!   protocol ([`cdp`]);
//! - the one without a window an account's requests are sent from: its
//!   profile, its tab, the guard on what leaves it and the listener for
//!   push-back ([`headless`]);
//! - the process that runs those for every command of this user's
//!   ([`owner`]).
//!
//! **Nothing here knows a list, a post or the monitor.** Of the rest of the
//! engine it names `system`, and `host` and `sentences` for what it says
//! (`crates/snob-core/tests/layers.rs` reads the sources).

pub mod cdp;
pub mod headless;
mod installed;
pub mod owner;

pub use installed::{Browser, detect, detect_all, detect_named, refresh_user_agent};
