//! Fuzz target for `snob_core::model::printable`.
//!
//! Feeds arbitrary UTF-8 strings from untrusted profile fields and verifies
//! termination without panicking.

#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let _ = black_box(snob_core::model::printable(text));
});
