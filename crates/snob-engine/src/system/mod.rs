//! What snob asks of the operating system: starting a process and tying its
//! life to this one, hearing that this one should stop, a place two of
//! snob's processes meet, power, chance, and where the person sits.
//!
//! **Nothing here knows Instagram, an account or a walk.** A module of this
//! directory names the rest of the engine only through `host`, to say a
//! notice (`crates/snob-core/tests/layers.rs` reads the sources), so each
//! can be changed, or ported to another system, with nothing above it open.
//! The per-platform halves are in AGENTS.md, "Platforms".

pub mod detach;
pub mod env;
pub mod exe;
pub mod interrupt;
pub mod local_socket;
pub mod parent;
pub mod pipe;
pub mod power;
pub mod random;
pub mod seat;
pub(crate) mod sync;
