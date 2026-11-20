//! The window: the frame every screen is in ([`frame`]) and its rail of
//! sections ([`rail`]), the screens ([`screens`]), what they are built from ([`parts`]), and the one view
//! that holds them and does what they ask ([`shell`]), with what the
//! automations section holds while it is shown ([`automations`]) and what
//! its editor's person does ([`automations_editor`]); what opens over a
//! profile ([`overlays`]) and a list moved out to a window of its own
//! ([`detached`]); and the one way a view asks for its next frame
//! ([`frames`]); and the moment a screen's words are said against ([`now`]).
//!
//! **A view draws what the state says and says what the person did.** What
//! follows from it is decided in `state` and done by the shell, so a screen
//! has no rule of its own to get wrong.

use gpui_kit::{App, KeyBinding, actions};

pub mod assets;
pub mod automations;
pub mod automations_editor;
pub mod detached;
pub mod frame;
pub mod frames;
pub mod look;
pub mod menu;
pub mod now;
pub mod overlays;
pub mod palette;
pub mod parts;
pub mod rail;
pub mod screens;
pub mod scroll;
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
        /// Goes to the item above, or below, in the menu open.
        MenuUp,
        MenuDown,
        /// Goes to the name above, or below, among those suggested under a
        /// field.
        SuggestUp,
        SuggestDown,
        /// One step back, as the band's arrow does.
        GoBack
    ]
);

/// The keys that go one step back: the system's own for it.
pub const BACK_KEYS: &str = if cfg!(target_os = "macos") {
    "cmd-["
} else {
    "alt-left"
};

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
        KeyBinding::new(BACK_KEYS, GoBack, Some("Snob")),
        KeyBinding::new("up", PaletteUp, Some("SnobPalette")),
        KeyBinding::new("down", PaletteDown, Some("SnobPalette")),
        KeyBinding::new("up", MenuUp, Some("SnobMenu")),
        KeyBinding::new("down", MenuDown, Some("SnobMenu")),
        KeyBinding::new("up", SuggestUp, Some("SnobSuggest")),
        KeyBinding::new("down", SuggestDown, Some("SnobSuggest")),
    ]);
}
