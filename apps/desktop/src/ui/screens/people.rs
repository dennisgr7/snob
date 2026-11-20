//! One of an account's lists: **what was stored of it shows at once**, and
//! it is read again behind it, its rows replaced once that read ends; a
//! list never stored has its rows come in as the walk reads them. A click
//! on a row opens that account's profile. The same list drawn in a window
//! of its own (`ui::detached`) is this one, and the way back is the band's.
//!
//! Only rows on screen are built (`uniform_list`): a list of thousands
//! costs what a screen of it costs. **A row on screen shows the small
//! picture the cache keeps, or one downloaded from the CDN at the address
//! its list answered**, asked for once the rows stay still
//! (`state::people`). Its options choose rows to copy their usernames, or
//! move it to a window of its own: **nothing is sent for either, and no
//! write is offered**.

use std::rc::Rc;

use gpui_kit::base::{
    Align, Button, FocusTrapElement as _, POPUP_PRIORITY, Placement, Positioner, Scrollbar,
    TestSupportExt as _,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Context, Div, Entity, FocusHandle, FontWeight, MouseButton, Role, Stateful,
    Toggled, Window, anchored, deferred, div, point, px, relative, uniform_list,
};
use snob_core::Epoch;

use super::handle;
use super::words::{self, Day};
use crate::backend::Person;
use crate::state::flow::Named;
use crate::state::people::{self, Said};
use crate::state::profile::{self, Side};
use crate::theme::motion;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Tone};
use crate::ui::screens::profile::{as_account, grouped, unread_words};
use crate::ui::shell::{PeopleList, Phase, Shell, eased};
use crate::ui::{MenuDown, MenuUp};

/// How tall a row is, at the default text size.
const ROW: f32 = 60.0;

/// What a click on a row does: opens the account's profile, wherever the
/// list is drawn.
pub type Open = Rc<dyn Fn(Named, &mut Window, &mut App)>;

/// Something a list does past itself, through the app's view.
pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a list does that reaches past it: open a row's profile, copy the
/// usernames chosen, and move it to a window of its own, which a list
/// already in one does not offer.
pub struct Acts {
    pub open: Open,
    pub copy: Act,
    pub detach: Option<Act>,
}

/// Now, for the words: **the one place this screen reads the clock**.
fn now() -> Epoch {
    snob_core::clock::now()
}

/// What a list is called: whose, and which.
pub fn title(owner: &Named, side: Side, look: &Look) -> String {
    let account = handle(owner);
    match side {
        Side::Followers => look.tr_with("people-followers-title", &[("account", &account)]),
        Side::Following => look.tr_with("people-following-title", &[("account", &account)]),
    }
}

/// Where the list stands, in words: how many accounts and when they were
/// last scanned, and the read behind them; or as far as a first read is.
pub fn progress_words(list: &PeopleList, look: &Look) -> String {
    let language = look.texts.language();
    let reading_words = |going: people::Going, of: &'static str, alone: &'static str| {
        let read = grouped(going.read as u64, language);
        match going.expected {
            Some(expected) => look.tr_with(
                of,
                &[("count", &read), ("total", &grouped(expected, language))],
            ),
            None => look.tr_with(alone, &[("count", &read)]),
        }
    };
    let why = || list.failed.as_ref().map(|why| unread_words(why, look));
    match list.said() {
        Said::Nothing => String::new(),
        Said::Reading(going) => reading_words(going, "people-reading-of", "people-reading"),
        Said::Failed => why().unwrap_or_default(),
        Said::Answered {
            answered,
            updating,
            failed,
        } => {
            let count = answered.count as u64;
            let mut said = vec![
                look.tr_count(
                    "people-accounts",
                    count,
                    &[("count", &grouped(count, language))],
                ),
                scan_words(answered.taken_at, now(), look),
            ];
            if !answered.whole {
                said.push(look.tr("people-not-whole"));
            }
            if let Some(going) = updating {
                said.push(match going.expected {
                    Some(_) => reading_words(going, "people-updating-of", "people-updating"),
                    None => look.tr("people-updating"),
                });
            }
            if failed {
                said.extend(why());
            }
            said.join(" ")
        }
    }
}

/// When a list was last scanned: its time alone today, yesterday's said so,
/// and its day before that.
fn scan_words(at: Epoch, now: Epoch, look: &Look) -> String {
    let time = words::clock_words(at, look);
    let hour = hour_written(&time);
    match words::day_of(at, now) {
        Day::Today => look.tr_count("people-scanned-today", hour, &[("time", &time)]),
        Day::Yesterday => look.tr_count("people-scanned-yesterday", hour, &[("time", &time)]),
        day => look.tr_count(
            "people-scanned-on",
            hour,
            &[("time", &time), ("date", &words::day_words(&day, look))],
        ),
    }
}

/// The hour a time of day is written with, which some languages say in the
/// singular at one.
fn hour_written(time: &str) -> u64 {
    let digits: String = time.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().unwrap_or(0)
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
    let shell_entity = cx.entity().downgrade();
    let open: Open = {
        let shell = shell_entity.clone();
        Rc::new(move |who, window, cx| {
            shell
                .update(cx, |shell, cx| shell.open_profile(who, window, cx))
                .ok();
        })
    };
    let copy = {
        let (shell, list) = (shell_entity.clone(), list.clone());
        Rc::new(move |_: &mut Window, cx: &mut App| {
            shell
                .update(cx, |shell, cx| shell.copy_the_usernames(&list, cx))
                .ok();
        })
    };
    let detach = Rc::new(move |window: &mut Window, cx: &mut App| {
        shell_entity
            .update(cx, |shell, cx| shell.detach_the_list(window, cx))
            .ok();
    });
    let acts = Acts {
        open,
        copy,
        detach: Some(detach),
    };
    body(&list, acts, &shell.title, &look, window, cx)
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .into_any_element()
}

/// The list's title and its options, where it stands, the rows chosen while
/// choosing, and its rows: what the panel and a window of its own both
/// draw.
pub fn body(
    list: &Entity<PeopleList>,
    acts: Acts,
    title_focus: &FocusHandle,
    look: &Look,
    window: &mut Window,
    cx: &App,
) -> Stateful<Div> {
    let palette = look.palette;
    let read = list.read(cx);
    let named = title(&read.owner, read.side, look);
    let said = progress_words(read, look);
    let going = read.reading.as_ref();
    let failed = read.failed.is_some() && going.is_none();
    let share = going.and_then(|walked| profile::share_read(walked.people.len(), walked.expected));
    let count = read.people.len();
    let handle = read.scroll.clone();
    let now = std::time::Instant::now();

    // The bar fills to what was read, smoothly, and leaves once the end
    // came.
    let progress = going.map(|_| {
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
                    .w(relative(share.unwrap_or(0.0).max(0.02)))
                    .rounded_full()
                    .bg(color(palette.accent)),
            )
    });

    let heading = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(16.))
        .child(
            div().flex_1().min_w_0().child(
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
            ),
        )
        .child(options(list, &acts, look, window, cx));

    let header = div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(sp(10.))
        .px(sp(32.))
        .pt(sp(26.))
        .pb(sp(14.))
        .child(heading)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(10.))
                .children(going.map(|_| parts::ring(16., look, window)))
                .child(
                    div()
                        .id("people-progress")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(said.clone())
                        .text_size(sp(14.))
                        .line_height(sp(20.))
                        .text_color(color(if failed {
                            palette.error
                        } else {
                            palette.secondary
                        }))
                        .child(said),
                ),
        )
        .children(
            read.selection
                .choosing()
                .then(|| selection_bar(list, &acts, look, cx)),
        )
        .children(progress);

    let moved = look.moved(motion::ENTER, 10.0, 1.0);
    let arrives = look.moved(motion::PICTURE_ARRIVES, 0.0, 1.0).lasts;
    let rows = {
        let list = list.clone();
        let look = look.clone();
        let drawn = read.drawn.clone();
        let open = acts.open.clone();
        uniform_list("people-rows", count, move |range, window, cx| {
            // The list lays its first row out alone before those in sight:
            // that is a measure, not what is drawn.
            let measured = range == (0..1) && count > 1;
            if !measured && drawn.replace(range.clone()) != range {
                let list = list.clone();
                cx.defer(move |cx| list.update(cx, |list, cx| list.look_for_pictures(cx)));
            }
            let read = list.read(cx);
            let choosing = read.selection.choosing();
            range
                .filter_map(|at| {
                    let person = read.people.get(at)?;
                    // A row comes in as its page arrived.
                    let here = read.arrived_at(at).map_or(1.0, |since| {
                        let since = since + std::time::Duration::from_millis(18) * (at % 12) as u32;
                        if now < since {
                            crate::ui::frames::ask(window);
                            0.0
                        } else {
                            eased(since, moved.lasts, window)
                        }
                    });
                    let shown = read
                        .came
                        .get(&person.named.pk)
                        .map_or(1.0, |since| eased(*since, arrives, window));
                    let chosen = choosing.then(|| read.selection.is_chosen(person.named.pk));
                    let drawn = Drawn {
                        here,
                        travels: moved.travels,
                        shown,
                        chosen,
                    };
                    Some(row(at, person, drawn, &look, &list, open.clone()))
                })
                .collect::<Vec<_>>()
        })
        .size_full()
        .track_scroll(&handle)
        .px(sp(24.))
        // Room for the first row's focus ring, which the list clips.
        .pt(px(shape::FOCUS_ROOM))
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
                .flex()
                .flex_col()
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

/// The options' button, kept where it was drawn, with its menu under it
/// while open: choose rows, and move the list to a window of its own.
fn options(
    list: &Entity<PeopleList>,
    acts: &Acts,
    look: &Look,
    window: &mut Window,
    cx: &App,
) -> Div {
    let read = list.read(cx);
    let open = read.menu.is_some();
    let button = parts::small_icon_button(
        "people-options",
        look.tr("people-options"),
        assets::MORE,
        look,
    )
    .track_focus(&read.options_button)
    .aria_expanded(open)
    .on_click({
        let list = list.clone();
        move |_, window, cx| list.update(cx, |list, cx| list.toggle_the_menu(window, cx))
    });
    parts::drawn_at(&read.options_drawn, open, button).children(menu(list, acts, look, window, cx))
}

/// The list's options, over everything else and walked with the arrows,
/// with a backdrop a press on which closes them.
fn menu(
    list: &Entity<PeopleList>,
    acts: &Acts,
    look: &Look,
    window: &mut Window,
    cx: &App,
) -> Vec<AnyElement> {
    let palette = look.palette;
    let read = list.read(cx);
    let Some(open) = &read.menu else {
        return Vec::new();
    };
    let moved = look.moved(motion::INTERACTION, 6.0, 1.0);
    let came = eased(open.since, moved.lasts, window);
    let on = |act: fn(&mut PeopleList, &mut Window, &mut Context<PeopleList>)| {
        let list = list.clone();
        move |window: &mut Window, cx: &mut App| list.update(cx, |list, cx| act(list, window, cx))
    };
    let mut items = vec![
        parts::menu_item(
            "people-select",
            look.tr("people-select"),
            assets::CHECK,
            look,
        )
        .on_click({
            let choose = on(PeopleList::choose);
            move |_, window, cx| choose(window, cx)
        })
        .into_any_element(),
    ];
    if let Some(detach) = acts.detach.clone() {
        items.push(
            parts::menu_item(
                "people-detach",
                look.tr("people-detach"),
                assets::NEW_WINDOW,
                look,
            )
            .on_click(move |_, window, cx| detach(window, cx))
            .into_any_element(),
        );
    }
    let close = on(PeopleList::close_the_menu);
    let up = on(|list, window, cx| list.move_in_the_menu(false, window, cx));
    let down = on(|list, window, cx| list.move_in_the_menu(true, window, cx));
    let shown = div()
        .id("people-menu")
        .test_support()
        .role(Role::Menu)
        .aria_label(look.tr("people-options-menu"))
        .key_context("SnobMenu")
        .on_action(move |_: &MenuUp, window, cx| up(window, cx))
        .on_action(move |_: &MenuDown, window, cx| down(window, cx))
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
                .focus_trap("people-menu-items", &open.focus),
        );
    let seen = window.viewport_size();
    let backdrop = div()
        .id("people-menu-backdrop")
        .w(seen.width)
        .h(seen.height)
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| close(window, cx));
    vec![
        deferred(anchored().position(point(px(0.), px(0.))).child(backdrop))
            .with_priority(POPUP_PRIORITY - 1)
            .into_any_element(),
        deferred(
            Positioner::side(read.options_drawn.get())
                .placement(Placement::Bottom)
                .align(Align::End)
                .offset(px(6.))
                .margin(px(8.))
                .child(shown),
        )
        .with_priority(POPUP_PRIORITY)
        .into_any_element(),
    ]
}

/// While rows are chosen: how many, and choosing all, copying their
/// usernames and stopping. **Nothing is sent for any of it.**
fn selection_bar(
    list: &Entity<PeopleList>,
    acts: &Acts,
    look: &Look,
    cx: &App,
) -> impl IntoElement + use<> {
    let palette = look.palette;
    let read = list.read(cx);
    let chosen = read.selection.count();
    let all = read
        .selection
        .has_all(read.people.iter().map(|person| person.named.pk));
    let language = look.texts.language();
    let (key, count) = match read.copied {
        Some(copied) => ("people-copied", copied as u64),
        None => ("people-selected", chosen as u64),
    };
    let counted = look.tr_count(key, count, &[("count", &grouped(count, language))]);
    let copy = acts.copy.clone();
    div()
        .id("people-selection")
        .test_support()
        .role(Role::Group)
        .aria_label(look.tr("people-selection"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(10.))
        .child(
            div()
                .id("people-selected")
                .test_support()
                .role(Role::Status)
                .aria_label(counted.clone())
                .text_size(sp(14.))
                .line_height(sp(20.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color(palette.text))
                .child(counted),
        )
        .child(div().flex_1())
        .child(
            parts::small_button(
                "people-select-all",
                look.tr("people-select-all"),
                !all,
                look,
            )
            .on_click({
                let list = list.clone();
                move |_, _, cx| list.update(cx, |list, cx| list.choose_all(cx))
            }),
        )
        .child(
            parts::small_button("people-copy", look.tr("people-copy"), chosen > 0, look)
                .on_click(move |_, window, cx| copy(window, cx)),
        )
        .child(
            parts::small_button("people-cancel", look.tr("people-cancel"), true, look).on_click({
                let list = list.clone();
                move |_, window, cx| list.update(cx, |list, cx| list.stop_choosing(window, cx))
            }),
        )
}

/// How a row is drawn on this frame.
struct Drawn {
    /// How far it has come in.
    here: f32,
    travels: f32,
    /// How far its picture has come in.
    shown: f32,
    /// Whether it is chosen, while rows are.
    chosen: Option<bool>,
}

/// One account of the list: its picture, its username, its names and its
/// marks. A click opens its profile, or while rows are chosen chooses it.
fn row(
    at: usize,
    person: &Person,
    drawn: Drawn,
    look: &Look,
    list: &Entity<PeopleList>,
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
        marks.push(parts::badge(
            ("person-verified", at),
            look.tr("profile-verified"),
            Tone::Verified,
            look,
        ));
    }
    if person.is_private {
        marks.push(parts::badge(
            ("person-private", at),
            look.tr("profile-private-mark"),
            Tone::Plain,
            look,
        ));
    }
    let who = person.named.clone();
    let button = Button::new(("person", at));
    let button = match drawn.chosen {
        Some(chosen) => {
            let (list, pk) = (list.clone(), who.pk);
            button
                .role(Role::CheckBox)
                .aria_toggled(if chosen {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .accessibility_label(look.tr_with("people-choose", &[("username", &username)]))
                .when(chosen, |row| row.bg(color_at(palette.accent, 0.08)))
                .on_click(move |_, _, cx| list.update(cx, |list, cx| list.toggle(pk, cx)))
        }
        None => button
            .accessibility_label(look.tr_with("people-open", &[("username", &username)]))
            .on_click(move |_, window, cx| open(who.clone(), window, cx)),
    };
    button
        .w_full()
        .h(sp(ROW))
        .px(sp(10.))
        .flex()
        .items_center()
        .justify_start()
        .gap(sp(12.))
        .rounded(px(shape::CONTROL_RADIUS))
        .relative()
        .top(px(drawn.travels * (1.0 - drawn.here)))
        .opacity(drawn.here)
        .hover(move |style| {
            style.bg(if drawn.chosen == Some(true) {
                color_at(palette.accent, 0.14)
            } else {
                color(palette.frame)
            })
        })
        .focus_visible(parts::focus_ring(&palette))
        .cursor_pointer()
        .children(drawn.chosen.map(|chosen| parts::tick_box(chosen, look)))
        .child(parts::avatar(&account, 40., look, drawn.shown))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(sp(2.))
                .line_height(sp(20.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(8.))
                        .child(
                            parts::username(look)
                                .min_w_0()
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(sp(14.))
                                .line_height(sp(20.))
                                .truncate()
                                .child(format!("@{username}")),
                        )
                        .children(marks.into_iter().map(|mark| mark.line_height(sp(16.)))),
                )
                .children(person.full_name.clone().map(|name| {
                    div()
                        .text_size(sp(13.))
                        .line_height(sp(18.))
                        .text_color(color(palette.secondary))
                        .truncate()
                        .child(name)
                })),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "testing")]
    fn speaking(locale: &str) -> Look {
        use crate::system::Settings;
        Look::of(
            Settings {
                locale: locale.to_string(),
                ..Settings::default()
            },
            false,
        )
    }

    #[cfg(feature = "testing")]
    fn at(moment: &str) -> Epoch {
        let at = chrono::NaiveDateTime::parse_from_str(moment, "%Y-%m-%d %H:%M").unwrap();
        Epoch::new(at.and_utc().timestamp())
    }

    #[cfg(feature = "testing")]
    #[test]
    fn a_scan_of_today_says_its_time_alone_and_one_of_another_day_its_day_too_in_both_languages() {
        // A Thursday.
        let now = at("2026-09-17 10:00");
        let (en, es) = (speaking("en"), speaking("es"));
        for (scanned, english, spanish) in [
            (
                "2026-09-17 15:05",
                "Last scan at 3:05 PM.",
                "Último escaneo a las 15:05.", // i18n-allow
            ),
            (
                "2026-09-17 01:05",
                "Last scan at 1:05 AM.",
                "Último escaneo a la 01:05.", // i18n-allow
            ),
            (
                "2026-09-16 01:05",
                "Last scan yesterday at 1:05 AM.",
                "Último escaneo ayer a la 01:05.", // i18n-allow
            ),
            (
                "2026-09-14 09:30",
                "Last scan on Monday at 9:30 AM.",
                "Último escaneo el lunes a las 09:30.", // i18n-allow
            ),
            (
                "2026-09-10 12:00",
                "Last scan on September 10 at 12:00 PM.",
                "Último escaneo el 10 de septiembre a las 12:00.", // i18n-allow
            ),
            (
                "2025-12-31 23:59",
                "Last scan on December 31, 2025 at 11:59 PM.",
                "Último escaneo el 31 de diciembre de 2025 a las 23:59.", // i18n-allow
            ),
        ] {
            assert_eq!(scan_words(at(scanned), now, &en), english);
            assert_eq!(scan_words(at(scanned), now, &es), spanish);
        }
    }

    #[test]
    fn the_hour_is_the_digits_a_time_begins_with() {
        assert_eq!(hour_written("01:05"), 1);
        assert_eq!(hour_written("1:05 AM"), 1);
        assert_eq!(hour_written("14:13"), 14);
        assert_eq!(hour_written(""), 0);
    }
}
