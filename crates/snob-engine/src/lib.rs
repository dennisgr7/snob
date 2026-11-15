//! What snob does, without its command line: the walks and the rules around
//! them (`engine`, `App`), the browser every request leaves from, the owner
//! of the browsers, the monitor's webhook, jobs, downloads and updates.
//!
//! **It asks nobody anything and draws nothing itself**: whoever runs it hands
//! it a front (`host`), the command line its bars and its prompt, an app its
//! own views.

pub mod account;
pub mod app;
pub mod browser;
pub mod cdp;
pub mod detach;
pub mod engine;
pub mod exe;
pub mod exit;
pub mod ffmpeg;
pub mod files;
pub mod headless;
pub mod host;
pub mod interrupt;
pub mod jobs;
pub mod login;
pub mod logout;
pub mod media;
pub mod owner;
pub mod parent;
pub mod pipe;
pub mod posts;
pub mod power;
pub mod random;
pub mod relay;
#[cfg(feature = "testing")]
pub mod sandbox;
pub mod seat;
pub mod sentences;
pub mod update;
pub mod watch;
