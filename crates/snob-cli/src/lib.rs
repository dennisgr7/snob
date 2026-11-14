//! snob-ig command line interface.

// `pub(crate)` where nothing outside the crate reaches in: the integration
// tests and `main` are the two consumers.
pub mod account;
pub mod app;
pub mod browser;
pub mod cdp;
pub mod cli;
pub mod commands;
pub(crate) mod detach;
pub mod engine;
pub(crate) mod exe;
pub mod exit;
pub mod ffmpeg;
pub mod headless;
pub(crate) mod interrupt;
pub mod jobs;
pub mod media;
pub mod output;
pub mod owner;
pub(crate) mod parent;
pub(crate) mod pipe;
pub mod posts;
pub(crate) mod power;
pub(crate) mod progress;
pub(crate) mod relay;
pub mod report;
pub mod seat;
pub mod ui;
pub mod update;
pub mod watch;
