//! How the app looks now: the palette of the theme and the system's
//! contrast, the settings it follows, and the words of its language. The
//! theme and the language are the system's unless the person chose others
//! in the settings. **One value for the whole app**, swapped when the system
//! or a choice changes, so every view redraws with it and none keeps a color
//! of its own.

use std::sync::Arc;

use gpui_kit::{App, Global, Hsla, Rems, Rgba, px, rems, rgb};

use crate::i18n::{self, Texts};
use crate::state::prefs::{Appearance, ThemeChoice};
use crate::system::Settings;
use crate::theme::fonts::{MONO, SANS};
use crate::theme::motion::{self, Move};
use crate::theme::tokens::{Palette, Rgb, Scheme, shape};

/// The look of the app.
#[derive(Clone)]
pub struct Look {
    pub palette: Palette,
    /// The theme drawn: the one chosen, or the system's.
    pub scheme: Scheme,
    /// What the system says, whatever was chosen over it.
    pub settings: Settings,
    pub texts: Arc<Texts>,
    /// What the person chose over the system.
    pub chosen: Appearance,
    /// A test build says so in its band.
    pub testing: bool,
}

impl Global for Look {}

impl Look {
    /// The system's look, with nothing chosen over it.
    pub fn of(settings: Settings, testing: bool) -> Self {
        Self::chosen(settings, Appearance::default(), testing)
    }

    /// The look of `chosen` over what the system says.
    pub fn chosen(settings: Settings, chosen: Appearance, testing: bool) -> Self {
        let dark = match chosen.theme {
            ThemeChoice::System => settings.dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        };
        let scheme = if dark { Scheme::Dark } else { Scheme::Light };
        let texts = match chosen.language.as_deref() {
            Some(tag) if i18n::speaks(tag) => Texts::of(tag),
            _ => Texts::of(&settings.locale),
        };
        Self {
            palette: Palette::of(scheme, settings.more_contrast),
            scheme,
            texts: Arc::new(texts),
            settings,
            chosen,
            testing,
        }
    }

    /// The look in use.
    pub fn now(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// The message `id`, in the app's language.
    pub fn tr(&self, id: &str) -> String {
        self.texts.tr(id)
    }

    pub fn tr_with(&self, id: &str, values: &[(&str, &str)]) -> String {
        self.texts.tr_with(id, values)
    }

    /// The message for `count` of something: `{id}-one` at one, `id`
    /// otherwise, `$count` put in (`Texts::tr_count`).
    pub fn tr_count(&self, id: &'static str, count: u64, values: &[(&str, &str)]) -> String {
        self.texts.tr_count(id, count, values)
    }

    /// A move, or the short fade that replaces it under "reduce motion".
    pub fn moved(&self, lasts: std::time::Duration, travels: f32, scales: f32) -> Move {
        motion::moved(self.settings.reduce_motion, lasts, travels, scales)
    }

    /// The band may let content show through it.
    pub fn translucent(&self) -> bool {
        !self.settings.reduce_transparency && !self.settings.more_contrast
    }
}

/// The size text is drawn at when the system's is its default, which every
/// measure of text is a share of ([`sp`]).
const TEXT_SIZE: f32 = 16.0;

/// Makes `look` the look in use: its colors and words, and its text size,
/// which the toolkit's root applies to the window on every frame.
pub fn wear(look: Look, cx: &mut App) {
    let palette = look.palette;
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.font_size = px(TEXT_SIZE * look.settings.text_scale());
    theme.font_family = SANS.into();
    theme.mono_font_family = MONO.into();
    theme.radius = px(shape::CONTROL_RADIUS);
    theme.radius_lg = px(shape::CARD_RADIUS);
    // What the toolkit's own controls are drawn with: the field a session ID
    // is pasted into, and whatever of it is used after.
    theme.background = color(palette.panel);
    theme.foreground = color(palette.text);
    theme.muted = color(palette.frame);
    theme.muted_foreground = color(palette.secondary);
    theme.border = color(palette.line);
    theme.input = color(palette.control_line);
    theme.ring = color(palette.accent);
    theme.caret = color(palette.text);
    theme.selection = color_at(palette.accent, 0.35);
    theme.primary = color(palette.accent);
    theme.primary_foreground = color(palette.on_accent);
    theme.popover = color(palette.panel);
    theme.popover_foreground = color(palette.text);
    theme.danger = color(palette.error);
    // What the toolkit's own moves follow: its scrollbar's.
    cx.set_reduce_motion(look.settings.reduce_motion);
    cx.set_global(look);
    cx.refresh_windows();
}

/// A token's color, for GPUI.
pub fn color(rgb_: Rgb) -> Hsla {
    rgb(rgb_.as_hex()).into()
}

/// A token's color at `alpha`.
pub fn color_at(rgb_: Rgb, alpha: f32) -> Hsla {
    let Rgba { r, g, b, .. } = rgb(rgb_.as_hex());
    Rgba { r, g, b, a: alpha }.into()
}

/// A measure that grows with the system's text size: `value` logical pixels
/// at the default size. Text and what holds text are measured in these; the
/// frame is not.
pub fn sp(value: f32) -> Rems {
    rems(value / 16.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_look_follows_the_theme_the_contrast_and_the_language() {
        let look = Look::of(Settings::default(), false);
        assert_eq!(look.palette, Palette::of(Scheme::Light, false));
        assert_eq!(look.texts.language(), "en");
        assert!(look.translucent());

        let look = Look::of(
            Settings {
                dark: true,
                more_contrast: true,
                locale: "es-ES".into(),
                ..Settings::default()
            },
            true,
        );
        assert_eq!(look.palette, Palette::of(Scheme::Dark, true));
        assert_eq!(look.tr("start-title"), "Empecemos.");
        assert!(!look.translucent(), "more contrast is no transparency");
    }

    #[test]
    fn a_theme_chosen_holds_whatever_the_system_says_and_the_system_s_follows_it() {
        let system = |dark: bool, more_contrast: bool| Settings {
            dark,
            more_contrast,
            locale: "es-ES".into(),
            ..Settings::default()
        };
        let chose = |theme: ThemeChoice, language: Option<&str>| Appearance {
            theme,
            language: language.map(str::to_string),
        };
        let light = Look::chosen(system(true, false), chose(ThemeChoice::Light, None), false);
        assert_eq!(light.scheme, Scheme::Light);
        assert_eq!(light.palette, Palette::of(Scheme::Light, false));
        assert!(light.settings.dark, "what the system says is kept");
        let dark = Look::chosen(system(false, true), chose(ThemeChoice::Dark, None), false);
        assert_eq!(
            dark.palette,
            Palette::of(Scheme::Dark, true),
            "the contrast is the system's"
        );
        for dark in [false, true] {
            let followed =
                Look::chosen(system(dark, false), chose(ThemeChoice::System, None), false);
            assert_eq!(followed.settings.dark, followed.scheme == Scheme::Dark);
        }

        let english = Look::chosen(
            system(false, false),
            chose(ThemeChoice::System, Some("en")),
            false,
        );
        assert_eq!(english.texts.language(), "en");
        let unspoken = Look::chosen(
            system(false, false),
            chose(ThemeChoice::System, Some("fr")),
            false,
        );
        assert_eq!(
            unspoken.texts.language(),
            "es",
            "a language the app lacks is the system's"
        );
    }

    #[test]
    fn a_measure_of_text_is_in_units_that_grow_with_it() {
        assert_eq!(sp(16.0), rems(1.0));
        assert_eq!(sp(12.0), rems(0.75));
    }
}
