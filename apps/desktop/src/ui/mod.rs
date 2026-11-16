//! The window: the frame every screen is in ([`frame`]) and its rail of
//! sections ([`rail`]), the screens ([`screens`]), what they are built from ([`parts`]), and the one view
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
pub mod palette;
pub mod parts;
pub mod rail;
pub mod screens;
pub mod shell;
pub mod window;

actions!(
    snob,
    [
        /// Closes the menu or the question that is open.
        Dismiss,
        /// Opens the settings.
        OpenSettings,
        /// Opens the command palette, or closes it.
        OpenPalette,
        /// Chooses the command above, or below, in the palette.
        PaletteUp,
        PaletteDown,
        /// Goes to the item above, or below, in the account's menu.
        MenuUp,
        MenuDown
    ]
);

/// The keys that open the settings: the system's own for it.
pub const SETTINGS_KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-,"
} else {
    "ctrl-,"
};

/// The same keys, as they are shown to the person.
pub const SETTINGS_SHORTCUT: &str = if cfg!(target_os = "macos") {
    "\u{2318},"
} else {
    "Ctrl+,"
};

/// The keys that open the command palette.
pub const PALETTE_KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-k"
} else {
    "ctrl-k"
};

/// What the keys do everywhere in the window, beside what the toolkit's
/// root binds: Tab and Shift+Tab, which stay inside a menu or a dialog
/// while one is open.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("Snob")),
        KeyBinding::new(SETTINGS_KEYS, OpenSettings, Some("Snob")),
        KeyBinding::new(PALETTE_KEYS, OpenPalette, Some("Snob")),
        KeyBinding::new("up", PaletteUp, Some("SnobPalette")),
        KeyBinding::new("down", PaletteDown, Some("SnobPalette")),
        KeyBinding::new("up", MenuUp, Some("SnobMenu")),
        KeyBinding::new("down", MenuDown, Some("SnobMenu")),
    ]);
}
