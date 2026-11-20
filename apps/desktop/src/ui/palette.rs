//! The command palette: what can be done from the screen shown, found by
//! typing. Opened with Ctrl+K, or Cmd+K on macOS.
//!
//! **It keeps focus inside it while it is open, closes on Escape or a press
//! outside, and hands focus back to where it was.** The arrows choose, Enter
//! does; what it lists is `state::commands`, and nothing is done only here.

use gpui_kit::base::{Button, FocusTrapElement as _, TestSupportExt as _};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, MouseButton, Role, div, px};

use super::look::{Look, color, color_at, sp};
use super::parts;
use super::screens::handle;
use super::shell::Shell;
use crate::state::commands::Command;
use crate::theme::tokens::{frame, shape};

/// How wide the palette is, at the default text size.
const WIDTH: f32 = 520.0;

/// How tall a command's row is, at the default text size.
const ROW: f32 = 44.0;

/// What a command is called: the words of the button that does the same.
pub fn label(command: Command, shell: &Shell, look: &Look) -> String {
    match command {
        Command::SignInBrowser => look.tr("start-browser"),
        Command::SignInPaste => look.tr("start-paste"),
        Command::Go(place) => super::screens::section_title(place, look),
        Command::Settings => look.tr("account-settings"),
        Command::AddAccount => look.tr("account-add"),
        Command::SwitchTo(pk) => {
            let account = shell
                .accounts()
                .iter()
                .find(|account| account.named.pk == pk)
                .map_or_else(|| pk.to_string(), |account| handle(&account.named));
            look.tr_with("palette-switch", &[("account", &account)])
        }
        Command::SignOut => look.tr("account-sign-out"),
        Command::OpenLogs => look.tr("settings-open-logs"),
        Command::CopyReport => look.tr("settings-copy-report"),
        Command::Profile => look.tr("account-profile"),
    }
}

/// The keys that do a command without the palette, where there are some.
fn shortcut(command: Command) -> Option<&'static str> {
    (command == Command::Settings).then_some(super::SETTINGS_SHORTCUT)
}

/// The palette and what is under it, when it is open.
pub fn render(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Vec<AnyElement> {
    let colors = look.palette;
    let Some(open) = shell.palette.as_ref() else {
        return Vec::new();
    };
    let (input, trap) = (open.input.clone(), open.focus.clone());
    let listed = shell.listed(cx);
    let selected = open.selected.min(listed.len().saturating_sub(1));
    let title = look.tr("palette-title");

    let rows: Vec<Button> = listed
        .iter()
        .enumerate()
        .map(|(row, (command, label))| {
            let command = *command;
            let keys = shortcut(command);
            Button::new(("palette-command", row))
                .role(Role::ListBoxOption)
                .accessibility_label(label.clone())
                .aria_selected(row == selected)
                .when_some(keys, |row, keys| row.aria_keyshortcuts(keys))
                .w_full()
                .h(sp(ROW))
                .flex_none()
                .px(sp(12.))
                .gap(sp(10.))
                .justify_start()
                .rounded(px(shape::CONTROL_RADIUS))
                .text_size(sp(14.))
                .text_color(color(colors.text))
                .when(row == selected, |row| {
                    // The frame's color is the panel's under more contrast:
                    // there the outline says which.
                    row.bg(color(colors.frame))
                        .when(look.settings.more_contrast, |row| {
                            row.border_1().border_color(color(colors.text))
                        })
                })
                .hover(move |style| style.bg(color(colors.frame)))
                .focus_visible(parts::focus_ring(&colors))
                .cursor_pointer()
                .child(label.clone())
                .child(div().flex_1())
                .children(keys.map(|keys| {
                    div()
                        .text_size(sp(12.))
                        .text_color(color(colors.secondary))
                        .child(keys)
                }))
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.run_from_the_palette(command, window, cx);
                }))
        })
        .collect();

    let dialog = div()
        .id("palette")
        .test_support()
        .role(Role::Dialog)
        .aria_label(title.clone())
        .key_context("SnobPalette")
        .on_action(cx.listener(|shell, _: &super::PaletteUp, _, cx| {
            shell.palette_moves(-1, cx);
        }))
        .on_action(cx.listener(|shell, _: &super::PaletteDown, _, cx| {
            shell.palette_moves(1, cx);
        }))
        .occlude()
        .w(sp(WIDTH))
        .max_w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .p(sp(8.))
        .bg(color(colors.panel))
        .border_1()
        .border_color(color(colors.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg();
    let inside = div()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(sp(6.))
        .child(
            Input::new(&input)
                .id("palette-input")
                .aria_label(look.tr("palette-placeholder"))
                .large(),
        )
        .child(parts::room_for_focus(
            div()
                .id("palette-commands")
                .test_support()
                .role(Role::ListBox)
                .aria_label(title)
                .min_h_0()
                .flex()
                .flex_col()
                // Six rows and a half, and the room each side keeps for
                // a row's focus ring; less where the window is lower.
                .max_h(px(
                    ROW * 6.5 * look.settings.text_scale() + 2.0 * shape::FOCUS_ROOM
                ))
                .overflow_y_scroll()
                .children(rows),
        ))
        .when(listed.is_empty(), |inside| {
            // Said to a screen reader too: the list just went empty.
            inside.child(
                div()
                    .id("palette-empty")
                    .test_support()
                    .role(Role::Status)
                    .aria_label(look.tr("palette-empty"))
                    .px(sp(12.))
                    .py(sp(10.))
                    .text_size(sp(14.))
                    .text_color(color(colors.secondary))
                    .child(look.tr("palette-empty")),
            )
        })
        .focus_trap("palette-inside", &trap);

    vec![
        // A press anywhere else closes it.
        div()
            .id("palette-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .bg(color_at(colors.frame, 0.72))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, _, window, cx| shell.close_the_palette(window, cx)),
            )
            .into_any_element(),
        div()
            .absolute()
            .top(px(frame::BAND + 40.))
            .left(px(frame::SIDE * 2.))
            .right(px(frame::SIDE * 2.))
            .bottom(px(frame::SIDE * 2.))
            .flex()
            .justify_center()
            .items_start()
            .child(dialog.child(inside))
            .into_any_element(),
    ]
}
