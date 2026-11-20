//! What opens over a profile, the frame included: the press anywhere that
//! closes a small menu, the question a list of another account asks first,
//! "About this account", and the picture large over a dark backdrop. **Each
//! keeps focus inside it while it is open, closes on Escape or a press
//! outside, and hands focus back to what opened it** (`Shell::dismiss`).

use gpui_kit::base::{Button, FocusTrapElement as _, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, CursorStyle, FontWeight, MouseButton, ObjectFit, Role, ScrollDelta,
    Window, div, img, px,
};

use super::assets;
use super::look::{Look, color, color_at, sp};
use super::parts::{self, Kind, icon};
use super::screens::handle;
use super::screens::profile::{as_account, grouped};
use super::screens::words;
use super::shell::{Shell, eased};
use crate::state::profile::{About, AboutRow, Side};
use crate::theme::motion;
use crate::theme::tokens::{Rgb, frame, shape};

/// The backdrop under the picture: dark in either theme, as a photo is
/// looked at.
const BACKDROP: Rgb = Rgb::hex(0x0b0b0f);

/// How much of the window's shorter side the picture takes when it fits.
const FITS: f32 = 0.72;

/// The largest the picture is drawn when it fits, in logical pixels.
const FITS_AT_MOST: f32 = 720.0;

/// Everything of the pages that is open over the screen, in the order it is
/// drawn.
pub fn render(
    shell: &mut Shell,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let mut over = Vec::new();
    if shell.pages.popup.is_some() {
        // A press anywhere but the menu closes it; the menu itself is drawn
        // over everything (`screens::profile`).
        over.push(
            div()
                .id("profile-menu-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|shell, _, window, cx| shell.close_the_popup(window, cx)),
                )
                .into_any_element(),
        );
    }
    over.extend(question(shell, look, cx));
    over.extend(about(shell, look, cx));
    over.extend(viewer(shell, look, window, cx));
    over.extend(super::screens::automation_dialogs(shell, look, cx));
    over
}

/// The question reading another account's list asks before anything of it
/// is sent: **only a yes reads it**.
fn question(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Option<AnyElement> {
    let palette = look.palette;
    let asking = shell.pages.asking.as_ref()?;
    let account = handle(&asking.owner);
    let title = look.tr_with(
        match asking.side {
            Side::Followers => "list-question-followers",
            Side::Following => "list-question-following",
        },
        &[("account", &account)],
    );
    let body = look.tr("list-question-body");
    let trap = asking.focus.clone();

    let dialog = div()
        .id("list-question")
        .test_support()
        .role(Role::AlertDialog)
        .aria_label(title.clone())
        .aria_description(body.clone())
        .occlude()
        .w(sp(460.))
        .max_w_full()
        .p(sp(24.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(14.))
                .child(
                    div()
                        .id("list-question-title")
                        .test_support()
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(title.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(18.))
                        .line_height(sp(26.))
                        .text_color(color(palette.text))
                        .child(title),
                )
                .child(
                    div()
                        .text_size(sp(14.))
                        .line_height(sp(21.))
                        .text_color(color(palette.secondary))
                        .child(parts::said("list-question-body", body)),
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
                                "list-question-cancel",
                                look.tr("cancel"),
                                Kind::Secondary,
                                true,
                                look,
                            )
                            .track_focus(&asking.cancel)
                            .on_click(cx.listener(
                                |shell, _, window, cx| {
                                    shell.keep_the_list_closed(window, cx);
                                },
                            )),
                        )
                        .child(
                            parts::button(
                                "list-question-confirm",
                                look.tr("list-question-confirm"),
                                Kind::Primary,
                                true,
                                look,
                            )
                            .on_click(cx.listener(
                                |shell, _, window, cx| {
                                    shell.read_the_list(window, cx);
                                },
                            )),
                        ),
                )
                .focus_trap("list-question-answers", &trap),
        );

    Some(
        div()
            .id("list-question-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(frame::SIDE * 2.))
            .bg(color_at(palette.frame, 0.72))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, _, window, cx| shell.keep_the_list_closed(window, cx)),
            )
            .child(dialog)
            .into_any_element(),
    )
}

/// "About this account": the picture, the @username and what Instagram's
/// own app tells of the account. **snob reads none of it yet and makes none
/// of it up**: each row says it is not read until the engine reads it.
fn about(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Option<AnyElement> {
    let palette = look.palette;
    let about = shell.pages.about.as_ref()?;
    let title = look.tr("profile-about");
    let account = as_account(
        &about.who,
        about.full_name.as_deref(),
        about.picture.clone(),
    );
    let rows = AboutRow::ALL.into_iter().enumerate().map(|(at, row)| {
        let (label, value) = about_row(&about.known, row, look);
        div()
            .flex()
            .flex_wrap()
            .justify_between()
            .gap_x(sp(16.))
            .py(sp(10.))
            .border_t_1()
            .border_color(color(palette.line))
            .text_size(sp(14.))
            .line_height(sp(21.))
            .child(
                div()
                    .id(("profile-about-label", at))
                    .test_support()
                    .role(Role::Label)
                    .aria_label(label.clone())
                    .text_color(color(palette.text))
                    .child(label),
            )
            .child(
                div()
                    .id(("profile-about-value", at))
                    .test_support()
                    .role(Role::Label)
                    .aria_label(value.clone())
                    .text_color(color(palette.secondary))
                    .child(value),
            )
    });

    let dialog = div()
        .id("profile-about")
        .test_support()
        .role(Role::Dialog)
        .aria_label(title.clone())
        .occlude()
        .w(sp(400.))
        .max_w_full()
        .p(sp(24.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(sp(14.))
                .child(
                    div()
                        .id("profile-about-title")
                        .test_support()
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(title.clone())
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(18.))
                        .line_height(sp(26.))
                        .text_color(color(palette.text))
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(sp(8.))
                        .child(parts::avatar(&account, 72., look, 1.0))
                        .child(
                            div()
                                .id("profile-about-username")
                                .test_support()
                                .role(Role::Label)
                                .aria_label(handle(&about.who))
                                .child(
                                    parts::username(look)
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_size(sp(16.))
                                        .line_height(sp(24.))
                                        .child(handle(&about.who)),
                                ),
                        ),
                )
                .child(div().flex().flex_col().children(rows))
                .child(
                    div()
                        .text_size(sp(13.))
                        .line_height(sp(19.))
                        .text_color(color(palette.secondary))
                        .child(parts::said(
                            "profile-about-note",
                            look.tr("profile-about-note"),
                        )),
                )
                .child(
                    div().flex().justify_end().child(
                        parts::button(
                            "profile-about-close",
                            look.tr("profile-about-close"),
                            Kind::Secondary,
                            true,
                            look,
                        )
                        .track_focus(&about.close)
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.close_about_the_account(window, cx);
                        })),
                    ),
                )
                .focus_trap("profile-about-items", &about.focus),
        );

    Some(
        div()
            .id("profile-about-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(frame::SIDE * 2.))
            .bg(color_at(palette.frame, 0.72))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, _, window, cx| shell.close_about_the_account(window, cx)),
            )
            .child(dialog)
            .into_any_element(),
    )
}

/// What a row of "About this account" is called, and what it says: the
/// value once read, and that it is not read until then.
fn about_row(known: &About, row: AboutRow, look: &Look) -> (String, String) {
    let label = look.tr(match row {
        AboutRow::Joined => "profile-about-joined",
        AboutRow::BasedIn => "profile-about-based",
        AboutRow::UsernameChanges => "profile-about-usernames",
    });
    let value = match row {
        AboutRow::Joined => known.joined.map(|joined| {
            look.tr_with(
                "profile-about-joined-in",
                &[
                    ("month", &words::month_words(joined.month, look)),
                    ("year", &joined.year.to_string()),
                ],
            )
        }),
        AboutRow::BasedIn => known.based_in.as_deref().map(snob_core::model::printable),
        AboutRow::UsernameChanges => known
            .username_changes
            .map(|count| grouped(count, look.texts.language())),
    };
    (
        label,
        value.unwrap_or_else(|| look.tr("profile-about-unread")),
    )
}

/// A round button over the dark backdrop, its drawing light whatever the
/// theme, its name as its tip.
fn over_button(id: &'static str, label: String, drawing: &'static str, look: &Look) -> Button {
    let palette = look.palette;
    Button::new(id)
        .accessibility_label(label.clone())
        .tooltip(parts::tip(label))
        .size(sp(40.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(color_at(Rgb::hex(0xffffff), 0.12))
        .hover(|style| style.bg(color_at(Rgb::hex(0xffffff), 0.22)))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .child(icon(drawing, 18.).text_color(color(Rgb::hex(0xffffff))))
}

/// The picture large over the screen: the wheel zooms it about the pointer,
/// a drag moves it once zoomed, a double click fits it again, a right click offers to copy it or save it
/// where the person chooses, and the buttons above it save it in the
/// downloads, copy it, or close it.
fn viewer(
    shell: &mut Shell,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let palette = look.palette;
    let viewer = shell.pages.viewer.as_ref()?;
    let viewport = window.viewport_size();
    let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
    let fit = (width.min(height - frame::BAND) * FITS).clamp(80.0, FITS_AT_MOST);

    // It comes in from a little smaller, the backdrop darkening with it.
    let moved = look.moved(motion::ENTER, 0.0, 0.94);
    let came = eased(viewer.since, moved.lasts, window);
    let grown = moved.scales + (1.0 - moved.scales) * came;
    let zoom = viewer.zoom;
    let drawn = fit * zoom.scale * grown;
    let left = (width - drawn) / 2.0 + zoom.x;
    let top = (height - drawn) / 2.0 + zoom.y;
    let fit_top = (height - fit) / 2.0;

    let named = viewer.who.clone();
    let username = named
        .username
        .clone()
        .unwrap_or_else(|| named.pk.to_string());
    let account = as_account(&named, None, viewer.small.clone());
    let shown = viewer.large.clone().or_else(|| viewer.small.clone());
    let fallback = {
        let account = account.clone();
        let look = look.clone();
        move || {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(parts::avatar(&account, drawn * 0.8, &look, 1.0))
                .into_any_element()
        }
    };
    let held = viewer.dragging.is_some();
    let zoomed = zoom.zoomed();

    let picture = div()
        .id("viewer-picture")
        .test_support()
        .role(Role::Image)
        .aria_label(look.tr_with("account-photo", &[("username", &username)]))
        .occlude()
        .absolute()
        .left(px(left))
        .top(px(top))
        .size(px(drawn))
        .rounded(px(if zoomed { 0.0 } else { 10.0 }))
        .overflow_hidden()
        .when(zoomed, |picture| {
            picture.cursor(if held {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
        })
        .child(match shown {
            Some(path) => img(path)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .with_fallback(fallback)
                .into_any_element(),
            None => fallback(),
        })
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|shell, event: &gpui_kit::MouseDownEvent, _, cx| {
                // A second press back to where it fits; one holds it.
                if event.click_count >= 2 {
                    shell.fit_the_picture(cx);
                } else {
                    shell.hold_the_picture(Some(event.position), cx);
                }
                cx.stop_propagation();
            }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|shell, event: &gpui_kit::MouseDownEvent, window, cx| {
                shell.picture_menu(Some(event.position), window, cx);
                cx.stop_propagation();
            }),
        );

    let buttons = div()
        .absolute()
        .left_0()
        .right_0()
        .top(px((fit_top - 60.0).max(frame::BAND + 8.0)))
        .flex()
        .justify_center()
        .opacity(came)
        .child(
            div()
                .occlude()
                .flex()
                .gap(sp(10.))
                .child(
                    over_button(
                        "viewer-download",
                        look.tr("viewer-download"),
                        assets::DOWNLOAD,
                        look,
                    )
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.save_the_picture(false, window, cx);
                    })),
                )
                .child(
                    over_button("viewer-copy", look.tr("viewer-copy"), assets::COPY, look)
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.copy_the_picture(window, cx);
                        })),
                )
                .child(
                    over_button("viewer-close", look.tr("viewer-close"), assets::CLOSE, look)
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.close_the_picture(window, cx);
                        })),
                ),
        );

    let said = viewer.said.clone().map(|said| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(28.))
            .flex()
            .justify_center()
            .child(
                div()
                    .px(sp(14.))
                    .py(sp(6.))
                    .rounded_full()
                    .bg(color_at(Rgb::hex(0xffffff), 0.12))
                    .text_size(sp(13.))
                    .text_color(color(Rgb::hex(0xffffff)))
                    .child(said),
            )
    });

    let menu = viewer.menu.map(|at| {
        let item = |id: &'static str, label: String| {
            Button::new(id)
                .role(Role::MenuItem)
                .accessibility_label(label.clone())
                .w_full()
                .h(sp(36.))
                .px(sp(10.))
                .justify_start()
                .rounded(px(shape::CONTROL_RADIUS))
                .text_size(sp(14.))
                .text_color(color(palette.text))
                .hover(move |style| style.bg(color(palette.frame)))
                .focus_visible(parts::focus_ring(&palette))
                .cursor_pointer()
                .child(label)
        };
        let menu_width = 220.0 * look.settings.text_scale();
        div()
            .id("viewer-menu")
            .test_support()
            .role(Role::Menu)
            .aria_label(look.tr("viewer-menu"))
            .occlude()
            .absolute()
            .left(px(f32::from(at.x).min(width - menu_width - 8.0).max(8.0)))
            .top(px(f32::from(at.y).min(height - 100.0).max(8.0)))
            .w(px(menu_width))
            .p(sp(6.))
            .bg(color(palette.panel))
            .border_1()
            .border_color(color(palette.control_line))
            .rounded(px(shape::CARD_RADIUS))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|shell, _, window, cx| {
                shell.picture_menu(None, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        item("viewer-menu-copy", look.tr("viewer-menu-copy")).on_click(
                            cx.listener(|shell, _, window, cx| shell.copy_the_picture(window, cx)),
                        ),
                    )
                    .child(
                        item("viewer-menu-save", look.tr("viewer-menu-save")).on_click(
                            cx.listener(|shell, _, window, cx| {
                                shell.save_the_picture(true, window, cx);
                            }),
                        ),
                    )
                    .focus_trap("viewer-menu-items", &viewer.menu_focus),
            )
    });

    let size = (fit, fit);
    Some(
        div()
            .id("photo-viewer")
            .test_support()
            .role(Role::Dialog)
            .aria_label(look.tr_with("viewer-title", &[("username", &username)]))
            .absolute()
            .inset_0()
            .occlude()
            .bg(color_at(BACKDROP, 0.86 * came))
            // A press on the backdrop closes it; one on the picture, the
            // buttons or the menu is theirs.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, _, window, cx| shell.close_the_picture(window, cx)),
            )
            .on_mouse_move(
                cx.listener(move |shell, event: &gpui_kit::MouseMoveEvent, _, cx| {
                    shell.drag_the_picture(event.position, size, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|shell, _, _, cx| shell.hold_the_picture(None, cx)),
            )
            .on_scroll(
                cx.listener(move |shell, event: &gpui_kit::ScrollEvent, _, cx| {
                    let notches = match event.delta {
                        ScrollDelta::Lines(lines) => lines.y,
                        ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 50.0,
                    };
                    let at = (
                        f32::from(event.position.x) - width / 2.0,
                        f32::from(event.position.y) - height / 2.0,
                    );
                    shell.zoom_the_picture(notches, at, size, cx);
                }),
            )
            .child(picture)
            .child(buttons.focus_trap("viewer-buttons", &viewer.focus))
            .children(said)
            .children(menu)
            .into_any_element(),
    )
}
