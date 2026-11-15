//! Fuzz target for `snob_ig::error` classification and envelope parsers.
//!
//! Splits the raw byte stream into a `u16` HTTP status and UTF-8 body so every
//! branch reachable from Instagram's responses is explored without panicking.

#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let status = u16::from_le_bytes([data[0], data[1]]);
    let Ok(body) = std::str::from_utf8(&data[2..]) else {
        return;
    };

    let _ = black_box(snob_ig::error::declares_failure(body));
    let _ = black_box(snob_ig::error::refused(status, body));
    let _ = black_box(snob_ig::error::classify(status, body));
    let _ = black_box(snob_ig::error::push_back(status, body));
    let _ = black_box(snob_ig::error::landed_on(body));
});
