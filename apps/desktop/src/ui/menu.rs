//! The account's menu, under its picture in the band: who is signed in, the
//! other accounts to change to, the settings, signing out. And the question
//! signing out asks first.
//!
//! **Both keep focus inside them while they are open, close on Escape or a
//! press outside, and hand focus back to where it was.** Nothing in them is
//! reached by the pointer alone: Tab and the arrows walk the menu.

use gpui_kit::base::{Button, FocusTrapElement as _, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, MouseButton, Role, Window, div, px};

use super::assets;
use super::look::{Look, color, color_at, sp};
use super::parts::{self, Kind, icon};
use super::screens::handle;
use super::shell::Shell;
use crate::backend::Account;
use crate::theme::motion;
use crate::theme::tokens::{frame, shape};

/// How wide the menu is, at the default text size.
const WIDTH: f32 = 320.0;

/// How tall a row of the menu is, at the default text size.
const ROW: f32 = 44.0;

/// The pictures of the other accounts beside "Switch account": this many at
/// most, and a count of the rest.
const STACKED: usize = 3;

/// How far the band's right edge is from the account's picture.
fn from_the_right() -> f32 {
    if cfg!(target_os = "macos") {
        frame::SIDE
    } else {
        super::frame::CAPTION_WIDTH * 3.0 + 10.0
    }
}

/// One thing the menu does: a row with a drawing or pictures at its left.
fn item(id: &'static str, label: String, look: &Look) -> Button {
    let palette = look.palette;
    Button::new(id)
        .role(Role::MenuItem)
        .accessibility_label(label)
        .w_full()
        .h(sp(ROW))
        .px(sp(10.))
        .gap(sp(10.))
        .justify_start()
        .rounded(px(shape::CONTROL_RADIUS))
        .text_size(sp(14.))
        .text_color(color(palette.text))
        .hover(move |style| style.bg(color(palette.frame)))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
}

/// Who the menu is about: the picture a little larger, centered beside the
/// real name, the username under it as every screen writes one, and the bio
/// in three lines at most.
fn header(account: &Account, look: &Look, shown: &parts::Shown) -> Div {
    let palette = look.palette;
    let named = handle(&account.named);
    let full = account
        .full_name
        .clone()
        .filter(|name| !name.trim().is_empty());
    let username = parts::username(look)
        .text_size(sp(if full.is_some() { 14. } else { 15. }))
        .when(full.is_none(), |username| {
            username.font_weight(FontWeight::BOLD)
        })
        .truncate()
        .id("menu-username-line")
        .test_support()
        .child(parts::said("menu-username", named));
    div()
        .flex()
        .items_center()
        .gap(sp(12.))
        .p(sp(10.))
        .child(
            div()
                .id("menu-photo")
                .test_support()
                .role(Role::Image)
                .aria_label(parts::photo_label(account, look))
                .flex_none()
                .child(parts::avatar(
                    account,
                    56.,
                    look,
                    parts::shown_of(shown, account),
                )),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .children(full.map(|name| {
                    div()
                        .id("menu-name-line")
                        .test_support()
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(15.))
                        .text_color(color(palette.text))
                        .truncate()
                        .child(parts::said("menu-name", name))
                }))
                .child(username)
                .children(account.biography.clone().map(|bio| {
                    div()
                        .id("menu-bio-line")
                        .test_support()
                        .text_size(sp(13.))
                        .line_height(sp(18.))
                        .text_color(color(palette.secondary))
                        .line_clamp(3)
                        .child(parts::said("menu-bio", bio))
                })),
        )
}

/// The pictures of the other accounts, one over the edge of the next.
fn stack(others: &[&Account], look: &Look, shown: &parts::Shown) -> Div {
    let palette = look.palette;
    let more = others.len().saturating_sub(STACKED);
    div()
        .flex()
        .items_center()
        .flex_none()
        .children(others.iter().take(STACKED).enumerate().map(|(at, other)| {
            div()
                .when(at > 0, |picture| picture.ml(px(-8.)))
                .rounded_full()
                .border_2()
                .border_color(color(palette.panel))
                .child(parts::avatar(
                    other,
                    22.,
                    look,
                    parts::shown_of(shown, other),
                ))
        }))
        .when(more > 0, |stack| {
            stack.child(
                div()
                    .ml(sp(4.))
                    .text_size(sp(12.))
                    .text_color(color(palette.secondary))
                    .child(look.tr_with("account-more", &[("count", &more.to_string())])),
            )
        })
}

/// The menu and what is under it, when it is open.
pub fn render(
    shell: &mut Shell,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let palette = look.palette;
    let Some(menu) = shell.menu.as_ref() else {
        return Vec::new();
    };
    let Some(account) = shell.active().cloned() else {
        return Vec::new();
    };
    let (trap, switching) = (menu.focus.clone(), menu.switching);
    let shown = shell.pictures_shown().clone();
    let others: Vec<Account> = shell
        .accounts()
        .iter()
        .filter(|other| !other.active)
        .cloned()
        .collect();

    // How far the other accounts' pictures have come from beside "Switch
    // account" to beside their own rows.
    let arrived = switching.map(|since| {
        let moved = look.moved(motion::ENTER, 1.0, 1.0);
        let at = (since.elapsed().as_secs_f32() / moved.lasts.as_secs_f32()).min(1.0);
        if at < 1.0 {
            crate::ui::frames::ask(window);
        }
        (motion::ease_out(at), moved.travels)
    });

    let accounts_row = if others.is_empty() {
        item("menu-add", look.tr("account-add"), look)
            .child(icon(assets::ADD, 18.).text_color(color(palette.text)))
            .child(look.tr("account-add"))
            .on_click(cx.listener(|shell, _, window, cx| shell.add_an_account(window, cx)))
    } else {
        let stacked: Vec<&Account> = others.iter().collect();
        item("menu-switch", look.tr("account-switch"), look)
            .aria_expanded(switching.is_some())
            .child(
                // The pictures leave for their rows once the list is open.
                stack(&stacked, look, &shown).when(switching.is_some(), |stack| stack.invisible()),
            )
            .child(look.tr("account-switch"))
            .child(div().flex_1())
            .child(icon(assets::CHEVRON, 16.).text_color(color(palette.secondary)))
            .on_click(cx.listener(|shell, _, _, cx| shell.list_the_other_accounts(cx)))
    };

    let row_height = ROW * look.settings.text_scale();
    let listed = arrived.map(|(at, travels)| {
        div()
            .flex()
            .flex_col()
            .children(others.iter().enumerate().map(|(row, other)| {
                let pk = other.named.pk;
                let named = handle(&other.named);
                // From its place in the stack, above, to its own row.
                let rise = -(row_height * (row as f32 + 1.0)) * (1.0 - at) * travels;
                let slide = (row.min(STACKED - 1) as f32 * 14.0) * (1.0 - at) * travels;
                Button::new(("menu-account", row))
                    .role(Role::MenuItem)
                    .accessibility_label(named.clone())
                    .w_full()
                    .h(sp(ROW))
                    .px(sp(10.))
                    .gap(sp(10.))
                    .justify_start()
                    .rounded(px(shape::CONTROL_RADIUS))
                    .text_color(color(palette.text))
                    .hover(move |style| style.bg(color(palette.frame)))
                    .focus_visible(parts::focus_ring(&palette))
                    .cursor_pointer()
                    .child(
                        div()
                            .relative()
                            .top(px(rise))
                            .left(px(slide))
                            .child(parts::avatar(
                                other,
                                28.,
                                look,
                                parts::shown_of(&shown, other),
                            )),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .opacity(at)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_size(sp(14.))
                                    .truncate()
                                    .child(named),
                            )
                            .children(other.full_name.clone().map(|name| {
                                div()
                                    .text_size(sp(12.))
                                    .text_color(color(palette.secondary))
                                    .truncate()
                                    .child(name)
                            })),
                    )
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.switch_to(pk, window, cx);
                    }))
            }))
            .child(
                item("menu-add", look.tr("account-add-another"), look)
                    .opacity(at)
                    .child(icon(assets::ADD, 18.).text_color(color(palette.text)))
                    .child(look.tr("account-add-another"))
                    .on_click(cx.listener(|shell, _, window, cx| shell.add_an_account(window, cx))),
            )
    });

    let shortcut = super::SETTINGS_SHORTCUT;
    let separator = || div().h(px(1.)).my(sp(6.)).bg(color(palette.line));

    let menu = div()
        .id("account-menu")
        .test_support()
        .role(Role::Menu)
        .aria_label(look.tr("account-menu"))
        .key_context("SnobMenu")
        .on_action(cx.listener(|shell, _: &super::MenuUp, window, cx| {
            shell.move_in_the_menu(false, window, cx);
        }))
        .on_action(cx.listener(|shell, _: &super::MenuDown, window, cx| {
            shell.move_in_the_menu(true, window, cx);
        }))
        .occlude()
        .absolute()
        .top(px(frame::BAND - 2.))
        .right(px(from_the_right()))
        .w(sp(WIDTH))
        .max_w(px(
            f32::from(window.viewport_size().width) - 2. * frame::SIDE
        ))
        .p(sp(6.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg();
    let items = div()
        .flex()
        .flex_col()
        .child(header(&account, look, &shown))
        .child(
            item("menu-profile", look.tr("account-profile"), look)
                .child(icon(assets::PROFILE, 18.).text_color(color(palette.text)))
                .child(look.tr("account-profile"))
                .on_click(cx.listener(|shell, _, window, cx| shell.open_own_profile(window, cx))),
        )
        .child(separator())
        .child(accounts_row)
        .children(listed)
        .child(separator())
        .child(
            item("menu-settings", look.tr("account-settings"), look)
                .aria_keyshortcuts(shortcut)
                .child(icon(assets::SETTINGS, 18.).text_color(color(palette.text)))
                .child(look.tr("account-settings"))
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(sp(12.))
                        .text_color(color(palette.secondary))
                        .child(shortcut),
                )
                .on_click(cx.listener(|shell, _, window, cx| shell.open_settings(window, cx))),
        )
        .child(
            item("menu-sign-out", look.tr("account-sign-out"), look)
                .child(icon(assets::SIGN_OUT, 18.).text_color(color(palette.text)))
                .child(look.tr("account-sign-out"))
                .on_click(cx.listener(|shell, _, window, cx| shell.ask_to_sign_out(window, cx))),
        )
        .focus_trap("account-menu-items", &trap);
    let menu = menu.child(items);

    vec![
        // A press anywhere else closes it.
        div()
            .id("menu-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, _, window, cx| shell.close_the_menu(window, cx)),
            )
            .into_any_element(),
        menu.into_any_element(),
    ]
}

/// The question signing out asks first, when it is being asked.
pub fn sign_out_question(
    shell: &mut Shell,
    look: &Look,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let palette = look.palette;
    let asking = shell.signing_out.as_ref()?;
    let (account, trap) = (handle(&asking.account), asking.focus.clone());
    let question = look.tr_with("sign-out-title", &[("account", &account)]);
    let body = look.tr("sign-out-body");

    let dialog = div()
        .id("sign-out")
        .test_support()
        .role(Role::AlertDialog)
        .aria_label(question.clone())
        .aria_description(body.clone())
        .occlude()
        .w(sp(440.))
        .max_w_full()
        .p(sp(24.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg();
    let asked = div()
        .flex()
        .flex_col()
        .gap(sp(14.))
        .child(
            div()
                .id("sign-out-title")
                .test_support()
                .role(Role::Heading)
                .aria_level(2)
                .aria_label(question.clone())
                .font_weight(FontWeight::BOLD)
                .text_size(sp(18.))
                .line_height(sp(26.))
                .text_color(color(palette.text))
                .child(question),
        )
        .child(
            div()
                .text_size(sp(14.))
                .line_height(sp(21.))
                .text_color(color(palette.secondary))
                .child(parts::said("sign-out-body", body)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(sp(10.))
                .pt(sp(6.))
                .child(
                    parts::button(
                        "sign-out-cancel",
                        look.tr("cancel"),
                        Kind::Secondary,
                        true,
                        look,
                    )
                    .track_focus(&asking.cancel)
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.keep_signed_in(window, cx);
                    })),
                )
                .child(
                    parts::button(
                        "sign-out-confirm",
                        look.tr("sign-out-confirm"),
                        Kind::Primary,
                        true,
                        look,
                    )
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.sign_out(window, cx);
                    })),
                ),
        )
        .focus_trap("sign-out-answers", &trap);
    let dialog = dialog.child(asked);

    Some(
        div()
            .id("sign-out-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(frame::SIDE * 2.))
            .bg(color_at(palette.frame, 0.72))
            .child(dialog)
            .into_any_element(),
    )
}
