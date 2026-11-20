//! A profile: the viewer's own or another account's, as Instagram lays one
//! out. The picture, with a ring while a story is up; the names; the bio;
//! the two lists' counters, which open them; the highlights; and the posts
//! of three tabs, seen from afar as a grid or a list. **Nothing here opens
//! a post, a highlight or a story, and nothing is marked as seen**: the
//! page draws what the profile and one page of each tab said.
//!
//! Two layouts (`state::profile::Layout`): the card above the posts, the
//! whole page scrolling, or the card standing at the left and the posts at
//! its right scrolling alone. What opens over the page (the picture large,
//! the small menus, the question a list asks) is in `ui::overlays`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::{
    Align, Button, FocusTrapElement as _, POPUP_PRIORITY, Placement, Positioner, Scrollbar,
    TestSupportExt as _,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, Context, Div, FontWeight, ObjectFit, Pixels, Role, ScrollDelta,
    ScrollHandle, Window, deferred, div, img, px, relative,
};

use super::handle;
use crate::backend::{Account, Profile, Tile, TileKind, Tiles, Unread};
use crate::state::flow::Named;
use crate::state::profile::{self, Layout, Mark, Showing, Side, Tab};
use crate::theme::motion;
use crate::theme::tokens::{frame, shape};
use crate::ui::assets;
use crate::ui::frame::clock;
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Tone, icon};
use crate::ui::shell::{Phase, PopupKind, Shell, TabRead, eased};

/// The picture's size in the card above the posts, and in the card at the
/// left, in logical pixels.
const PICTURE: f32 = 132.0;
const PICTURE_SPLIT: f32 = 112.0;

/// How wide the card at the left is, at the default text size.
const CARD_WIDTH: f32 = 320.0;

/// The widest the page is with the card above the posts, at the default
/// text size: past it, the page stays in the middle.
const STACKED_WIDTH: f32 = 980.0;

/// How wide a post of the grid may grow before the grid takes another
/// column.
const WIDEST_TILE: f32 = 260.0;

/// How long after one tile the next starts coming in, and the most any
/// waits: a long page comes in at once after its first rows.
const TILE_STAGGER: Duration = Duration::from_millis(28);
const TILE_STAGGER_MOST: Duration = Duration::from_millis(340);

/// How far a hidden or unread profile's part is from the page's top.
const STATUS_PAD: f32 = 48.0;

/// A highlight's cover: a rounded upright rectangle a little taller than
/// three by four.
const HIGHLIGHT_WIDTH: f32 = 72.0;
const HIGHLIGHT_HEIGHT: f32 = 100.0;
const HIGHLIGHT_RADIUS: f32 = 12.0;

/// How wide the page must be, at the default text size, for the bar to say
/// in words how the posts are shown: under it, the buttons alone.
const NAMED_VIEW_FROM: f32 = 560.0;

/// Why something of a profile was not read, said as what happened and what
/// to do.
pub fn unread_words(why: &Unread, look: &Look) -> String {
    match why {
        Unread::Hidden => look.tr("profile-private"),
        Unread::Paused(Some(until)) => {
            look.tr_with("error-paused-until", &[("time", &clock(*until))])
        }
        Unread::Paused(None) => look.tr("error-paused"),
        Unread::NoSession => look.tr("profile-no-session"),
        Unread::Nobody => look.tr("profile-nobody"),
        Unread::Failed(_) => look.tr("profile-failed"),
    }
}

/// `count` with its thousands set apart as `language` writes them: a comma
/// in English, a point in Spanish from five digits on.
pub fn grouped(count: u64, language: &str) -> String {
    let digits = count.to_string();
    let (mark, from) = if language == "es" {
        ('.', 10_000)
    } else {
        (',', 1_000)
    };
    if count < from {
        return digits;
    }
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(mark);
        }
        out.push(digit);
    }
    out
}

/// An account as the picture parts draw it.
pub(crate) fn as_account(
    named: &Named,
    full_name: Option<&str>,
    picture: Option<std::path::PathBuf>,
) -> Account {
    Account {
        named: named.clone(),
        full_name: full_name.map(str::to_string),
        biography: None,
        picture,
        picture_kept_at: None,
        active: false,
    }
}

/// The username of `named`, or its number while none is known.
fn username(named: &Named) -> String {
    named
        .username
        .clone()
        .unwrap_or_else(|| named.pk.to_string())
}

/// What the page knows of its profile on this frame, taken out of the shell
/// so the parts can borrow it freely.
struct Shown {
    who: Named,
    profile: Option<Profile>,
    failed: Option<Unread>,
    tab: Tab,
    tab_from: (Tab, Instant),
    read: Option<TabRead>,
    /// The tab's first page the cache kept, shown until it is read.
    kept: Option<Tiles>,
    shown_since: Instant,
    read_at: Option<Instant>,
    highlights_since: Option<Instant>,
    scroll: ScrollHandle,
    highlights: ScrollHandle,
    picture_button: gpui_kit::FocusHandle,
    options_button: gpui_kit::FocusHandle,
    picture_drawn: Rc<Cell<Bounds<Pixels>>>,
    options_drawn: Rc<Cell<Bounds<Pixels>>>,
}

/// The profile shown.
pub fn render(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let Some(page) = shell.profile_page() else {
        return div().into_any_element();
    };
    let shown = Shown {
        who: page.who.clone(),
        profile: page.profile.clone(),
        failed: page.failed.clone(),
        tab: page.tab,
        tab_from: page.tab_from,
        read: page.tabs.get(&page.tab).cloned(),
        kept: page.kept_tabs.get(&page.tab).cloned(),
        shown_since: page.shown_since,
        read_at: page.read_at,
        highlights_since: page.highlights_since,
        scroll: page.scroll.clone(),
        highlights: page.highlights.clone(),
        picture_button: page.picture_button.clone(),
        options_button: page.options_button.clone(),
        picture_drawn: page.picture_drawn.clone(),
        options_drawn: page.options_drawn.clone(),
    };
    let layout = shell.pages.layout;
    let showing = shell.pages.showing;
    let showing_from = shell.pages.showing_from;
    let popup = shell.pages.popup.as_ref().map(|popup| popup.kind);

    // A layout changed comes in again, from a little under its place.
    let moved = look.moved(motion::ENTER, 10.0, 1.0);
    let settled = shell
        .pages
        .laid_out_at
        .map_or(1.0, |since| eased(since, moved.lasts, window));
    let (presence, rise) = (
        phase.presence * settled,
        phase.rise + moved.travels * (1.0 - settled),
    );

    // How wide the page is, and the posts have to be laid in.
    let scale = look.settings.text_scale();
    let page_room =
        f32::from(window.viewport_size().width) - frame::RAIL - frame::SIDE - 64.0 * scale;
    let page_room = match layout {
        Layout::Stacked => page_room.min(STACKED_WIDTH * scale),
        Layout::Split => page_room,
    };
    let room = match layout {
        Layout::Stacked => page_room,
        Layout::Split => page_room - CARD_WIDTH * scale,
    };
    let columns = profile::columns(room, WIDEST_TILE);

    let options = options(shell, &shown, popup, &look, window, cx);
    let tools = bar(
        layout,
        showing,
        showing_from,
        options,
        page_room >= NAMED_VIEW_FROM * scale,
        &look,
        window,
        cx,
    );
    let posts = posts(&shown, showing, showing_from, columns, &look, window, cx);

    let page = match layout {
        Layout::Stacked => div()
            .id("profile")
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .px(sp(32.))
            .pt(sp(18.))
            .pb(sp(40.))
            .child(
                div()
                    .w_full()
                    .max_w(sp(STACKED_WIDTH))
                    .flex()
                    .flex_col()
                    .gap(sp(22.))
                    .child(card(
                        shell,
                        &shown,
                        layout,
                        popup,
                        Some(tools),
                        &look,
                        window,
                        cx,
                    ))
                    .child(posts),
            ),
        Layout::Split => {
            let bar = Scrollbar::vertical(&shown.scroll)
                .id("profile-scrollbar")
                .viewport_from_layout()
                .styles(|styles| {
                    let palette = look.palette;
                    styles
                        .thumb(move |thumb| thumb.bg(color(palette.control_line)))
                        .thumb_hover(move |thumb| thumb.bg(color(palette.secondary)))
                        .thumb_active(move |thumb| thumb.bg(color(palette.secondary)))
                });
            div()
                .id("profile")
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .px(sp(32.))
                        .pt(sp(18.))
                        .pb(sp(12.))
                        .child(tools),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(
                            // The border is the column's, so the card's own
                            // box can reach past it for a focus ring.
                            div()
                                .flex_none()
                                .w(sp(CARD_WIDTH))
                                .h_full()
                                .flex()
                                .flex_col()
                                .border_r_1()
                                .border_color(color(look.palette.line))
                                .child(
                                    // Room above for a focus ring; the
                                    // padding below is room enough.
                                    div()
                                        .id("profile-card")
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .pt(px(shape::FOCUS_ROOM))
                                        .mt(px(-shape::FOCUS_ROOM))
                                        .pl(sp(32.))
                                        .pr(sp(20.))
                                        .pb(sp(32.))
                                        .child(card(
                                            shell, &shown, layout, popup, None, &look, window, cx,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .relative()
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .id("profile-posts")
                                        .test_support()
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .track_scroll(&shown.scroll)
                                        .pt(px(shape::FOCUS_ROOM))
                                        .mt(px(-shape::FOCUS_ROOM))
                                        .px(sp(24.))
                                        .pb(sp(40.))
                                        .child(posts),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .left_0()
                                        .right(px(6.))
                                        .child(bar),
                                ),
                        ),
                )
        }
    };

    page.relative()
        .top(px(rise))
        .opacity(presence)
        .into_any_element()
}

/// The bar, on one line: how the posts are shown, said in words when
/// `named` and chosen beside them, the layout, and the options. The way
/// back is the band's.
#[expect(
    clippy::too_many_arguments,
    reason = "the bar's state, the options built with their menu, and the view"
)]
fn bar(
    layout: Layout,
    showing: Showing,
    showing_from: Option<(Showing, Instant)>,
    options: Div,
    named: bool,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let grid = showing == Showing::Grid;
    let to_split = layout == Layout::Stacked;
    div()
        .id("profile-bar")
        .test_support()
        .flex()
        .flex_none()
        .items_center()
        .justify_end()
        .gap(sp(10.))
        .when(named, |bar| {
            bar.child(view_words(showing, showing_from, look, window))
        })
        .child(
            parts::segmented("profile-showing", look.tr("profile-showing"), look)
                .child(
                    showing_segment(Showing::Grid, grid, look).on_click(cx.listener(
                        |shell, _, _, cx| {
                            shell.show_posts_as(Showing::Grid, cx);
                        },
                    )),
                )
                .child(
                    showing_segment(Showing::List, !grid, look).on_click(cx.listener(
                        |shell, _, _, cx| {
                            shell.show_posts_as(Showing::List, cx);
                        },
                    )),
                ),
        )
        .child(
            layout_button(to_split, look)
                .on_click(cx.listener(|shell, _, _, cx| shell.change_the_layout(cx))),
        )
        .child(options)
        .into_any_element()
}

/// One of the two ways posts are shown, as a drawing with its tip.
fn showing_segment(showing: Showing, on: bool, look: &Look) -> Button {
    let (id, drawing) = match showing {
        Showing::Grid => ("profile-grid", assets::GRID),
        Showing::List => ("profile-list", assets::LIST),
    };
    parts::icon_segment(id, look.tr(id), drawing, on, look)
}

/// What puts the card beside the posts, or above them.
fn layout_button(to_split: bool, look: &Look) -> Button {
    parts::icon_button(
        "profile-layout",
        look.tr(if to_split {
            "profile-layout-split"
        } else {
            "profile-layout-stacked"
        }),
        if to_split {
            assets::LAYOUT_SPLIT
        } else {
            assets::LAYOUT_STACKED
        },
        true,
        look,
    )
    .w(sp(36.))
    .h(sp(36.))
}

/// What opens the options, enabled once the profile is known.
fn options_button(enabled: bool, look: &Look) -> Button {
    parts::icon_button(
        "profile-options",
        look.tr("profile-options"),
        assets::MORE,
        enabled,
        look,
    )
    .w(sp(34.))
    .h(sp(34.))
}

/// The options' button, kept where it was drawn, with its menu when open.
fn options(
    shell: &Shell,
    shown: &Shown,
    popup: Option<PopupKind>,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let button = options_button(shown.profile.is_some(), look)
        .track_focus(&shown.options_button)
        .aria_expanded(popup == Some(PopupKind::Options))
        .on_click(cx.listener(|shell, _, window, cx| {
            shell.open_a_popup(PopupKind::Options, window, cx);
        }));
    parts::drawn_at(
        &shown.options_drawn,
        popup == Some(PopupKind::Options),
        button,
    )
    .children(popup_menu(
        shell,
        PopupKind::Options,
        shown.options_drawn.get(),
        look,
        window,
        cx,
    ))
}

/// What a way of showing posts is called in words.
fn showing_words(showing: Showing, look: &Look) -> String {
    look.tr(match showing {
        Showing::Grid => "profile-showing-grid",
        Showing::List => "profile-showing-list",
    })
}

/// How the posts are shown, in words, crossing from the words of how they
/// were: the old ones leave upward and the new ones come from under them.
/// **As wide as the wider of the two at all times**, so nothing beside them
/// moves. Not a control: the buttons beside it carry the choice.
fn view_words(
    showing: Showing,
    from: Option<(Showing, Instant)>,
    look: &Look,
    window: &mut Window,
) -> AnyElement {
    let palette = look.palette;
    let moved = look.moved(motion::ENTER, 6.0, 1.0);
    let crossed = from.map_or(1.0, |(_, since)| eased(since, moved.lasts, window));
    let (leaving, coming) = profile::crossing(crossed);
    let words = |showing: Showing, shown: f32, rise: f32| {
        div()
            .absolute()
            .left_0()
            .right_0()
            .top(px(rise))
            .h_full()
            .flex()
            .items_center()
            .justify_end()
            .whitespace_nowrap()
            .opacity(shown)
            .child(showing_words(showing, look))
    };
    let other = match showing {
        Showing::Grid => Showing::List,
        Showing::List => Showing::Grid,
    };
    div()
        .id("profile-showing-words")
        .test_support()
        .aria_label(showing_words(showing, look))
        .relative()
        .flex_none()
        .h(sp(28.))
        .text_size(sp(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color(palette.secondary))
        // Both, in no height: what holds the widest of them.
        .child(
            div()
                .flex()
                .flex_col()
                .h(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .child(showing_words(Showing::Grid, look))
                .child(showing_words(Showing::List, look)),
        )
        .when(leaving > 0.0, |held| {
            held.child(words(other, leaving, -moved.travels * (1.0 - leaving)))
        })
        .child(words(showing, coming, moved.travels * (1.0 - coming)))
        .into_any_element()
}

/// The card: the picture, the names, the bio, the counters and the
/// highlights; across the page, with the bar beside the names, or down its
/// left.
#[expect(
    clippy::too_many_arguments,
    reason = "the card's state, the bar it may hold, and the view"
)]
fn card(
    shell: &Shell,
    shown: &Shown,
    layout: Layout,
    popup: Option<PopupKind>,
    bar: Option<AnyElement>,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let split = layout == Layout::Split;
    let named = shown
        .profile
        .as_ref()
        .map_or_else(|| shown.who.clone(), |profile| profile.named.clone());

    // What a read changed comes in: the ring, the counters, the highlights.
    let came = shown.read_at.map_or(1.0, |since| {
        eased(since, look.moved(motion::ENTER, 0.0, 1.0).lasts, window)
    });

    let picture = parts::drawn_at(
        &shown.picture_drawn,
        popup == Some(PopupKind::Picture),
        picture(shown, &named, split, popup, came, look, cx),
    )
    .children(popup_menu(
        shell,
        PopupKind::Picture,
        shown.picture_drawn.get(),
        look,
        window,
        cx,
    ));
    let names = names(shell, shown, &named, look, cx);
    let about = about(shown, look);
    let counters = counters(shown, came, look, cx);
    // The highlights come in when a read changes them, not when it says
    // again what the cache kept.
    let highlights_came = shown.highlights_since.map_or(1.0, |since| {
        eased(since, look.moved(motion::ENTER, 0.0, 1.0).lasts, window)
    });
    let highlights = highlights(shown, highlights_came, look, window);
    let failed = shown.failed.as_ref().map(|why| {
        let said = unread_words(why, look);
        div()
            .id("profile-failed")
            .test_support()
            .role(Role::Status)
            .aria_label(said.clone())
            .flex()
            .items_center()
            .gap(sp(8.))
            .text_size(sp(14.))
            .text_color(color(look.palette.error))
            .child(icon(assets::ERROR, 16.).text_color(color(look.palette.error)))
            .child(said)
    });

    if split {
        div()
            .flex()
            .flex_col()
            .gap(sp(16.))
            .pt(sp(8.))
            .child(div().flex().child(picture))
            .child(names)
            .children(failed)
            .child(about)
            .child(counters)
            .children(highlights)
    } else {
        div()
            .flex()
            .flex_col()
            .gap(sp(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(28.))
                    .child(picture)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(sp(10.))
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap(sp(10.))
                                    .child(div().flex_1().min_w_0().child(names))
                                    .children(bar),
                            )
                            .children(failed)
                            .child(about)
                            .child(counters),
                    ),
            )
            .children(highlights)
    }
}

/// The picture: a button that opens it large, or, with a story up, asks
/// which. **The ring says a story is up and that is all**: nothing of the
/// story is read.
fn picture(
    shown: &Shown,
    named: &Named,
    split: bool,
    popup: Option<PopupKind>,
    came: f32,
    look: &Look,
    cx: &mut Context<Shell>,
) -> Button {
    let palette = look.palette;
    let size = if split { PICTURE_SPLIT } else { PICTURE };
    let story = shown
        .profile
        .as_ref()
        .is_some_and(|profile| profile.story_up);
    let account = as_account(
        named,
        shown
            .profile
            .as_ref()
            .and_then(|profile| profile.full_name.as_deref()),
        shown
            .profile
            .as_ref()
            .and_then(|profile| profile.picture.clone()),
    );
    let name = username(named);
    let label = if story {
        look.tr_with("profile-picture-story", &[("username", &name)])
    } else {
        look.tr_with("profile-picture-open", &[("username", &name)])
    };
    // The ring is drawn outside the picture with a gap of the panel's color,
    // as Instagram draws it; it fades in once the read says a story is up.
    let ring = 3.0;
    let gap = 3.0;
    Button::new("profile-picture")
        .accessibility_label(label.clone())
        .tooltip(parts::tip(label))
        .disabled(shown.profile.is_none())
        .track_focus(&shown.picture_button)
        .when(story, |button| {
            button.aria_expanded(popup == Some(PopupKind::Picture))
        })
        .flex_none()
        .size(px(size + 2.0 * (ring + gap)))
        .p(px(0.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .border(px(ring))
        .border_color(if story {
            color_at(palette.underline, came)
        } else {
            gpui_kit::transparent_black()
        })
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .hover(|style| style.opacity(0.9))
        .child(parts::avatar(&account, size, look, 1.0))
        .on_click(cx.listener(|shell, _, window, cx| shell.click_the_picture(window, cx)))
}

/// A small menu under what opened it, over everything else: the story or
/// the picture, or what can be copied of the account. **Copying is all the
/// options do**: no follow, no write. The picture's starts at its left edge
/// and the options' ends at their right one.
fn popup_menu(
    shell: &Shell,
    kind: PopupKind,
    under: Bounds<Pixels>,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let palette = look.palette;
    let popup = shell
        .pages
        .popup
        .as_ref()
        .filter(|popup| popup.kind == kind)?;
    let moved = look.moved(motion::INTERACTION, 6.0, 1.0);
    let came = eased(popup.since, moved.lasts, window);
    let item = |id: &'static str, label: String, drawing: &'static str| {
        parts::menu_item(id, label, drawing, look)
    };
    let items: Vec<Button> = match kind {
        PopupKind::Picture => vec![
            item(
                "profile-open-story",
                look.tr("profile-open-story"),
                assets::VIDEO,
            )
            .on_click(cx.listener(|shell, _, window, cx| shell.open_the_story(window, cx))),
            item(
                "profile-open-picture",
                look.tr("profile-open-picture"),
                assets::PHOTO,
            )
            .on_click(cx.listener(|shell, _, window, cx| shell.open_the_picture(window, cx))),
        ],
        PopupKind::Options => vec![
            item(
                "profile-copy-username",
                look.tr("profile-copy-username"),
                assets::COPY,
            )
            .on_click(cx.listener(|shell, _, window, cx| {
                shell.copy_of_the_profile("username", window, cx);
            })),
            item(
                "profile-copy-link",
                look.tr("profile-copy-link"),
                assets::COPY,
            )
            .on_click(cx.listener(|shell, _, window, cx| {
                shell.copy_of_the_profile("link", window, cx);
            })),
            item("profile-copy-id", look.tr("profile-copy-id"), assets::COPY).on_click(
                cx.listener(|shell, _, window, cx| {
                    shell.copy_of_the_profile("id", window, cx);
                }),
            ),
        ],
    };
    let name = look.tr(match kind {
        PopupKind::Picture => "profile-picture-menu",
        PopupKind::Options => "profile-options-menu",
    });
    let menu = div()
        .id("profile-menu")
        .test_support()
        .role(Role::Menu)
        .aria_label(name)
        .occlude()
        .w(sp(250.))
        .p(sp(6.))
        .bg(color(palette.panel))
        .border_1()
        .border_color(color(palette.control_line))
        .rounded(px(shape::CARD_RADIUS))
        .shadow_lg()
        .relative()
        .top(px(-moved.travels * (1.0 - came)))
        .opacity(came)
        .child(
            div()
                .flex()
                .flex_col()
                .children(items)
                .focus_trap("profile-menu-items", &popup.focus),
        );
    let align = match kind {
        PopupKind::Picture => Align::Start,
        PopupKind::Options => Align::End,
    };
    Some(
        deferred(
            Positioner::side(under)
                .placement(Placement::Bottom)
                .align(align)
                .offset(px(6.))
                .margin(px(8.))
                .child(menu),
        )
        .with_priority(POPUP_PRIORITY)
        .into_any_element(),
    )
}

/// The names: the real name large and the username beside it, underlined in
/// pink as the welcome has it, which a click copies; and the marks under
/// them. **The page's title**, where focus goes as it comes in.
fn names(
    shell: &Shell,
    shown: &Shown,
    named: &Named,
    look: &Look,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let palette = look.palette;
    let at = handle(named);
    let full = shown
        .profile
        .as_ref()
        .and_then(|profile| profile.full_name.clone())
        .filter(|name| !name.trim().is_empty());
    let verified = shown
        .profile
        .as_ref()
        .is_some_and(|profile| profile.is_verified);
    let private = shown
        .profile
        .as_ref()
        .is_some_and(|profile| profile.is_private);
    let heading = match &full {
        Some(full) => look.tr_with("profile-title", &[("name", full), ("account", &at)]),
        None => at.clone(),
    };
    let username = username_button(
        named,
        shown.profile.is_some(),
        if full.is_some() { 17. } else { 24. },
        look,
    )
    .on_click(cx.listener(|shell, _, window, cx| {
        shell.copy_of_the_profile("username", window, cx);
    }));
    let marks: Vec<Div> = shown
        .profile
        .as_ref()
        .map(|profile| profile::marks(verified, private, profile.you_follow, profile.follows_you))
        .unwrap_or_default()
        .into_iter()
        .map(|mark| {
            let (id, tone) = match mark {
                Mark::Verified => ("profile-verified", Tone::Verified),
                Mark::Private => ("profile-private-mark", Tone::Plain),
                Mark::Friend => ("profile-friend", Tone::Friend),
                Mark::FollowsYou => ("profile-follows-you", Tone::Plain),
            };
            parts::badge(id, look.tr(id), tone, look)
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(sp(6.))
        .child(
            div()
                .id("title")
                .test_support()
                .role(Role::Heading)
                .aria_level(1)
                .aria_label(heading)
                .track_focus(&shell.title)
                .flex()
                .flex_wrap()
                .items_baseline()
                .gap_x(sp(12.))
                .gap_y(sp(2.))
                .children(full.map(|full| {
                    div()
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(26.))
                        .line_height(sp(34.))
                        .text_color(color(palette.text))
                        .child(full)
                }))
                .child(username),
        )
        .when(!marks.is_empty(), |names| {
            names.child(div().flex().flex_wrap().gap(sp(6.)).children(marks))
        })
        .into_any_element()
}

/// The @username, drawn `size` large, as what copies it once the profile is
/// known.
fn username_button(named: &Named, known: bool, size: f32, look: &Look) -> Button {
    let palette = look.palette;
    let copy = look.tr_with("profile-username-copy", &[("username", &username(named))]);
    Button::new("profile-username")
        .accessibility_label(copy.clone())
        .tooltip(parts::tip(copy))
        .disabled(!known || named.username.is_none())
        .min_h(px(shape::MIN_TARGET))
        .min_w(px(shape::MIN_TARGET))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(shape::CONTROL_RADIUS))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .child(
            parts::username(look)
                .font_weight(FontWeight::MEDIUM)
                .text_size(sp(size))
                .child(handle(named)),
        )
}

/// The bio, the category and the link the profile names: words to read,
/// none of them opened.
fn about(shown: &Shown, look: &Look) -> Div {
    let palette = look.palette;
    let profile = shown.profile.as_ref();
    let category = profile.and_then(|profile| profile.category.clone());
    let bio = profile
        .and_then(|profile| profile.biography.clone())
        .filter(|bio| !bio.trim().is_empty());
    let link = profile.and_then(|profile| profile.external_url.clone());
    div()
        .flex()
        .flex_col()
        .gap(sp(4.))
        .children(category.map(|category| {
            div()
                .text_size(sp(13.))
                .text_color(color(palette.secondary))
                .child(parts::said("profile-category", category))
        }))
        .children(bio.map(|bio| {
            div()
                .text_size(sp(15.))
                .line_height(sp(22.))
                .text_color(color(palette.text))
                .child(parts::said("profile-bio", bio))
        }))
        .children(link.map(|link| {
            div()
                .text_size(sp(14.))
                .text_color(color(palette.emphasis))
                .truncate()
                .child(parts::said("profile-link", link))
        }))
}

/// The two lists' counters, each a button that opens its list.
fn counters(shown: &Shown, came: f32, look: &Look, cx: &mut Context<Shell>) -> Div {
    let palette = look.palette;
    let language = look.texts.language();
    let profile = shown.profile.as_ref();
    let open = profile.is_some_and(|profile| profile.visible == Some(true));
    let counter = |side: Side| {
        let (id, count, word, label) = match side {
            Side::Followers => (
                "profile-followers",
                profile.and_then(|profile| profile.followers),
                "profile-followers",
                "profile-followers-open",
            ),
            Side::Following => (
                "profile-following",
                profile.and_then(|profile| profile.following),
                "profile-following",
                "profile-following-open",
            ),
        };
        let number = count.map_or_else(|| "\u{2014}".to_string(), |n| grouped(n, language));
        Button::new(id)
            .accessibility_label(look.tr_with(label, &[("count", &number)]))
            .disabled(!open)
            .flex()
            .items_baseline()
            .gap(sp(6.))
            .min_h(px(shape::MIN_TARGET))
            .px(sp(10.))
            .py(sp(6.))
            .rounded(px(shape::CONTROL_RADIUS))
            .focus_visible(parts::focus_ring(&palette))
            .when(open, |button| {
                button
                    .cursor_pointer()
                    .hover(move |style| style.bg(color(palette.frame)))
            })
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_size(sp(17.))
                    .text_color(color(palette.text))
                    .opacity(0.4 + 0.6 * came)
                    .child(number),
            )
            .child(
                div()
                    .text_size(sp(14.))
                    .text_color(color(palette.secondary))
                    .child(look.tr(word)),
            )
            .on_click(cx.listener(move |shell, _, window, cx| {
                shell.open_a_list(side, window, cx);
            }))
    };
    div()
        .flex()
        .flex_wrap()
        .gap(sp(6.))
        .ml(sp(-10.))
        .child(counter(Side::Followers))
        .child(counter(Side::Following))
}

/// The highlights in a row that scrolls sideways: their covers, rounded
/// upright rectangles, and their titles under them, none opened. The wheel
/// turned over them moves them while they can move.
fn highlights(shown: &Shown, came: f32, look: &Look, window: &mut Window) -> Option<AnyElement> {
    let palette = look.palette;
    let profile = shown.profile.as_ref()?;
    if profile.highlights.is_empty() {
        return None;
    }
    let handle = shown.highlights.clone();
    let moved = look.moved(motion::ENTER, 8.0, 1.0);
    let items = profile
        .highlights
        .iter()
        .enumerate()
        .map(|(at, highlight)| {
            // Each comes in a little after the one before it.
            let delay = Duration::from_millis(40) * at.min(10) as u32;
            let here = shown.highlights_since.map_or(1.0, |since| {
                let since = since + delay;
                if Instant::now() < since {
                    crate::ui::frames::ask(window);
                    0.0
                } else {
                    eased(since, moved.lasts, window)
                }
            });
            let letter: String = highlight
                .title
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            // **Given its proportions**: an image given none takes its own,
            // which win over its size, and a tall cover overflows its box.
            let (width, height) = (sp(HIGHLIGHT_WIDTH), sp(HIGHLIGHT_HEIGHT));
            let cover = div()
                .id(("profile-highlight-cover", at))
                .test_support()
                .flex_none()
                .w(width)
                .h(height)
                .rounded(px(HIGHLIGHT_RADIUS))
                .overflow_hidden()
                .child(match &highlight.cover {
                    Some(cover) => {
                        let letter = letter.clone();
                        img(cover.clone())
                            .w(width)
                            .h(height)
                            .aspect_ratio(HIGHLIGHT_WIDTH / HIGHLIGHT_HEIGHT)
                            .rounded(px(HIGHLIGHT_RADIUS))
                            .object_fit(ObjectFit::Cover)
                            .with_fallback(move || {
                                initial_tile(&letter, palette).into_any_element()
                            })
                            .id(("profile-highlight-picture", at))
                            .test_support()
                            .into_any_element()
                    }
                    None => initial_tile(&letter, palette).into_any_element(),
                });
            div()
                .id(("profile-highlight", at))
                .test_support()
                .role(Role::Image)
                .aria_label(look.tr_with("profile-highlight", &[("title", &highlight.title)]))
                .flex_none()
                .w(sp(HIGHLIGHT_WIDTH))
                .flex()
                .flex_col()
                .items_center()
                .gap(sp(6.))
                .relative()
                .top(px(moved.travels * (1.0 - here)))
                .opacity(here * came.max(0.0001))
                .child(cover)
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .text_size(sp(12.))
                        .text_color(color(palette.text))
                        .truncate()
                        .child(highlight.title.clone()),
                )
        });
    Some(
        div()
            .id("profile-highlights")
            .test_support()
            .role(Role::Group)
            .aria_label(look.tr("profile-highlights"))
            .w_full()
            .flex()
            .gap(sp(12.))
            .py(sp(4.))
            .overflow_x_scroll()
            .track_scroll(&shown.highlights)
            .on_scroll(move |event, _, cx: &mut App| {
                // A wheel turns up and down: over the row, it moves it
                // sideways for as long as it can move, then lets the page
                // have it.
                let ScrollDelta::Lines(lines) = event.delta else {
                    return;
                };
                let most = f32::from(handle.max_offset().x);
                if most <= 0.0 || lines.y == 0.0 {
                    return;
                }
                let offset = handle.offset();
                let at = (f32::from(offset.x) + lines.y * 40.0).clamp(-most, 0.0);
                if (at - f32::from(offset.x)).abs() > 0.5 {
                    handle.set_offset(gpui_kit::point(px(at), offset.y));
                    cx.stop_propagation();
                }
            })
            .children(items)
            .into_any_element(),
    )
}

/// The first letter of a highlight with no cover, on a quiet tint.
fn initial_tile(letter: &str, palette: crate::theme::tokens::Palette) -> Div {
    div()
        .size_full()
        .rounded(px(HIGHLIGHT_RADIUS))
        .bg(color_at(palette.text, 0.07))
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::BOLD)
        .text_size(sp(20.))
        .text_color(color(palette.secondary))
        .child(letter.to_string())
}

/// What each tab is called: the posts with their total, when the profile
/// says it.
fn tab_name(tab: Tab, profile: Option<&Profile>, look: &Look) -> String {
    match tab {
        Tab::Posts => match profile.and_then(|profile| profile.posts) {
            Some(count) => look.tr_with(
                "profile-tab-posts-count",
                &[("count", &grouped(count, look.texts.language()))],
            ),
            None => look.tr("profile-tab-posts"),
        },
        Tab::Reposts => look.tr("profile-tab-reposts"),
        Tab::Tagged => look.tr("profile-tab-tagged"),
    }
}

/// The drawing of each tab.
fn tab_icon(tab: Tab) -> &'static str {
    match tab {
        Tab::Posts => assets::GRID,
        Tab::Reposts => assets::REPOST,
        Tab::Tagged => assets::MENTION,
    }
}

/// The tabs, with their underline sliding to the one chosen, and the posts
/// of that one, which come in whole from a little under where they are when
/// they are shown another way.
fn posts(
    shown: &Shown,
    showing: Showing,
    showing_from: Option<(Showing, Instant)>,
    columns: usize,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let palette = look.palette;
    let at = |tab: Tab| Tab::ALL.iter().position(|each| *each == tab).unwrap_or(0) as f32;
    let slid = eased(
        shown.tab_from.1,
        look.moved(motion::INTERACTION, 1.0, 1.0).lasts,
        window,
    );
    let from = at(shown.tab_from.0);
    let under = from + (at(shown.tab) - from) * slid;
    let count = Tab::ALL.len() as f32;

    let tabs = div()
        .id("profile-tabs")
        .test_support()
        .role(Role::TabList)
        .aria_label(look.tr("profile-tabs"))
        .relative()
        .flex()
        .border_b_1()
        .border_color(color(palette.line))
        .children(Tab::ALL.iter().enumerate().map(|(index, tab)| {
            let tab = *tab;
            let on = tab == shown.tab;
            let name = tab_name(tab, shown.profile.as_ref(), look);
            Button::new(("profile-tab", index))
                .role(Role::Tab)
                .accessibility_label(name.clone())
                .aria_selected(on)
                .flex_1()
                .min_w_0()
                .h(sp(44.))
                .flex()
                .items_center()
                .justify_center()
                .gap(sp(8.))
                .rounded_t(px(shape::CONTROL_RADIUS))
                .text_size(sp(14.))
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
                .text_color(color(if on { palette.text } else { palette.secondary }))
                .focus_visible(parts::focus_ring(&palette))
                .cursor_pointer()
                .hover(move |style| style.text_color(color(palette.text)))
                .child(icon(tab_icon(tab), 15.).text_color(color(if on {
                    palette.text
                } else {
                    palette.secondary
                })))
                .child(div().truncate().child(name))
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.choose_tab(tab, window, cx);
                }))
        }))
        .child(
            div()
                .absolute()
                .bottom(px(-1.))
                .left(relative(under / count))
                .w(relative(1.0 / count))
                .h(px(2.))
                .bg(color(palette.underline)),
        );

    let moved = look.moved(motion::ENTER, 8.0, 1.0);
    let switched = showing_from.map_or(1.0, |(_, since)| eased(since, moved.lasts, window));
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(sp(14.))
        .child(tabs)
        .child(
            div()
                .w_full()
                .relative()
                .top(px(moved.travels * (1.0 - switched)))
                .opacity(switched)
                .child(tab_content(shown, showing, columns, look, window)),
        )
}

/// What the chosen tab holds, or why it holds nothing yet.
fn tab_content(
    shown: &Shown,
    showing: Showing,
    columns: usize,
    look: &Look,
    window: &mut Window,
) -> AnyElement {
    let visible = shown.profile.as_ref().and_then(|profile| profile.visible);
    let kept = shown.kept.as_ref().filter(|kept| !kept.tiles.is_empty());
    let posts = |tiles: &Tiles, window: &mut Window| match showing {
        Showing::Grid => grid(tiles, shown.shown_since, columns, look, window),
        Showing::List => list(tiles, shown.shown_since, look, window),
    };
    // What could not be read is said above what the cache kept of it.
    let failed = |said: AnyElement, window: &mut Window| match kept {
        Some(kept) => div()
            .w_full()
            .flex()
            .flex_col()
            .child(said)
            .child(posts(kept, window))
            .into_any_element(),
        None => said,
    };
    match (visible, &shown.read, &shown.failed) {
        (_, _, Some(_)) if shown.profile.is_none() => div().into_any_element(),
        (Some(false), _, _) => status(
            "profile-hidden",
            look.tr("profile-private"),
            Some(look.tr("profile-private-note")),
            look,
        ),
        // What the cache kept, while the tab is read again.
        (None, _, _) | (_, None | Some(TabRead::Reading), _) if let Some(kept) = kept => {
            posts(kept, window)
        }
        (None, _, _) | (_, None | Some(TabRead::Reading), _) => div()
            .id("profile-reading")
            .test_support()
            .role(Role::Status)
            .aria_label(look.tr("profile-reading"))
            .w_full()
            .flex()
            .justify_center()
            .pt(sp(STATUS_PAD))
            .child(parts::ring(26., look, window))
            .into_any_element(),
        (_, Some(TabRead::Failed(Unread::Failed(_))), _) => failed(
            status(
                "profile-tab-failed",
                look.tr("profile-posts-failed"),
                None,
                look,
            ),
            window,
        ),
        (_, Some(TabRead::Failed(why)), _) => failed(
            status("profile-tab-failed", unread_words(why, look), None, look),
            window,
        ),
        (_, Some(TabRead::Read(tiles)), _) if tiles.tiles.is_empty() => status(
            "profile-empty",
            look.tr(match shown.tab {
                Tab::Posts => "profile-no-posts",
                Tab::Reposts => "profile-no-reposts",
                Tab::Tagged => "profile-no-tagged",
            }),
            None,
            look,
        ),
        (_, Some(TabRead::Read(tiles)), _) => posts(tiles, window),
    }
}

/// A sentence in the middle of where posts would be, and a quieter one
/// under it.
fn status(id: &'static str, said: String, note: Option<String>, look: &Look) -> AnyElement {
    let palette = look.palette;
    let whole = match &note {
        Some(note) => format!("{said} {note}"),
        None => said.clone(),
    };
    div()
        .id(id)
        .test_support()
        .role(Role::Status)
        .aria_label(whole)
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(sp(6.))
        .pt(sp(STATUS_PAD))
        .text_center()
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(sp(16.))
                .text_color(color(palette.text))
                .child(said),
        )
        .children(note.map(|note| {
            div()
                .max_w(sp(420.))
                .text_size(sp(14.))
                .line_height(sp(20.))
                .text_color(color(palette.secondary))
                .child(note)
        }))
        .into_any_element()
}

/// How far the tile at `at` has come in, `since` the posts were shown: one
/// after the other, quickly.
fn tile_in(at: usize, since: Instant, look: &Look, window: &mut Window) -> f32 {
    let delay = (TILE_STAGGER * at as u32).min(TILE_STAGGER_MOST);
    let starts = since + delay;
    if Instant::now() < starts {
        crate::ui::frames::ask(window);
        return 0.0;
    }
    eased(starts, look.moved(motion::ENTER, 0.0, 1.0).lasts, window)
}

/// What a post is called, for whoever does not see its picture.
fn tile_label(tile: &Tile, look: &Look) -> String {
    let kind = match tile.kind {
        TileKind::Photo => look.tr("profile-photo"),
        TileKind::Video => look.tr("profile-video"),
        TileKind::Reel => look.tr("profile-reel"),
        TileKind::Carousel(count) => {
            look.tr_with("profile-carousel", &[("count", &count.to_string())])
        }
    };
    let mut label = kind;
    if tile.pinned {
        label = format!("{label}, {}", look.tr("profile-pinned"));
    }
    if let Some(owner) = &tile.owner {
        label = format!("{label}, @{owner}");
    }
    match &tile.caption {
        Some(caption) if !caption.trim().is_empty() => format!("{label}: {caption}"),
        _ => label,
    }
}

/// The drawing that marks what a post is, over its corner.
fn kind_icon(kind: TileKind) -> Option<&'static str> {
    match kind {
        TileKind::Photo => None,
        TileKind::Video | TileKind::Reel => Some(assets::VIDEO),
        TileKind::Carousel(_) => Some(assets::CAROUSEL),
    }
}

/// A post's small picture filling its place, or its kind's drawing while
/// there is none.
fn thumb(tile: &Tile, look: &Look) -> Div {
    let palette = look.palette;
    let drawing = kind_icon(tile.kind).unwrap_or(assets::PHOTO);
    let empty = move || {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(icon(drawing, 22.).text_color(color_at(palette.secondary, 0.7)))
    };
    div()
        .size_full()
        .overflow_hidden()
        .rounded(px(4.))
        .bg(color_at(palette.text, 0.06))
        .child(match &tile.thumb {
            Some(path) => img(path.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .with_fallback(move || empty().into_any_element())
                .into_any_element(),
            None => empty().into_any_element(),
        })
}

/// The first page of a tab as a grid of squares.
fn grid(
    tiles: &Tiles,
    since: Instant,
    columns: usize,
    look: &Look,
    window: &mut Window,
) -> AnyElement {
    let palette = look.palette;
    let moved = look.moved(motion::ENTER, 10.0, 0.96);
    let share = 1.0 / columns.max(1) as f32;
    div()
        .id("profile-grid-posts")
        .test_support()
        .w_full()
        .flex()
        .flex_wrap()
        .children(tiles.tiles.iter().enumerate().map(|(at, tile)| {
            let here = tile_in(at, since, look, window);
            let badge = kind_icon(tile.kind).map(|drawing| {
                div()
                    .absolute()
                    .top(px(8.))
                    .right(px(8.))
                    .p(px(3.))
                    .rounded_full()
                    .bg(color_at(palette.text, 0.55))
                    .child(icon(drawing, 12.).text_color(color(palette.panel)))
            });
            div()
                .id(("profile-tile", at))
                .test_support()
                .role(Role::Image)
                .aria_label(tile_label(tile, look))
                .w(relative(share))
                .p(px(2.))
                .relative()
                .top(px(moved.travels * (1.0 - here)))
                .opacity(here)
                .child(
                    div()
                        .w_full()
                        .aspect_square()
                        .relative()
                        .child(thumb(tile, look))
                        .children(badge),
                )
        }))
        .children(more(tiles, look))
        .into_any_element()
}

/// The first page of a tab as a list: the small picture, the caption, and
/// when and how it was liked.
fn list(tiles: &Tiles, since: Instant, look: &Look, window: &mut Window) -> AnyElement {
    let palette = look.palette;
    let language = look.texts.language();
    let moved = look.moved(motion::ENTER, 8.0, 1.0);
    div()
        .id("profile-list-posts")
        .test_support()
        .w_full()
        .flex()
        .flex_col()
        .children(tiles.tiles.iter().enumerate().map(|(at, tile)| {
            let here = tile_in(at, since, look, window);
            let mut meta = Vec::new();
            if let Some(owner) = &tile.owner {
                meta.push(format!("@{owner}"));
            }
            if let Some(taken) = tile.taken_at {
                meta.push(day(taken));
            }
            if let Some(likes) = tile.likes {
                meta.push(look.tr_with("profile-likes", &[("count", &grouped(likes, language))]));
            }
            if let Some(comments) = tile.comments {
                meta.push(look.tr_with(
                    "profile-comments",
                    &[("count", &grouped(comments, language))],
                ));
            }
            let caption = tile
                .caption
                .clone()
                .filter(|caption| !caption.trim().is_empty())
                .unwrap_or_else(|| look.tr("profile-no-caption"));
            let kind = match tile.kind {
                TileKind::Photo => look.tr("profile-photo"),
                TileKind::Video => look.tr("profile-video"),
                TileKind::Reel => look.tr("profile-reel"),
                TileKind::Carousel(count) => {
                    look.tr_with("profile-carousel", &[("count", &count.to_string())])
                }
            };
            div()
                .id(("profile-tile", at))
                .test_support()
                .role(Role::Image)
                .aria_label(tile_label(tile, look))
                .w_full()
                .flex()
                .items_center()
                .gap(sp(14.))
                .py(sp(10.))
                .border_b_1()
                .border_color(color(palette.line))
                .relative()
                .left(px(moved.travels * (1.0 - here)))
                .opacity(here)
                .child(div().flex_none().size(sp(64.)).child(thumb(tile, look)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(sp(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(6.))
                                .text_size(sp(12.))
                                .text_color(color(palette.secondary))
                                .children(kind_icon(tile.kind).map(|drawing| {
                                    icon(drawing, 12.).text_color(color(palette.secondary))
                                }))
                                .child(kind)
                                .when(tile.pinned, |row| {
                                    row.child("\u{b7}").child(look.tr("profile-pinned"))
                                }),
                        )
                        .child(
                            div()
                                .text_size(sp(14.))
                                .line_height(sp(20.))
                                .text_color(color(palette.text))
                                .line_clamp(2)
                                .child(caption),
                        )
                        .child(
                            div()
                                .text_size(sp(12.))
                                .text_color(color(palette.secondary))
                                .truncate()
                                .child(meta.join(" \u{b7} ")),
                        ),
                )
        }))
        .children(more(tiles, look))
        .into_any_element()
}

/// A post's date, in the person's time.
fn day(at: snob_core::Epoch) -> String {
    chrono::DateTime::from_timestamp(at.get(), 0)
        .map(|at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

/// Under a first page with more past it: that only the first is shown.
fn more(tiles: &Tiles, look: &Look) -> Option<Div> {
    tiles.more.then(|| {
        div()
            .w_full()
            .pt(sp(18.))
            .text_center()
            .text_size(sp(13.))
            .text_color(color(look.palette.secondary))
            .child(parts::said(
                "profile-more",
                look.tr_with(
                    "profile-first-page",
                    &[("count", &tiles.tiles.len().to_string())],
                ),
            ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::Settings;

    #[test]
    fn a_count_sets_its_thousands_apart_as_its_language_writes_them() {
        assert_eq!(grouped(999, "en"), "999");
        assert_eq!(grouped(1_234, "en"), "1,234");
        assert_eq!(grouped(1_234_567, "en"), "1,234,567");
        assert_eq!(grouped(9_999, "es"), "9999");
        assert_eq!(grouped(12_345, "es"), "12.345");
        assert_eq!(grouped(0, "es"), "0");
    }

    #[test]
    fn every_reason_a_profile_was_not_read_is_said_in_words() {
        let look = Look::of(Settings::default(), false);
        for why in [
            Unread::Hidden,
            Unread::Paused(None),
            Unread::Paused(Some(snob_core::Epoch::new(1_800_000_000))),
            Unread::NoSession,
            Unread::Nobody,
            Unread::Failed("error".into()),
        ] {
            let said = unread_words(&why, &look);
            assert!(said.ends_with('.'), "{why:?}: {said}");
            assert!(!said.starts_with("profile-"), "{why:?} has no words");
        }
    }

    /// GPUI refuses a second tip on one control in a debug build (the
    /// assertion in gpui's `div.rs`): a button of the page that refuses one
    /// has its own. Each refusal prints its panic message.
    #[cfg(debug_assertions)]
    #[test]
    fn every_button_of_the_page_that_shows_no_words_has_its_tip() {
        let look = Look::of(Settings::default(), false);
        let has_a_tip = |button: Button| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                drop(button.tooltip(parts::tip("another")));
            }))
            .is_err()
        };
        let me = Named {
            pk: 42.into(),
            username: Some("me".into()),
        };
        for (what, button) in [
            ("grid", showing_segment(Showing::Grid, true, &look)),
            ("list", showing_segment(Showing::List, false, &look)),
            ("layout", layout_button(true, &look)),
            ("the other layout", layout_button(false, &look)),
            ("options", options_button(true, &look)),
            ("username", username_button(&me, true, 17., &look)),
        ] {
            assert!(has_a_tip(button), "{what} has no tip");
        }
    }

    #[test]
    fn a_post_is_named_by_its_kind_its_owner_and_its_caption() {
        let look = Look::of(Settings::default(), false);
        let tile = Tile {
            code: "C".into(),
            kind: TileKind::Carousel(3),
            thumb: None,
            caption: Some("at the sea".into()),
            owner: Some("user1".into()),
            taken_at: None,
            likes: None,
            comments: None,
            pinned: true,
        };
        assert_eq!(
            tile_label(&tile, &look),
            "Carousel of 3, pinned, @user1: at the sea"
        );
    }
}
