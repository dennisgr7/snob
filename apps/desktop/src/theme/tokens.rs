//! The colors, as data: the website's, with a pure white or black panel and
//! one pink in both themes. **Every pair that carries text or marks a control
//! is checked against WCAG 2.2 AA in this file's tests**, in both themes and
//! in their high-contrast variants; a color changed here that no longer
//! passes fails the build's tests, not a review.

/// An opaque color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn hex(value: u32) -> Self {
        Self((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    /// `0xRRGGBB`.
    pub const fn as_hex(self) -> u32 {
        ((self.0 as u32) << 16) | ((self.1 as u32) << 8) | self.2 as u32
    }

    /// Relative luminance (WCAG 2.2, "relative luminance").
    pub fn luminance(self) -> f64 {
        let channel = |value: u8| {
            let c = f64::from(value) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(self.0) + 0.7152 * channel(self.1) + 0.0722 * channel(self.2)
    }
}

/// The contrast ratio of two colors, from 1 to 21 (WCAG 2.2, "contrast
/// ratio").
pub fn contrast(one: Rgb, other: Rgb) -> f64 {
    let (one, other) = (one.luminance(), other.luminance());
    let (lighter, darker) = if one > other {
        (one, other)
    } else {
        (other, one)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// The brand's pink, the same in both themes.
pub const PINK: Rgb = Rgb::hex(0xf472b6);

/// Which of the system's two themes is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    Light,
    Dark,
}

/// Every color a view draws with, named by its role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The content panel.
    pub panel: Rgb,
    /// The frame that holds the panel, the top band included.
    pub frame: Rgb,
    pub text: Rgb,
    pub secondary: Rgb,
    /// Lines that separate, and the outline of a button that has a label.
    pub line: Rgb,
    /// The outline of a control known by its outline alone: a field.
    pub control_line: Rgb,
    /// The brand, a primary button's fill, and the focus ring.
    pub accent: Rgb,
    /// Text on [`Self::accent`].
    pub on_accent: Rgb,
    /// Text that stands out, and a link. **Pink where pink reads as text**
    /// (dark); the text's own color where it does not (light), where what
    /// stands out is bold with a pink underline and a link is underlined.
    pub emphasis: Rgb,
    /// The line under emphasized text and links.
    pub underline: Rgb,
    /// The second contour of the focus ring, inside the pink one, so focus
    /// does not rest on a color alone.
    pub focus_inner: Rgb,
    pub disabled_fill: Rgb,
    pub disabled_text: Rgb,
    /// The mark and text of an error. Never the only sign of one.
    pub error: Rgb,
}

impl Palette {
    pub const fn of(scheme: Scheme, high_contrast: bool) -> Self {
        match (scheme, high_contrast) {
            (Scheme::Light, false) => Self {
                panel: Rgb::hex(0xffffff),
                frame: Rgb::hex(0xf7f7f7),
                text: Rgb::hex(0x111111),
                secondary: Rgb::hex(0x52525b),
                line: Rgb::hex(0xe4e4e7),
                control_line: Rgb::hex(0x85858f),
                accent: PINK,
                on_accent: Rgb::hex(0x0b0d10),
                emphasis: Rgb::hex(0x111111),
                underline: PINK,
                focus_inner: Rgb::hex(0x111111),
                disabled_fill: Rgb::hex(0xe4e4e7),
                disabled_text: Rgb::hex(0x52525b),
                error: Rgb::hex(0xb42318),
            },
            (Scheme::Dark, false) => Self {
                panel: Rgb::hex(0x000000),
                frame: Rgb::hex(0x12151a),
                text: Rgb::hex(0xe6e8ec),
                secondary: Rgb::hex(0x9aa1ab),
                line: Rgb::hex(0x23272f),
                control_line: Rgb::hex(0x6b7280),
                accent: PINK,
                on_accent: Rgb::hex(0x0b0d10),
                emphasis: PINK,
                underline: PINK,
                focus_inner: Rgb::hex(0x000000),
                disabled_fill: Rgb::hex(0x23272f),
                disabled_text: Rgb::hex(0x9aa1ab),
                error: Rgb::hex(0xff8a80),
            },
            // The system asked for more contrast: the frame and the lines
            // stay, drawn in the text's color, and the pink moves to a shade
            // that reads as text on the panel.
            (Scheme::Light, true) => Self {
                panel: Rgb::hex(0xffffff),
                frame: Rgb::hex(0xffffff),
                text: Rgb::hex(0x000000),
                secondary: Rgb::hex(0x1f1f23),
                line: Rgb::hex(0x000000),
                control_line: Rgb::hex(0x000000),
                accent: Rgb::hex(0x9d174d),
                on_accent: Rgb::hex(0xffffff),
                emphasis: Rgb::hex(0x9d174d),
                underline: Rgb::hex(0x9d174d),
                focus_inner: Rgb::hex(0xffffff),
                disabled_fill: Rgb::hex(0xffffff),
                disabled_text: Rgb::hex(0x52525b),
                error: Rgb::hex(0x8f1409),
            },
            (Scheme::Dark, true) => Self {
                panel: Rgb::hex(0x000000),
                frame: Rgb::hex(0x000000),
                text: Rgb::hex(0xffffff),
                secondary: Rgb::hex(0xe6e8ec),
                line: Rgb::hex(0xffffff),
                control_line: Rgb::hex(0xffffff),
                accent: Rgb::hex(0xf9a8d4),
                on_accent: Rgb::hex(0x000000),
                emphasis: Rgb::hex(0xf9a8d4),
                underline: Rgb::hex(0xf9a8d4),
                focus_inner: Rgb::hex(0x000000),
                disabled_fill: Rgb::hex(0x000000),
                disabled_text: Rgb::hex(0x9aa1ab),
                error: Rgb::hex(0xffb4ab),
            },
        }
    }
}

/// The frame's measures, in logical pixels.
pub mod frame {
    /// The top band: the height of the system's own window buttons.
    pub const BAND: f32 = 42.0;
    /// The frame at the sides and under the panel.
    pub const SIDE: f32 = 14.0;
    /// The panel's corners. **A default to confirm** (20 to 24).
    pub const PANEL_RADIUS: f32 = 22.0;
    /// How much of the band's color stays once content scrolls under it:
    /// translucent, with no blur (GPUI has none per element). Opaque when
    /// the system asks for less transparency.
    pub const BAND_ALPHA_OVER_CONTENT: f32 = 0.86;
}

/// The shapes of controls, in logical pixels.
pub mod shape {
    pub const CONTROL_RADIUS: f32 = 6.0;
    pub const CARD_RADIUS: f32 = 10.0;
    /// The focus ring: the pink contour, and the contour inside it.
    pub const FOCUS_RING: f32 = 3.0;
    pub const FOCUS_INNER: f32 = 1.0;
    /// The least a target measures on either side (WCAG 2.5.8).
    pub const MIN_TARGET: f32 = 24.0;
    /// What a main button measures at least.
    pub const BUTTON_HEIGHT: f32 = 36.0;
    /// No text is drawn smaller: a reduced screenshot still reads it.
    pub const MIN_TEXT: f32 = 12.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What WCAG 2.2 AA asks of text (1.4.3) and of what marks a control or
    /// its state (1.4.11).
    const TEXT: f64 = 4.5;
    const MARK: f64 = 3.0;

    fn every() -> [(&'static str, Palette); 4] {
        [
            ("light", Palette::of(Scheme::Light, false)),
            ("dark", Palette::of(Scheme::Dark, false)),
            ("light, more contrast", Palette::of(Scheme::Light, true)),
            ("dark, more contrast", Palette::of(Scheme::Dark, true)),
        ]
    }

    #[test]
    fn the_ratio_is_the_one_the_guideline_computes() {
        let (black, white) = (Rgb::hex(0x000000), Rgb::hex(0xffffff));
        assert!((contrast(black, white) - 21.0).abs() < 0.001);
        assert!((contrast(white, white) - 1.0).abs() < 0.001);
        // The guideline's own example: #777 on white is just under 4.5.
        let grey = contrast(Rgb::hex(0x777777), white);
        assert!((4.47..4.49).contains(&grey), "{grey}");
        assert_eq!(Rgb::hex(0xf472b6).as_hex(), 0xf472b6);
    }

    #[test]
    fn every_text_reads_on_what_it_is_drawn_on() {
        for (name, p) in every() {
            for (what, text, on) in [
                ("text on the panel", p.text, p.panel),
                ("text on the frame", p.text, p.frame),
                ("secondary text on the panel", p.secondary, p.panel),
                ("secondary text on the frame", p.secondary, p.frame),
                ("text on the accent", p.on_accent, p.accent),
                ("emphasis on the panel", p.emphasis, p.panel),
                ("emphasis on the frame", p.emphasis, p.frame),
                ("an error on the panel", p.error, p.panel),
            ] {
                let ratio = contrast(text, on);
                assert!(ratio >= TEXT, "{name}: {what} is {ratio:.2}:1");
            }
        }
    }

    #[test]
    fn what_marks_a_control_stands_out_from_what_is_around_it() {
        for (name, p) in every() {
            for (what, mark, on) in [
                ("a field's outline on the panel", p.control_line, p.panel),
                // The ring is two contours: where the pink one does not reach
                // 3:1, the one inside it does.
                (
                    "the focus ring on the panel",
                    best_of(p.accent, p.focus_inner, p.panel),
                    p.panel,
                ),
                (
                    "the focus ring on the frame",
                    best_of(p.accent, p.focus_inner, p.frame),
                    p.frame,
                ),
                (
                    "a primary button on the panel",
                    best_of(p.accent, p.on_accent, p.panel),
                    p.panel,
                ),
            ] {
                let ratio = contrast(mark, on);
                assert!(ratio >= MARK, "{name}: {what} is {ratio:.2}:1");
            }
            // And the two contours of the ring are told apart from each other.
            let ring = contrast(p.accent, p.focus_inner);
            assert!(ring >= MARK, "{name}: the ring's contours are {ring:.2}:1");
        }
    }

    /// Whichever of two colors stands out more from `on`.
    fn best_of(one: Rgb, other: Rgb, on: Rgb) -> Rgb {
        if contrast(one, on) >= contrast(other, on) {
            one
        } else {
            other
        }
    }

    /// Why the light theme does not write in pink: the brand's pink on white
    /// is under what even large text needs.
    #[test]
    fn the_pink_is_text_only_where_it_reads() {
        let light = Palette::of(Scheme::Light, false);
        assert!(contrast(PINK, light.panel) < MARK);
        assert_ne!(light.emphasis, PINK);
        assert_eq!(light.underline, PINK);

        let dark = Palette::of(Scheme::Dark, false);
        assert!(contrast(PINK, dark.panel) >= TEXT);
        assert_eq!(dark.emphasis, PINK);
    }

    #[test]
    fn the_usual_themes_are_the_documented_colors() {
        let light = Palette::of(Scheme::Light, false);
        assert_eq!(
            [
                light.panel,
                light.frame,
                light.text,
                light.secondary,
                light.line
            ],
            [0xffffff, 0xf7f7f7, 0x111111, 0x52525b, 0xe4e4e7].map(Rgb::hex)
        );
        let dark = Palette::of(Scheme::Dark, false);
        assert_eq!(
            [dark.panel, dark.frame, dark.text, dark.secondary, dark.line],
            [0x000000, 0x12151a, 0xe6e8ec, 0x9aa1ab, 0x23272f].map(Rgb::hex)
        );
        for palette in [light, dark] {
            assert_eq!(palette.accent, PINK);
            assert_eq!(palette.on_accent, Rgb::hex(0x0b0d10));
        }
    }

    #[test]
    fn a_disabled_control_is_still_legible() {
        // Exempt from 1.4.3, and still held to what a mark needs here: it is
        // read by its words, not by its grey alone.
        for (name, p) in every() {
            let ratio = contrast(p.disabled_text, p.disabled_fill);
            assert!(ratio >= MARK, "{name}: disabled text is {ratio:.2}:1");
        }
    }
}
