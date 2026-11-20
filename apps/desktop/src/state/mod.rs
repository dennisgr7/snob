//! What the app knows, apart from how it draws it: which screen is shown and
//! why ([`flow`]), the frame's shape and its way between them ([`frame`]),
//! what the top band says ([`status`]), what a pasted session ID is
//! ([`paste`]), what the command palette offers ([`commands`]), what the
//! settings hold and which of it a search shows ([`settings`]), what they
//! say of what is kept ([`storage`]), what is remembered between runs
//! ([`prefs`]), when the app rests ([`rest`]), the automations section:
//! its introduction, its list and its editor's canvas ([`automations`]),
//! the automation the editor holds ([`editing`]), its panel of steps
//! ([`shelf`]), what its dialogs turn what is typed into ([`steps`]) and a
//! schedule as its dialog chooses it ([`schedule`]),
//! how a profile page is laid out and what it reads without asking
//! ([`profile`]), what a list of followers asks for and copies
//! ([`people`]), and how often the window is drawn while something moves
//! ([`frames`]).
//! No window, no clock and no network in any of it.

pub mod automations;
pub mod commands;
pub mod editing;
pub mod flow;
pub mod frame;
pub mod frames;
pub mod paste;
pub mod people;
pub mod prefs;
pub mod profile;
pub mod rest;
pub mod schedule;
pub mod settings;
pub mod shelf;
pub mod status;
pub mod steps;
pub mod storage;
