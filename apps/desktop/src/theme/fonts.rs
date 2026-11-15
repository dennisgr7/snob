//! The two families the app draws with, built in: Inter for every word that
//! is read, titles and buttons included, and IBM Plex Mono for the brand and
//! for what is code (the session ID field, a failure's code, a crash
//! report). Where they came from, why Inter, and their license are in
//! `assets/fonts/`.

use std::borrow::Cow;

/// The brand, and what is code.
pub const MONO: &str = "IBM Plex Mono";

/// Every word that is read.
pub const SANS: &str = "Inter";

/// Every font file, to hand to the text system once at the start.
pub fn all() -> Vec<Cow<'static, [u8]>> {
    macro_rules! font {
        ($name:literal) => {
            Cow::Borrowed(include_bytes!(concat!("../../assets/fonts/", $name)).as_slice())
        };
    }
    vec![
        font!("IBMPlexMono-Regular.ttf"),
        font!("IBMPlexMono-BoldItalic.ttf"),
        font!("Inter-Regular.ttf"),
        font!("Inter-Medium.ttf"),
        font!("Inter-SemiBold.ttf"),
        font!("Inter-Bold.ttf"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_built_in_is_a_font_file() {
        let fonts = all();
        assert_eq!(fonts.len(), 6);
        for font in fonts {
            // A TrueType file starts with its version, 1.0.
            assert_eq!(font[..4], [0, 1, 0, 0]);
            assert!(font.len() > 100_000);
        }
    }
}
