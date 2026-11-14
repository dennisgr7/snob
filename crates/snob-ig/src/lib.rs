//! Client for the web API that instagram.com itself uses.
//!
//! **Almost all of it reads. Two writes exist, follow and unfollow, and a
//! third does not compile**: `IgClient::post` is the only function that sends
//! a method other than GET to Instagram, and it takes a
//! [`graphql::Mutation`], not a path. No block, no remove-follower, no like,
//! no comment, no message, and **nothing that marks a story as seen**, which
//! would put the user in somebody else's viewer list.
//!
//! [`graphql::Mutation::ALL`] makes "there are two" a value a test reads, and
//! its matches have no wildcard arm. `crates/snob-core/tests/no_seen.rs` is
//! the backstop for the write that must never appear, and [`allowlist`]
//! refuses, before it leaves, every call from the page that it does not
//! name. The regime the two writes come with is in `AGENTS.md`.

pub mod allowlist;
pub mod awake;
pub mod client;
pub mod client_hints;
pub mod error;
pub mod graphql;
pub mod http;
pub mod login;
pub mod model;
pub mod pace;
pub mod page_values;
pub mod pager;
pub mod shortcode;
pub mod web;

/// Identifies the request as Instagram's web app; several endpoints answer
/// 401 or 403 without it.
pub const IG_APP_ID: &str = "936619743392459";

pub const BASE_URL: &str = "https://www.instagram.com";
