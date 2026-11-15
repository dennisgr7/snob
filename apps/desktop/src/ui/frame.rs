//! The frame every screen is in: the gray that holds the panel, and its top
//! band with the status, the brand in its middle, the account and the
//! window's own buttons.
//!
//! **The band stays and the panel scrolls under it.** It is opaque while
//! nothing is under it and lets a little through once something is, with no
//! blur (GPUI has none per element), and never when the system asks for less
//! transparency.
//!
//! **The brand never leaves the middle**: the status and what is right of
//! the brand each get half of what the brand leaves. The status sits at the
//! band's left edge, and is cut short with an ellipsis before it pushes
//! anything.

use gpui_kit::base::{Button, ObservedElement, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, CursorStyle, Decorations, Div, FontWeight, MouseButton, ResizeEdge, Role,
    Stateful, Window, WindowControlArea, accesskit, div, px,
};

use super::assets;
use super::look::{Look, color, color_at, sp};
use super::parts::{self, icon};
use super::shell::Shell;
use crate::state::status::Status;
use crate::theme::fonts::{MONO, SANS};
use crate::theme::tokens::{Rgb, frame, shape};

/// How wide each of the window's buttons is where the app draws them:
/// Windows 11's own caption buttons.
pub const CAPTION_WIDTH: f32 = 46.0;

/// What macOS's own buttons take of the band's left.
const TRAFFIC_LIGHTS: f32 = 78.0;

/// How close to the window's edge a press resizes it, where the app is its
/// own decoration.
const RESIZE_EDGE: f32 = 6.0;

/// The local time of day `at`, as the band says it.
pub fn clock(at: snob_core::Epoch) -> String {
    chrono::DateTime::from_timestamp(at.get(), 0)
        .map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// The icon and the words of a status.
pub fn status_line(status: &Status, look: &Look) -> (&'static str, String) {
    match status {
        Status::Offline => (assets::OFFLINE, look.tr("status-offline")),
        Status::Paused { until } => (
            assets::PAUSED,
            look.tr_with("status-paused", &[("time", &clock(*until))]),
        ),
        Status::Resuming => (assets::RESUMING, look.tr("status-resuming")),
        Status::Downloading {
            percent: Some(percent),
        } => (
            assets::DOWNLOADING,
            look.tr_with("status-downloading", &[("percent", &percent.to_string())]),
        ),
        Status::Downloading { percent: None } => {
            (assets::DOWNLOADING, look.tr("status-downloading-unknown"))
        }
        Status::UpdateAvailable => (assets::UPDATE, look.tr("status-update-available")),
    }
}

/// The frame around `screen`.
pub fn render(
    shell: &mut Shell,
    screen: AnyElement,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let language = look.texts.language();
    let announced = shell.announced.clone();

    div()
        .id("window")
        .role(Role::Group)
        .aria_label(look.tr("window-title"))
        // The language the words are in, so a screen reader says them right.
        .a11y_synthetic_children(move |tree| {
            tree.parent_node().set_language(language);
        })
        .key_context("Snob")
        .on_action(cx.listener(|shell, _: &super::Dismiss, window, cx| {
            shell.dismiss(window, cx);
        }))
        .on_action(cx.listener(|shell, _: &super::OpenSettings, window, cx| {
            shell.open_settings(window, cx);
        }))
        .on_action(cx.listener(|shell, _: &super::OpenPalette, window, cx| {
            shell.toggle_the_palette(window, cx);
        }))
        .relative()
        .size_full()
        .bg(color(palette.frame))
        .font_family(SANS)
        .text_size(sp(15.))
        .text_color(color(palette.text))
        .child(
            div()
                .id("content")
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(&shell.scroll)
                .child(
                    div()
                        .min_h_full()
                        .flex()
                        .flex_col()
                        .pt(px(frame::BAND))
                        .px(px(frame::SIDE))
                        .pb(px(frame::SIDE))
                        .child(
                            div()
                                .id("panel")
                                .test_support()
                                .role(Role::Main)
                                .flex_1()
                                .flex()
                                .flex_col()
                                .bg(color(palette.panel))
                                .rounded(px(frame::PANEL_RADIUS))
                                .overflow_hidden()
                                .when(look.settings.more_contrast, |panel| {
                                    panel.border_1().border_color(color(palette.line))
                                })
                                .child(screen),
                        ),
                ),
        )
        .child(band(shell, &look, window, cx))
        .children(super::menu::render(shell, &look, window, cx))
        .children(super::menu::sign_out_question(shell, &look, cx))
        .children(super::palette::render(shell, &look, cx))
        // What a screen reader is told that no control says: a status that
        // changed, an error, the welcome. Not drawn.
        .child(
            div()
                .id("announcements")
                .test_support()
                .role(Role::Status)
                .aria_label(announced)
                .a11y_synthetic_children(|tree| {
                    tree.parent_node().set_live(accesskit::Live::Polite);
                })
                .absolute()
                .size(px(1.))
                .overflow_hidden(),
        )
        .children(resize_edges(window))
}

fn band(
    shell: &mut Shell,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> ObservedElement<Stateful<Div>> {
    let palette = look.palette;
    let over_content = shell.scrolled() > px(0.5);
    let alpha = if over_content && look.translucent() {
        frame::BAND_ALPHA_OVER_CONTENT
    } else {
        1.0
    };

    let status = shell.status().map(|status| status_line(status, look));
    let status = div()
        .id("status")
        .test_support()
        .role(Role::Status)
        .aria_label(
            status
                .as_ref()
                .map(|(_, said)| said.clone())
                .unwrap_or_default(),
        )
        .min_w_0()
        .flex()
        .items_center()
        .gap(sp(6.))
        .text_size(sp(12.5))
        .text_color(color(palette.secondary))
        .when_some(status, |line, (drawing, said)| {
            line.child(icon(drawing, 14.).text_color(color(palette.secondary)))
                .child(div().min_w_0().truncate().child(said))
        });

    let brand = div()
        .id("brand")
        .test_support()
        .role(Role::Label)
        .aria_label(look.tr("brand"))
        .aria_value(look.tr("brand"))
        .flex_none()
        .font_family(MONO)
        .font_weight(FontWeight::BOLD)
        .italic()
        .text_size(px(18.))
        .text_color(color(palette.accent))
        .child(look.tr("brand"));

    // Each side is half of what the brand leaves, whatever is in it: what
    // a side holds is padded inside it, since padding of its own would be
    // width the other side does not have.
    let side = || {
        div()
            .flex_1()
            .flex_basis(px(0.))
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .flex()
            .items_center()
    };
    // The status starts at the band's left edge, wherever the brand is.
    let left = side().justify_start().child(
        div()
            .min_w_0()
            .flex()
            .items_center()
            .pl(px(if cfg!(target_os = "macos") {
                TRAFFIC_LIGHTS
            } else {
                frame::SIDE
            }))
            .pr(px(14.))
            .child(status),
    );

    let right = side()
        .justify_end()
        .when(look.testing, |right| {
            right.child(
                div()
                    .id("test-badge")
                    .test_support()
                    .role(Role::Label)
                    .aria_label(look.tr("test-badge"))
                    .aria_value(look.tr("test-badge"))
                    .flex_none()
                    .ml(px(14.))
                    .mr(px(12.))
                    .px(px(6.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(color(palette.control_line))
                    .font_family(MONO)
                    .font_weight(FontWeight::BOLD)
                    .text_size(px(11.))
                    .text_color(color(palette.text))
                    .child(look.tr("test-badge")),
            )
        })
        .children(account(shell, look, cx))
        .children(caption_buttons(look, window));

    div()
        .id("band")
        .test_support()
        .role(Role::Banner)
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(frame::BAND))
        .bg(color_at(palette.frame, alpha))
        .when(look.settings.more_contrast, |band| {
            band.border_b_1().border_color(color(palette.line))
        })
        .flex()
        .items_center()
        // What the window is dragged by: under everything else in the band
        // and a sibling of the buttons, which is what lets Windows show its
        // snap layouts over the one that maximizes.
        .child(drag_area())
        .child(left)
        .child(brand)
        .child(right)
}

/// The part of the band the window is moved by, and maximized by with a
/// double click where the system does that.
fn drag_area() -> Stateful<Div> {
    let area = div().id("band-drag").absolute().inset_0();
    if cfg!(target_os = "windows") {
        area.window_control_area(WindowControlArea::Drag)
    } else {
        area.on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count == 2 {
                if cfg!(target_os = "macos") {
                    window.titlebar_double_click();
                } else {
                    window.zoom_window();
                }
            } else {
                window.start_window_move();
            }
        })
    }
}

/// The account in use, where its menu can be opened: its picture, right of
/// the brand, which opens the menu.
fn account(shell: &mut Shell, look: &Look, cx: &mut Context<Shell>) -> Option<AnyElement> {
    if !shell.has_a_menu() {
        return None;
    }
    let account = shell.active()?;
    let palette = look.palette;
    let size = shape::MIN_TARGET + 6.;
    Some(
        Button::new("account")
            .accessibility_label(look.tr("account-menu"))
            .aria_expanded(shell.menu.is_some())
            .track_focus(&shell.account_button)
            .occlude()
            .flex_none()
            .mr(px(if cfg!(target_os = "macos") {
                frame::SIDE
            } else {
                10.
            }))
            .size(px(size))
            .rounded_full()
            .cursor_pointer()
            .focus_visible(parts::focus_ring(&palette))
            .on_click(cx.listener(|shell, _, window, cx| shell.toggle_the_menu(window, cx)))
            .child(
                div()
                    .id("account-photo")
                    .test_support()
                    .role(Role::Image)
                    .aria_label(parts::photo_label(account, look))
                    .child(parts::avatar(account, size, look)),
            )
            .into_any_element(),
    )
}

/// One of the window's own buttons, where the app draws it.
fn caption(
    id: &'static str,
    label: String,
    area: WindowControlArea,
    look: &Look,
    act: fn(&mut Window),
) -> ObservedElement<Stateful<Div>> {
    let palette = look.palette;
    let close = area == WindowControlArea::Close;
    // Windows 11's own: a wash of the text's color, and its red under the
    // button that closes.
    let (hover, hover_text) = if close {
        (color(Rgb::hex(0xc42b1c)), color(Rgb::hex(0xffffff)))
    } else {
        (color_at(palette.text, 0.08), color(palette.text))
    };
    div()
        .id(id)
        .test_support()
        .role(Role::Button)
        .aria_label(label)
        .occlude()
        .flex_none()
        .w(px(CAPTION_WIDTH))
        .h(px(frame::BAND))
        .flex()
        .items_center()
        .justify_center()
        .text_color(color(palette.text))
        .hover(move |style| style.bg(hover).text_color(hover_text))
        // Windows acts on a press over one of these areas itself; the click
        // is how a screen reader, and every other system, presses it.
        .window_control_area(area)
        .on_click(move |_, window, _| act(window))
}

/// The window's own buttons, on the systems where the app draws them: on
/// macOS they are the system's, at the band's left.
fn caption_buttons(look: &Look, window: &mut Window) -> Vec<ObservedElement<Stateful<Div>>> {
    if cfg!(target_os = "macos") {
        return Vec::new();
    }
    let maximized = window.is_maximized();
    let (zoom_label, zoom_drawing, zoom_glyph) = if maximized {
        ("window-restore", assets::WINDOW_RESTORE, "\u{e923}")
    } else {
        ("window-maximize", assets::WINDOW_MAXIMIZE, "\u{e922}")
    };
    let button = |id, label: &str, area, drawing, glyph: &str, act: fn(&mut Window)| {
        let button = caption(id, look.tr(label), area, look, act);
        if cfg!(target_os = "windows") {
            // The system's own glyphs, from the font Windows 11 draws its
            // caption buttons with.
            button
                .font_family("Segoe Fluent Icons")
                .text_size(px(10.))
                .child(glyph.to_string())
        } else {
            button.child(icon(drawing, 16.))
        }
    };
    vec![
        button(
            "window-minimize",
            "window-minimize",
            WindowControlArea::Min,
            assets::WINDOW_MINIMIZE,
            "\u{e921}",
            |window| window.minimize_window(),
        ),
        button(
            "window-zoom",
            zoom_label,
            WindowControlArea::Max,
            zoom_drawing,
            zoom_glyph,
            |window| window.zoom_window(),
        ),
        button(
            "window-close",
            "window-close",
            WindowControlArea::Close,
            assets::WINDOW_CLOSE,
            "\u{e8bb}",
            |window| window.remove_window(),
        ),
    ]
}

/// The edges a window is resized by where the app is its own decoration (a
/// Linux desktop that draws none over the content): eight strips nobody
/// sees, each with the pointer that says what it does.
fn resize_edges(window: &mut Window) -> Vec<Stateful<Div>> {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) || window.is_maximized() {
        return Vec::new();
    }
    let strip = |id: &'static str, edge: ResizeEdge, cursor: CursorStyle| {
        div()
            .id(id)
            .absolute()
            .occlude()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, move |_, window, _| {
                window.start_window_resize(edge);
            })
    };
    let (thin, corner) = (px(RESIZE_EDGE), px(RESIZE_EDGE * 2.));
    vec![
        strip("resize-top", ResizeEdge::Top, CursorStyle::ResizeUpDown)
            .top_0()
            .left(corner)
            .right(corner)
            .h(thin),
        strip(
            "resize-bottom",
            ResizeEdge::Bottom,
            CursorStyle::ResizeUpDown,
        )
        .bottom_0()
        .left(corner)
        .right(corner)
        .h(thin),
        strip(
            "resize-left",
            ResizeEdge::Left,
            CursorStyle::ResizeLeftRight,
        )
        .left_0()
        .top(corner)
        .bottom(corner)
        .w(thin),
        strip(
            "resize-right",
            ResizeEdge::Right,
            CursorStyle::ResizeLeftRight,
        )
        .right_0()
        .top(corner)
        .bottom(corner)
        .w(thin),
        strip(
            "resize-top-left",
            ResizeEdge::TopLeft,
            CursorStyle::ResizeUpLeftDownRight,
        )
        .top_0()
        .left_0()
        .size(corner),
        strip(
            "resize-top-right",
            ResizeEdge::TopRight,
            CursorStyle::ResizeUpRightDownLeft,
        )
        .top_0()
        .right_0()
        .size(corner),
        strip(
            "resize-bottom-left",
            ResizeEdge::BottomLeft,
            CursorStyle::ResizeUpRightDownLeft,
        )
        .bottom_0()
        .left_0()
        .size(corner),
        strip(
            "resize-bottom-right",
            ResizeEdge::BottomRight,
            CursorStyle::ResizeUpLeftDownRight,
        )
        .bottom_0()
        .right_0()
        .size(corner),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Settings;

    #[test]
    fn a_status_is_said_in_words_with_its_numbers() {
        let look = Look::of(
            Settings {
                locale: "es".into(),
                ..Settings::default()
            },
            false,
        );
        assert_eq!(
            status_line(&Status::Offline, &look).1,
            "Sin conexión a Internet" // i18n-allow
        );
        assert_eq!(
            status_line(&Status::Downloading { percent: Some(63) }, &look).1,
            "Descargando actualización · 63 %" // i18n-allow
        );
        let paused = status_line(
            &Status::Paused {
                until: snob_core::Epoch::new(1_800_000_000),
            },
            &look,
        )
        .1;
        let time = clock(snob_core::Epoch::new(1_800_000_000));
        assert_eq!(time.len(), 5, "{time}");
        assert!(paused.ends_with(&time), "{paused}");
    }
}
