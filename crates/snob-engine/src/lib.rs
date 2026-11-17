//! What snob does, without its command line: the walks and the rules around
//! them (`engine`, `App`), the browser every request leaves from, the owner
//! of the browsers, the monitor, jobs, downloads and updates.
//!
//! **One engine for every front**: the command line and the desktop app link
//! it, and the MCP server runs the command line, so a rule kept here is kept
//! for all three. **It asks nobody anything and draws nothing itself**:
//! whoever runs it hands it a front (`host`), the command line its bars and
//! its prompt, an app its own views.
//!
//! Its parts, from what the others stand on to what stands on them:
//!
//! - what it is handed and what it answers with: [`host`], [`exit`];
//! - the operating system: [`system`];
//! - the browser every request leaves from: [`browser`];
//! - a run as an account, its lists, and the words of their refusals:
//!   [`app`], [`engine`], [`sentences`];
//! - an account on this computer: [`account`], [`login`] (from another
//!   machine, [`relay`]), [`logout`], [`viewer`], [`cache`], [`storage`];
//! - what is read besides a list: [`profile`], [`posts`], [`stories`],
//!   [`highlights`], what a story is and how one is kept ([`media`]), and
//!   the files they are kept in ([`files`], [`ffmpeg`]);
//! - the two writes, follow and unfollow: [`follow`];
//! - what runs by itself: the monitor ([`watch`]), [`automations`], [`jobs`];
//! - the program itself: [`log`], [`update`].
//!
//! **The parts others stand on name nothing above them**, which
//! `crates/snob-core/tests/layers.rs` reads the sources for: a part can be
//! changed with what stands on it closed.

pub mod account;
pub mod app;
pub mod automations;
pub mod browser;
pub mod cache;
pub mod engine;
pub mod exit;
pub mod ffmpeg;
pub mod files;
pub mod follow;
pub mod highlights;
pub mod host;
pub mod jobs;
pub mod log;
pub mod login;
pub mod logout;
pub mod media;
pub mod posts;
pub mod profile;
pub mod relay;
#[cfg(feature = "testing")]
pub mod sandbox;
pub mod sentences;
pub mod storage;
pub mod stories;
pub mod system;
pub mod update;
pub mod viewer;
pub mod watch;
