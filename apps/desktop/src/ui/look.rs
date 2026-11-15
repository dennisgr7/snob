//! How the app looks now: the palette of the system's theme and contrast,
//! the settings it follows, and the words of its language. **One value for
//! the whole app**, swapped when the system changes a setting, so every view
//! redraws with it and none keeps a color of its own.

use std::sync::Arc;

use gpui_kit::{App, Global, Hsla, Rems, Rgba, px, rems, rgb};

use crate::i18n::Texts;
use crate::system::Settings;
use crate::theme::motion::{self, Move};
use crate::theme::tokens::{Palette, Rgb, Scheme};

/// The look of the app.
#[derive(Clone)]
pub struct Look {
    pub palette: Palette,
    pub settings: Settings,
    pub texts: Arc<Texts>,
    /// A test build says so in its band.
    pub testing: bool,
}

impl Global for Look {}

impl Look {
    pub fn of(settings: Settings, testing: bool) -> Self {
        let scheme = if settings.dark {
            Scheme::Dark
        } else {
            Scheme::Light
        };
        Self {
            palette: Palette::of(scheme, settings.more_contrast),
            texts: Arc::new(Texts::of(&settings.locale)),
            settings,
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
    gpui_kit::component::Theme::global_mut(cx).font_size =
        px(TEXT_SIZE * look.settings.text_scale());
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
    fn a_measure_of_text_is_in_units_that_grow_with_it() {
        assert_eq!(sp(16.0), rems(1.0));
        assert_eq!(sp(12.0), rems(0.75));
    }
}
