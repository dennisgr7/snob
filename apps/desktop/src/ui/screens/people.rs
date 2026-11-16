//! One of an account's lists, as far as it is read: **its rows come in as
//! the walk reads them**, a page at a time, and a click on one opens that
//! account's profile. The same list drawn in a window of its own
//! (`ui::detached`) is this one without the way back.
//!
//! Only rows on screen are built (`uniform_list`): a list of thousands
//! costs what a screen of it costs. **A row shows a picture only when the
//! cache already holds one**: nothing is fetched for a row.

use std::rc::Rc;

use gpui_kit::base::{Scrollbar, TestSupportExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, FocusHandle, FontWeight, Role, Stateful, Window, div,
    px, relative, uniform_list,
};

use super::handle;
use crate::backend::Listed;
use crate::state::flow::Named;
use crate::state::profile::{self, Side};
use crate::theme::motion;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts;
use crate::ui::screens::profile::{as_account, grouped, unread_words};
use crate::ui::shell::{PeopleList, Phase, Shell, eased};

/// How tall a row is, at the default text size.
const ROW: f32 = 60.0;

/// What a click on a row does: opens the account's profile, wherever the
/// list is drawn.
pub type Open = Rc<dyn Fn(Named, &mut Window, &mut App)>;

/// What a list is called: whose, and which.
pub fn title(owner: &Named, side: Side, look: &Look) -> String {
    let account = handle(owner);
    match side {
        Side::Followers => look.tr_with("people-followers-title", &[("account", &account)]),
        Side::Following => look.tr_with("people-following-title", &[("account", &account)]),
    }
}

/// Where the reading stands, in words.
pub fn progress_words(list: &PeopleList, look: &Look) -> String {
    let language = look.texts.language();
    let read = grouped(list.walked.people.len() as u64, language);
    match &list.ended {
        None => match list.walked.expected {
            Some(expected) => look.tr_with(
                "people-reading-of",
                &[("count", &read), ("total", &grouped(expected, language))],
            ),
            None => look.tr_with("people-reading", &[("count", &read)]),
        },
        Some(Ok(Listed {
            whole, stored_at, ..
        })) => {
            let mut said = look.tr_with("people-read", &[("count", &read)]);
            if let Some(at) = stored_at {
                said = format!(
                    "{said} {}",
                    look.tr_with("people-stored", &[("time", &crate::ui::frame::clock(*at))])
                );
            }
            if !whole {
                said = format!("{said} {}", look.tr("people-not-whole"));
            }
            said
        }
        Some(Err(why)) => unread_words(why, look),
    }
}

/// The list shown in the panel.
pub fn render(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let Some(list) = shell.people_list() else {
        return div().into_any_element();
    };
    let back = shell.can_go_back();
    let shell_entity = cx.entity().downgrade();
    let open: Open = Rc::new(move |who, window, cx| {
        shell_entity
            .update(cx, |shell, cx| shell.open_profile(who, window, cx))
            .ok();
    });
    let bar = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(10.))
        .children(back.then(|| {
            parts::small_icon_button("people-back", look.tr("back"), assets::BACK, &look)
                .on_click(cx.listener(|shell, _, window, cx| shell.go_back(window, cx)))
        }))
        .child(div().flex_1())
        .child(
            parts::small_icon_button(
                "people-detach",
                look.tr("people-detach"),
                assets::NEW_WINDOW,
                &look,
            )
            .on_click(cx.listener(|shell, _, window, cx| shell.detach_the_list(window, cx))),
        );
    body(&list, Some(bar), &shell.title, open, &look, window, cx)
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .into_any_element()
}

/// The list's title, where its reading stands, and its rows: what the panel
/// and a window of its own both draw. `bar` goes above the title.
pub fn body(
    list: &Entity<PeopleList>,
    bar: Option<Div>,
    title_focus: &FocusHandle,
    open: Open,
    look: &Look,
    window: &mut Window,
    cx: &App,
) -> Stateful<Div> {
    let palette = look.palette;
    let read = list.read(cx);
    let named = title(&read.owner, read.side, look);
    let said = progress_words(read, look);
    let going = read.ended.is_none();
    let failed = matches!(read.ended, Some(Err(_)));
    let share = profile::share_read(read.walked.people.len(), read.walked.expected);
    let count = read.walked.people.len();
    let handle = read.scroll.clone();
    let now = std::time::Instant::now();

    // The bar fills to what was read, smoothly, and leaves once the end
    // came.
    let filled = share.unwrap_or(0.0);
    let progress = going.then(|| {
        div()
            .id("people-bar")
            .test_support()
            .role(Role::ProgressIndicator)
            .aria_label(said.clone())
            .w_full()
            .h(px(3.))
            .rounded_full()
            .bg(color_at(palette.text, 0.08))
            .overflow_hidden()
            .child(
                div()
                    .h_full()
                    .w(relative(filled.max(0.02)))
                    .rounded_full()
                    .bg(color(palette.accent)),
            )
    });

    let header = div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(sp(10.))
        .px(sp(32.))
        .pt(sp(18.))
        .pb(sp(14.))
        .children(bar)
        .child(
            div()
                .id("title")
                .test_support()
                .role(Role::Heading)
                .aria_level(1)
                .aria_label(named.clone())
                .track_focus(title_focus)
                .font_weight(FontWeight::BOLD)
                .text_size(sp(26.))
                .line_height(sp(34.))
                .text_color(color(palette.text))
                .child(named),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(10.))
                .children(going.then(|| parts::ring(16., look, window)))
                .child(
                    div()
                        .id("people-progress")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(said.clone())
                        .text_size(sp(14.))
                        .text_color(color(if failed {
                            palette.error
                        } else {
                            palette.secondary
                        }))
                        .child(said),
                ),
        )
        .children(progress);

    let moved = look.moved(motion::ENTER, 10.0, 1.0);
    let rows = {
        let list = list.clone();
        let look = look.clone();
        uniform_list("people-rows", count, move |range, window, cx| {
            let read = list.read(cx);
            range
                .filter_map(|at| {
                    let person = read.walked.people.get(at)?;
                    // A row comes in as its page arrived.
                    let here = read.arrived_at(at).map_or(1.0, |since| {
                        let since = since + std::time::Duration::from_millis(18) * (at % 12) as u32;
                        if now < since {
                            window.request_animation_frame();
                            0.0
                        } else {
                            eased(since, moved.lasts, window)
                        }
                    });
                    Some(row(at, person, here, moved.travels, &look, open.clone()))
                })
                .collect::<Vec<_>>()
        })
        .size_full()
        .track_scroll(&handle)
        .px(sp(24.))
        .pb(sp(24.))
    };
    let bar = Scrollbar::vertical(&handle)
        .id("people-scrollbar")
        .viewport_from_layout()
        .styles(move |styles| {
            styles
                .thumb(move |thumb| thumb.bg(color(palette.control_line)))
                .thumb_hover(move |thumb| thumb.bg(color(palette.secondary)))
                .thumb_active(move |thumb| thumb.bg(color(palette.secondary)))
        });

    div()
        .id("people")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .child(header)
        .child(
            div()
                .id("people-list")
                .test_support()
                .role(Role::List)
                .aria_label(look.tr("people-list"))
                .flex_1()
                .min_h_0()
                .relative()
                .child(rows)
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right(px(6.))
                        .child(bar),
                ),
        )
}

/// One account of the list: its picture when the cache holds one, its
/// names, and what it is; a click opens its profile.
fn row(
    at: usize,
    person: &crate::backend::Person,
    here: f32,
    travels: f32,
    look: &Look,
    open: Open,
) -> AnyElement {
    let palette = look.palette;
    let username = person
        .named
        .username
        .clone()
        .unwrap_or_else(|| person.named.pk.to_string());
    let account = as_account(
        &person.named,
        person.full_name.as_deref(),
        person.picture.clone(),
    );
    let mut marks = Vec::new();
    if person.is_verified {
        marks.push(look.tr("profile-verified"));
    }
    if person.is_private {
        marks.push(look.tr("profile-private-mark"));
    }
    let who = person.named.clone();
    gpui_kit::base::Button::new(("person", at))
        .accessibility_label(look.tr_with("people-open", &[("username", &username)]))
        .w_full()
        .h(sp(ROW))
        .px(sp(10.))
        .flex()
        .items_center()
        .justify_start()
        .gap(sp(12.))
        .rounded(px(shape::CONTROL_RADIUS))
        .relative()
        .top(px(travels * (1.0 - here)))
        .opacity(here)
        .hover(move |style| style.bg(color(palette.frame)))
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .child(parts::avatar(&account, 40., look, 1.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.))
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(sp(14.))
                                .text_color(color(palette.text))
                                .truncate()
                                .child(format!("@{username}")),
                        )
                        .children(marks.into_iter().map(|mark| {
                            div()
                                .flex_none()
                                .text_size(sp(12.))
                                .text_color(color(palette.secondary))
                                .child(mark)
                        })),
                )
                .children(person.full_name.clone().map(|name| {
                    div()
                        .text_size(sp(13.))
                        .text_color(color(palette.secondary))
                        .truncate()
                        .child(name)
                })),
        )
        .on_click(move |_, window, cx| open(who.clone(), window, cx))
        .into_any_element()
}
