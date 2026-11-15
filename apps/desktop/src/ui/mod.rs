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
pub mod parts;
pub mod screens;
pub mod shell;
pub mod window;

actions!(
    snob,
    [
        /// Moves focus to the next control.
        FocusNext,
        /// Moves focus to the control before.
        FocusPrevious
    ]
);

/// What the keys do everywhere in the window.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", FocusNext, Some("Snob")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("Snob")),
    ]);
}
