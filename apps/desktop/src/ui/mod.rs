//! The window: the frame every screen is in ([`frame`]), the screens
//! ([`screens`]), what they are built from ([`parts`]), and the one view
//! that holds them and does what they ask ([`shell`]).
//!
//! **A view draws what the state says and says what the person did.** What
//! follows from it is decided in `state` and done by the shell, so a screen
//! has no rule of its own to get wrong.

use gpui_kit::{App, KeyBinding, actions};

pub mod assets;
pub mod frame;
pub mod look;
pub mod menu;
pub mod parts;
pub mod screens;
pub mod shell;
pub mod window;

actions!(
    snob,
    [
        /// Closes the menu or the question that is open.
        Dismiss,
        /// Opens the settings.
        OpenSettings
    ]
);

/// The keys that open the settings: the system's own for it.
pub const SETTINGS_KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-,"
} else {
    "ctrl-,"
};

/// What the keys do everywhere in the window, beside what the toolkit's
/// root binds: Tab and Shift+Tab, which stay inside a menu or a dialog
/// while one is open.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("Snob")),
        KeyBinding::new(SETTINGS_KEYS, OpenSettings, Some("Snob")),
    ]);
}
