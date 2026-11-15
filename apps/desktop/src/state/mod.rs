//! What the app knows, apart from how it draws it: which screen is shown and
//! why ([`flow`]), what the top band says ([`status`]), what a pasted
//! session ID is ([`paste`]), what the command palette offers
//! ([`commands`]), and what is remembered between runs ([`prefs`]). No
//! window, no clock and no network in any of it.

pub mod commands;
pub mod flow;
pub mod paste;
pub mod prefs;
pub mod status;
