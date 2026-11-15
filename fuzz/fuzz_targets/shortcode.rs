//! Fuzz target for `snob_ig::shortcode` link and code parsers.
//!
//! Feeds arbitrary UTF-8 strings into [`Shortcode::parse`], [`pk_of`], and
//! [`CommentPk::parse`] so malformed user or app URLs never panic.

#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use snob_ig::shortcode::{CommentPk, Shortcode, code_of, pk_of};

fuzz_target!(|data: &[u8]| {
    let Ok(input) = std::str::from_utf8(data) else {
        return;
    };

    if let Ok(parsed) = Shortcode::parse(input) {
        let pk = parsed.pk();
        let _ = black_box(code_of(pk));
        let _ = black_box(parsed.page());
    }
    let _ = black_box(pk_of(input));
    let _ = black_box(CommentPk::parse(input));
});
