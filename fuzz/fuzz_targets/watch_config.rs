//! Fuzz target for `snob_store::config::parse`.
//!
//! Feeds arbitrary UTF-8 TOML text into the `watch.toml` parser so hand-edited
//! or corrupted configs never panic.

#![no_main]

use std::hint::black_box;
use std::path::Path;

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };

    let _ = black_box(snob_store::config::parse(text, Path::new("/watch.toml")));
});
