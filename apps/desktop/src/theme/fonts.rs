//! The two families the app draws with, built in: IBM Plex Mono for the
//! brand, titles, labels and usernames, IBM Plex Sans for reading. Where
//! they came from and their license are in `assets/fonts/`.

use std::borrow::Cow;

/// The brand, titles, labels and usernames.
pub const MONO: &str = "IBM Plex Mono";

/// Text that is read.
pub const SANS: &str = "IBM Plex Sans";

/// Every font file, to hand to the text system once at the start.
pub fn all() -> Vec<Cow<'static, [u8]>> {
    macro_rules! font {
        ($name:literal) => {
            Cow::Borrowed(include_bytes!(concat!("../../assets/fonts/", $name)).as_slice())
        };
    }
    vec![
        font!("IBMPlexMono-Regular.ttf"),
        font!("IBMPlexMono-Medium.ttf"),
        font!("IBMPlexMono-Bold.ttf"),
        font!("IBMPlexMono-BoldItalic.ttf"),
        font!("IBMPlexSans-Regular.ttf"),
        font!("IBMPlexSans-Medium.ttf"),
        font!("IBMPlexSans-SemiBold.ttf"),
        font!("IBMPlexSans-Bold.ttf"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_built_in_is_a_font_file() {
        let fonts = all();
        assert_eq!(fonts.len(), 8);
        for font in fonts {
            // A TrueType file starts with its version, 1.0.
            assert_eq!(font[..4], [0, 1, 0, 0]);
            assert!(font.len() > 100_000);
        }
    }
}
