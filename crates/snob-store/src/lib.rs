//! Everything snob keeps on the machine it runs on: the SQLite databases and
//! their migrations, the platform directories, the credential store, the
//! account registry and the monitor's `watch.toml`.
//!
//! **The direction is one-way**: this crate depends on `snob_core`, never the
//! reverse. Everything here is per user, never per directory.

pub mod config;
pub mod layout;
pub mod paths;
pub mod registry;
pub mod secrets;
pub mod store;
#[cfg(windows)]
pub mod windows_user;
