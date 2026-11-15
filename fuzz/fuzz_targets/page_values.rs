//! Fuzz target for `snob_ig::page_values` document extraction.
//!
//! Feeds arbitrary UTF-8 documents into [`PageValues::parse`] and
//! [`web_session_id`] so malformed script payloads never panic.

#![no_main]

use std::hint::black_box;

use libfuzzer_sys::fuzz_target;
use snob_ig::page_values::{PageValues, web_session_id};

fuzz_target!(|data: &[u8]| {
    let Ok(document) = std::str::from_utf8(data) else {
        return;
    };

    let _ = black_box(PageValues::parse(document));
    let _ = black_box(web_session_id(document));
});
