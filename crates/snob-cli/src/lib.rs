//! snob's command line: the commands, how their results are written, the
//! terminal's views, and the front it hands the engine.

// The engine's modules, by the names the command line has always used.
pub use snob_engine::{
    account, app, browser, cdp, engine, exe, exit, ffmpeg, files, headless, host, media, owner,
    posts, random, seat, sentences, update, watch,
};
pub(crate) use snob_engine::{detach, interrupt, power, relay};

pub mod cli;
pub mod commands;
pub mod front;
pub mod jobs;
pub mod output;
pub mod progress;
pub mod report;
pub mod ui;
