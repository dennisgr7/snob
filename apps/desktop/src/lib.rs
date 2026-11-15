//! The desktop app, as a library: everything but the `main` that opens its
//! window, so its tests and its harness link the same code the binary runs.
//!
//! Every rule of the root `AGENTS.md` holds here through the engine: a view
//! reaches Instagram through `snob_engine`, never a client of its own.

pub mod backend;
pub mod front;
pub mod i18n;
pub mod instance;
pub mod report;
pub mod runtime;
pub mod state;
pub mod system;
pub mod theme;
