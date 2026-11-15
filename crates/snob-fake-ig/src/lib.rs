//! The fake Instagram the tests talk to, and a tab on it with no browser.
//!
//! **This is the other side, never part of snob**: no shipped binary links
//! it (`crates/snob-core/tests/layers.rs`), and no test reaches the real
//! Instagram because this is what a client is pointed at instead.

pub mod tab;
pub mod world;
