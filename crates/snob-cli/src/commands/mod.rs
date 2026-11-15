//! The commands, one module each: they take an `App` and orchestrate, and
//! the wording and exit codes of what the engine returns live here and in
//! `report`. `snob mcp` is `snob_mcp`'s, and `snob jobs` is `crate::jobs`'.

pub mod account;
pub mod background;
pub mod changes;
pub mod common;
pub mod dry_run;
pub mod fetch;
pub mod follow;
pub mod highlights;
pub mod import;
pub mod likers;
pub mod lists;
pub mod login;
pub mod logout;
pub mod pfp;
pub mod post;
pub mod posts;
pub mod profile;
pub mod purge;
pub mod scan;
pub mod sets;
pub mod snapshots;
pub mod start;
pub mod status;
pub mod stories;
pub mod update;
pub mod watch;
pub mod whoami;
