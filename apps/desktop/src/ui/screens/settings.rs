//! The settings: a header that stays, with the title and a search, over a
//! list that scrolls on its own, in sections (`state::settings`). The
//! app's theme, language and log; what snob keeps on this computer and the
//! one part of it that can be cleared here, the cache.
//!
//! **What is kept is measured from the files' sizes**, no database opened
//! (`snob_engine::storage::measure`), and **clearing removes the cache and
//! nothing else**: never a session, a stored list or a log.

use gpui_kit::base::{Button, ObservedElement, Scrollbar, TestSupportExt as _};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, Role, SharedString, Stateful, Toggled, Window, div, px,
};
use snob_engine::storage::Kind;

use super::home;
use crate::backend::{Cleared, Storage};
use crate::state::prefs::ThemeChoice;
use crate::state::settings::{self as listed, Row, Section};
use crate::state::storage;
use crate::theme::motion;
use crate::theme::tokens::shape;
use crate::ui::assets;
use crate::ui::frame::clock;
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, icon};
use crate::ui::scroll;
use crate::ui::shell::{Phase, Shell};

/// How wide the sections are at most.
const WIDTH: f32 = 680.0;

/// The languages offered: the system's, then each the app speaks, named in
/// itself.
const LANGUAGES: [(Option<&str>, &str); 3] = [
    (None, "language-system"),
    (Some("en"), "language-en"),
    (Some("es"), "language-es"),
];

/// A theme's option: its id, and the message of its name.
fn theme_id(theme: ThemeChoice) -> &'static str {
    match theme {
        ThemeChoice::System => "theme-system",
        ThemeChoice::Light => "theme-light",
        ThemeChoice::Dark => "theme-dark",
    }
}

/// A kind of what is kept: its name and what it is, as message ids.
fn kind_words(kind: Kind) -> (&'static str, &'static str) {
    match kind {
        Kind::Databases => ("storage-databases", "storage-databases-note"),
        Kind::Cache => ("storage-cache", "storage-cache-note"),
        Kind::BrowserProfiles => ("storage-browsers", "storage-browsers-note"),
        Kind::Jobs => ("storage-jobs", "storage-jobs-note"),
        Kind::Logs => ("storage-logs", "storage-logs-note"),
        Kind::Update => ("storage-update", "storage-update-note"),
        Kind::Scratch => ("storage-scratch", "storage-scratch-note"),
        Kind::Other => ("storage-other", "storage-other-note"),
    }
}

/// A section's heading.
fn heading(section: Section, look: &Look) -> String {
    match section {
        Section::App => look.tr("settings-app"),
        Section::Storage => look.tr("settings-storage"),
    }
}

/// A section's heading, as its id.
fn heading_id(section: Section) -> &'static str {
    match section {
        Section::App => "settings-app",
        Section::Storage => "settings-storage",
    }
}

/// What is kept, said as a total.
fn total_words(storage: &Storage, look: &Look) -> String {
    let size = storage::said(storage::total(&storage.kinds), look.texts.language());
    look.tr_with("settings-storage-total", &[("size", &size)])
}

/// What clearing the cache did, in words.
pub fn cleared_words(cleared: Cleared, look: &Look) -> String {
    let size = storage::said(cleared.freed.bytes, look.texts.language());
    let id = if cleared.whole {
        "settings-cleared"
    } else {
        "settings-cleared-partly"
    };
    look.tr_with(id, &[("size", &size)])
}

/// The name an account's line says.
fn account_name(storage: &Storage, pk: snob_core::Pk) -> String {
    storage
        .accounts
        .iter()
        .find(|held| held.pk == pk)
        .and_then(|held| held.username.as_ref())
        .map_or_else(|| pk.to_string(), |name| format!("@{name}"))
}

/// Everything a row is called, for the search: what it says and what it
/// may be looked for by.
fn words(row: Row, storage: Option<&Storage>, look: &Look) -> String {
    let all = |ids: &[&str]| {
        ids.iter()
            .map(|id| look.tr(id))
            .collect::<Vec<_>>()
            .join(" ")
    };
    match row {
        Row::Theme => all(&[
            "settings-theme",
            "settings-theme-note",
            "theme-system",
            "theme-light",
            "theme-dark",
            "settings-theme-keywords",
        ]),
        Row::Language => all(&[
            "settings-language",
            "settings-language-note",
            "language-system",
            "language-en",
            "language-es",
            "settings-language-keywords",
        ]),
        Row::DetailedLogging => all(&[
            "settings-detailed",
            "settings-detailed-note",
            "settings-logs-keywords",
        ]),
        Row::Logs => all(&[
            "settings-logs",
            "settings-logs-note",
            "settings-open-logs",
            "settings-copy-report",
            "settings-logs-keywords",
        ]),
        Row::StorageTotal => {
            let total = storage.map(|storage| total_words(storage, look));
            format!(
                "{} {}",
                total.unwrap_or_default(),
                all(&["settings-storage-note", "settings-storage-keywords"])
            )
        }
        Row::StorageKind(kind) => {
            let (name, note) = kind_words(kind);
            let clear = if kind.clearable() {
                "settings-clear-cache"
            } else {
                name
            };
            all(&[name, note, clear])
        }
        Row::Account(pk) => {
            let name = storage.map(|storage| account_name(storage, pk));
            format!(
                "{} {}",
                name.unwrap_or_default(),
                look.tr("storage-account-note")
            )
        }
    }
}

/// Words of a line, as a screen reader and a test find them.
fn said(id: SharedString, text: String) -> ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .role(Role::Label)
        .aria_label(text.clone())
        .child(text)
}

/// A line of a section: what it is, what it says under it, and its control.
fn line(id: &str, label: String, note: Option<String>, first: bool, look: &Look) -> Div {
    let palette = look.palette;
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(14.))
        .px(sp(18.))
        .py(sp(12.))
        .when(!first, |line| {
            line.border_t_1().border_color(color(palette.line))
        })
        .child(
            div()
                .flex_1()
                .min_w(sp(200.))
                .flex()
                .flex_col()
                .gap(sp(2.))
                .child(
                    said(SharedString::from(id.to_string()), label)
                        .text_size(sp(14.5))
                        .text_color(color(palette.text)),
                )
                .children(note.map(|note| {
                    said(SharedString::from(format!("{id}-note")), note)
                        .text_size(sp(13.))
                        .line_height(sp(19.))
                        .text_color(color(palette.secondary))
                })),
        )
}

/// The switch of the detailed log: its track and thumb, its state said.
fn switch(detailed: bool, look: &Look, cx: &mut Context<Shell>) -> Button {
    let palette = look.palette;
    Button::new("settings-detailed")
        .role(Role::Switch)
        .accessibility_label(look.tr("settings-detailed"))
        .aria_toggled(if detailed {
            Toggled::True
        } else {
            Toggled::False
        })
        .flex_none()
        .w(px(46.))
        .h(px(26.))
        .p(px(2.))
        .rounded_full()
        .border_1()
        .border_color(color(palette.control_line))
        .bg(color(if detailed {
            palette.accent
        } else {
            palette.panel
        }))
        .cursor_pointer()
        .focus_visible(parts::focus_ring(&palette))
        .justify_start()
        .on_click(cx.listener(move |shell, _, _, cx| shell.set_detailed(!detailed, cx)))
        .child(
            div()
                .size(px(20.))
                .rounded_full()
                .bg(color(if detailed {
                    palette.on_accent
                } else {
                    palette.control_line
                }))
                .when(detailed, |thumb| thumb.ml(px(20.))),
        )
}

/// The language field, and under it, while it is open, the languages.
fn languages(chosen: Option<&str>, open: bool, look: &Look, cx: &mut Context<Shell>) -> Div {
    let palette = look.palette;
    let at = LANGUAGES
        .iter()
        .position(|(tag, _)| *tag == chosen)
        .unwrap_or(0);
    let name = look.tr(LANGUAGES[at].1);
    let field = look.tr("settings-language");
    let option = |id: gpui_kit::ElementId, on: bool| {
        Button::new(id)
            .w_full()
            .h(sp(32.))
            .px(sp(10.))
            .gap(sp(8.))
            .justify_start()
            .rounded(px(shape::CONTROL_RADIUS))
            .text_size(sp(13.5))
            .text_color(color(palette.text))
            .when(!on, |row| {
                row.hover(move |style| style.bg(color_at(palette.text, 0.08)))
            })
            .focus_visible(parts::focus_ring(&palette))
            .cursor_pointer()
    };
    div()
        .flex_none()
        .w(sp(210.))
        .flex()
        .flex_col()
        .gap(sp(4.))
        .child(
            option("settings-language".into(), false)
                .role(Role::ComboBox)
                .accessibility_label(format!("{field}: {name}"))
                .aria_expanded(open)
                .border_1()
                .border_color(color(palette.control_line))
                .bg(color(palette.panel))
                .child(name)
                .child(div().flex_1())
                .child(icon(assets::CHEVRON, 14.).text_color(color(palette.secondary)))
                .on_click(cx.listener(|shell, _, _, cx| shell.toggle_the_languages(cx))),
        )
        .when(open, |field_and_list| {
            field_and_list.child(
                div()
                    .id("settings-languages")
                    .test_support()
                    .role(Role::ListBox)
                    .aria_label(field)
                    .p(sp(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .rounded(px(shape::CONTROL_RADIUS + 2.))
                    .border_1()
                    .border_color(color(palette.control_line))
                    .bg(color(palette.frame))
                    .children(LANGUAGES.iter().enumerate().map(|(at_, (tag, id))| {
                        let on = at_ == at;
                        let tag = tag.map(str::to_string);
                        let name = look.tr(id);
                        option(("settings-language-option", at_).into(), on)
                            .role(Role::ListBoxOption)
                            .accessibility_label(name.clone())
                            .aria_selected(on)
                            // The one chosen stands out by itself: the
                            // panel's color, a bar and a heavier name.
                            .when(on, |row| {
                                row.relative()
                                    .bg(color(palette.panel))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top(sp(8.))
                                            .bottom(sp(8.))
                                            .w(px(3.))
                                            .rounded_r(px(2.))
                                            .bg(color(palette.text)),
                                    )
                            })
                            .child(name)
                            .on_click(cx.listener(move |shell, _, window, cx| {
                                shell.choose_language(tag.clone(), window, cx);
                            }))
                    })),
            )
        })
}

/// A size, said at the end of a line.
fn size(bytes: u64, look: &Look) -> Div {
    div()
        .flex_none()
        .text_size(sp(13.5))
        .text_color(color(look.palette.secondary))
        .child(storage::said(bytes, look.texts.language()))
}

/// One line of a section.
fn row(
    shell: &Shell,
    row: Row,
    first: bool,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let Some(page) = &shell.settings else {
        return div().into_any_element();
    };
    let storage = page.storage.as_ref();
    match row {
        Row::Theme => {
            let chosen = look.chosen.theme;
            let label = look.tr("settings-theme");
            line(
                "settings-theme-label",
                label.clone(),
                Some(look.tr("settings-theme-note")),
                first,
                look,
            )
            .child(
                parts::segmented("settings-theme", label, look).children(ThemeChoice::ALL.map(
                    |theme| {
                        let id = theme_id(theme);
                        parts::segment(id, look.tr(id), theme == chosen, look).on_click(
                            cx.listener(move |shell, _, _, cx| shell.choose_theme(theme, cx)),
                        )
                    },
                )),
            )
            .into_any_element()
        }
        Row::Language => line(
            "settings-language-label",
            look.tr("settings-language"),
            Some(look.tr("settings-language-note")),
            first,
            look,
        )
        .child(languages(
            look.chosen.language.as_deref(),
            page.choosing_language,
            look,
            cx,
        ))
        .into_any_element(),
        Row::DetailedLogging => {
            let until = shell.backend().detailed_until();
            let note = match until {
                Some(until) => look.tr_with("settings-detailed-until", &[("time", &clock(until))]),
                None => look.tr("settings-detailed-note"),
            };
            line(
                "settings-detailed-label",
                look.tr("settings-detailed"),
                Some(note),
                first,
                look,
            )
            .child(switch(until.is_some(), look, cx))
            .into_any_element()
        }
        Row::Logs => {
            // Said for as long as the screen is shown, not for two seconds.
            let note = if shell.report_copied {
                look.tr("settings-report-copied")
            } else {
                look.tr("settings-logs-note")
            };
            line(
                "settings-logs-label",
                look.tr("settings-logs"),
                Some(note),
                first,
                look,
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(sp(8.))
                    .child(
                        parts::small_button(
                            "settings-open-logs",
                            look.tr("settings-open-logs"),
                            true,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, _, cx| shell.open_the_logs(cx))),
                    )
                    .child(
                        parts::small_button(
                            "settings-copy-report",
                            look.tr("settings-copy-report"),
                            true,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.copy_the_report(window, cx);
                        })),
                    ),
            )
            .into_any_element()
        }
        Row::StorageTotal => match storage {
            Some(storage) => line(
                "settings-storage-total",
                total_words(storage, look),
                Some(look.tr("settings-storage-note")),
                first,
                look,
            )
            .into_any_element(),
            None => line(
                "settings-storage-total",
                look.tr("settings-storage-measuring"),
                Some(look.tr("settings-storage-note")),
                first,
                look,
            )
            .child(div().flex_none().child(parts::ring(18., look, window)))
            .into_any_element(),
        },
        Row::StorageKind(kind) => {
            let (name, note) = kind_words(kind);
            let bytes = storage
                .and_then(|storage| storage.kinds.iter().find(|(k, _)| *k == kind))
                .map_or(0, |(_, size)| size.bytes);
            let id = match kind {
                Kind::Databases => "storage-databases",
                Kind::Cache => "storage-cache",
                Kind::BrowserProfiles => "storage-browsers",
                Kind::Jobs => "storage-jobs",
                Kind::Logs => "storage-logs",
                Kind::Update => "storage-update",
                Kind::Scratch => "storage-scratch",
                Kind::Other => "storage-other",
            };
            let note = match page.cleared {
                Some(cleared) if kind.clearable() => cleared_words(cleared, look),
                _ => look.tr(note),
            };
            let line = line(id, look.tr(name), Some(note), first, look).child(size(bytes, look));
            if !kind.clearable() {
                return line.into_any_element();
            }
            let clearing = page.clearing;
            line.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(sp(8.))
                    .when(clearing, |it| it.child(parts::ring(16., look, window)))
                    .child(
                        parts::small_button(
                            "settings-clear-cache",
                            look.tr("settings-clear-cache"),
                            bytes > 0 && !clearing,
                            look,
                        )
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.clear_the_cache(window, cx);
                        })),
                    ),
            )
            .into_any_element()
        }
        Row::Account(pk) => {
            let name = storage.map_or_else(|| pk.to_string(), |storage| account_name(storage, pk));
            let at = storage
                .and_then(|storage| storage.accounts.iter().position(|held| held.pk == pk))
                .unwrap_or_default();
            let bytes = storage
                .and_then(|storage| storage.accounts.iter().find(|held| held.pk == pk))
                .map_or(0, |held| held.total.bytes);
            let picture = shell
                .accounts()
                .iter()
                .find(|account| account.named.pk == pk)
                .map(|account| {
                    let shown = parts::shown_of(shell.pictures_shown(), account);
                    parts::avatar(account, 28., look, shown).flex_none()
                });
            div()
                .relative()
                .child(
                    line(
                        &format!("storage-account-{at}"),
                        name,
                        Some(look.tr("storage-account-note")),
                        first,
                        look,
                    )
                    .when(picture.is_some(), |line| line.pl(sp(58.)))
                    .child(size(bytes, look)),
                )
                .children(picture.map(|picture| {
                    div()
                        .absolute()
                        .left(sp(18.))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .child(picture)
                }))
                .into_any_element()
        }
    }
}

/// How far the outline of the sections has come in, now: in while the
/// thumb is held, out once it is let go. Asks for frames only while it moves.
fn outline_presence(
    shell: &mut Shell,
    look: &Look,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> f32 {
    let Some(page) = &mut shell.settings else {
        return 0.0;
    };
    let dragging = page.scroll.dragging();
    let now = cx.background_executor().now();
    match page.outline {
        Some((was, _)) if was == dragging => {}
        None if !dragging => return 0.0,
        _ => page.outline = Some((dragging, now)),
    }
    let Some((coming, since)) = page.outline else {
        return 0.0;
    };
    let lasts = if look.settings.reduce_motion {
        motion::FADE
    } else {
        motion::OUTLINE_ENTER
    };
    let through = now.saturating_duration_since(since).as_secs_f32() / lasts.as_secs_f32();
    if through < 1.0 {
        window.request_animation_frame();
    }
    let eased = motion::ease_out(through.min(1.0));
    if coming { eased } else { 1.0 - eased }
}

/// Beside the scrollbar while its thumb is held: the headings of the
/// sections shown, each where its section is on the track, the one read now
/// standing out. Never focused: the keyboard has the search.
fn outline(
    shown: &[(Section, Vec<Row>)],
    first: usize,
    list: &scroll::OutlinedScroll,
    presence: f32,
    look: &Look,
) -> Option<AnyElement> {
    if presence <= 0.0 {
        return None;
    }
    let palette = look.palette;
    let viewport = list.handle.bounds();
    let scrolled = -f32::from(list.handle.offset().y);
    let tops: Vec<f32> = (0..shown.len())
        .map(|at| {
            list.handle.bounds_for_item(first + at).map_or(0.0, |item| {
                f32::from(item.origin.y - viewport.origin.y) + scrolled
            })
        })
        .collect();
    let track = f32::from(viewport.size.height);
    let whole = f32::from(list.handle.max_offset().y) + track;
    let (at, read) = scroll::outline(&tops, scrolled, whole, track);
    // The pill's height and a gap below it, in pixels at this text size.
    let pill = 28.0 * look.settings.text_scale();
    Some(
        div()
            .id("settings-outline")
            .test_support()
            .role(Role::Group)
            .aria_label(look.tr("settings-outline"))
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(18.))
            .w(sp(180.))
            .opacity(presence)
            .children(
                shown
                    .iter()
                    .zip(at)
                    .enumerate()
                    .map(|(n, ((section, _), y))| {
                        let here = read == Some(n);
                        div()
                            .absolute()
                            .right_0()
                            .top(px(y.min(track - pill).max(0.)))
                            .h(sp(26.))
                            .px(sp(10.))
                            .flex()
                            .items_center()
                            .gap(sp(6.))
                            .rounded(px(shape::CONTROL_RADIUS))
                            .border_1()
                            .border_color(color(if here {
                                palette.control_line
                            } else {
                                palette.line
                            }))
                            .bg(color(palette.panel))
                            .shadow_sm()
                            .text_size(sp(12.5))
                            .text_color(color(if here {
                                palette.text
                            } else {
                                palette.secondary
                            }))
                            .when(here, |label| {
                                label.font_weight(FontWeight::BOLD).child(
                                    div()
                                        .w(px(3.))
                                        .h(sp(12.))
                                        .rounded(px(2.))
                                        .bg(color(palette.accent)),
                                )
                            })
                            .child(heading(*section, look))
                    }),
            )
            .into_any_element(),
    )
}

pub fn render(
    shell: &mut Shell,
    phase: Phase,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let look = Look::now(cx).clone();
    let palette = look.palette;
    let Some(page) = &shell.settings else {
        return div().into_any_element();
    };
    let (search, scroll) = (page.search.clone(), page.scroll.clone());
    let typed = search.read(cx).value().to_string();
    let storage = page.storage.clone();
    let kept = storage.as_ref().map_or_else(Vec::new, |storage| {
        storage::rows(&storage.kinds, storage.accounts.iter().map(|held| held.pk))
    });
    let shown = listed::shown(
        listed::every(kept),
        &typed,
        |row| words(row, storage.as_ref(), &look),
        |section| heading(section, &look),
    );

    // The search in the middle while the title leaves room for it; under
    // the title when it does not.
    let header = div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(16.))
        .px(sp(32.))
        .pt(sp(26.))
        .pb(sp(16.))
        .child(div().flex_1().flex().child(parts::title(
            &shell.title,
            look.tr("settings-title"),
            &look,
        )))
        .child(
            div().w(sp(300.)).flex_shrink_1().min_w(sp(140.)).child(
                Input::new(&search)
                    .id("settings-search")
                    .aria_label(look.tr("settings-search"))
                    .prefix(icon(assets::SEARCH, 15.).text_color(color(palette.secondary))),
            ),
        )
        .child(div().flex_1());

    let sections: Vec<AnyElement> = shown
        .iter()
        .map(|(section, rows)| {
            let id = heading_id(*section);
            let name = heading(*section, &look);
            div()
                .id(("settings-section", *section as usize))
                .w_full()
                .max_w(sp(WIDTH))
                .flex()
                .flex_col()
                .gap(sp(10.))
                .pb(sp(28.))
                .child(
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Heading)
                        .aria_level(2)
                        .aria_label(name.clone())
                        .px(sp(4.))
                        .font_weight(FontWeight::BOLD)
                        .text_size(sp(16.))
                        .text_color(color(palette.text))
                        .child(name),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .rounded(px(shape::CARD_RADIUS))
                        .border_1()
                        .border_color(color(palette.line))
                        .children(
                            rows.iter()
                                .enumerate()
                                .map(|(at, each)| row(shell, *each, at == 0, &look, window, cx)),
                        ),
                )
                .into_any_element()
        })
        .collect();

    let first = usize::from(shell.crash.is_some());
    let presence = outline_presence(shell, &look, window, cx);
    let outline = outline(&shown, first, &scroll, presence, &look);
    let bar = Scrollbar::vertical(&scroll)
        .id("settings-scrollbar")
        .viewport_from_layout()
        .styles(|styles| {
            styles
                .thumb(|thumb| thumb.bg(color(palette.control_line)))
                .thumb_hover(|thumb| thumb.bg(color(palette.secondary)))
                .thumb_active(|thumb| thumb.bg(color(palette.secondary)))
        });

    let none = shown.is_empty().then(|| {
        let said = look.tr_with("settings-none", &[("typed", typed.trim())]);
        div()
            .id("settings-none")
            .test_support()
            .role(Role::Status)
            .aria_label(said.clone())
            .w_full()
            .max_w(sp(WIDTH))
            .py(sp(24.))
            .text_center()
            .text_size(sp(14.))
            .text_color(color(palette.secondary))
            .child(said)
    });

    div()
        .id("settings")
        .size_full()
        .min_h_0()
        .flex()
        .flex_col()
        .relative()
        .top(px(phase.rise))
        .opacity(phase.presence)
        .child(header)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    div()
                        .id("settings-list")
                        .test_support()
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&scroll.handle)
                        .flex()
                        .flex_col()
                        .items_center()
                        .px(sp(32.))
                        .pt(sp(8.))
                        .children(home::crash(shell, &look, cx).map(|card| card.mb(sp(24.))))
                        .children(sections)
                        .children(none),
                )
                // Over the list, its track kept off the window's edge, where
                // a resize strip may lie.
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left_0()
                        .right(px(6.))
                        .child(bar)
                        .children(outline),
                ),
        )
        .into_any_element()
}
