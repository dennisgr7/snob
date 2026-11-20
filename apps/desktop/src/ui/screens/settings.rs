//! The settings: a header that stays, with the title and a search, over a
//! list that scrolls on its own, in sections (`state::settings`). The
//! app's theme, language and log; what snob keeps on this computer, the
//! disk it is on, and what can be cleared or deleted here: each account's
//! cache, and its history.
//!
//! **What is kept is measured from the files' sizes**, no database opened
//! (`snob_engine::storage::measure`). **Clearing removes the cache and
//! nothing else**, and asks nothing. **Deleting history asks first**, says
//! it cannot be undone, and never touches a session, a browser or what
//! paces an account (`snob_engine::storage::forget_history`).

use gpui_kit::base::{
    Align, Button, ElementExt as _, FocusTrapElement as _, ObservedElement, POPUP_PRIORITY,
    Placement, Positioner, Scrollbar, ScrollbarMode, TestSupportExt as _,
};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, FocusHandle, FontWeight, MouseDownEvent, Role, SharedString,
    Stateful, Toggled, Window, deferred, div, px, relative,
};
use snob_core::Pk;
use snob_engine::storage::Kind;

use super::home;
use crate::backend::{AccountStorage, Storage};
use crate::state::prefs::ThemeChoice;
use crate::state::settings::{self as listed, Row, Section};
use crate::state::storage;
use crate::theme::motion;
use crate::theme::tokens::{frame, shape};
use crate::ui::assets;
use crate::ui::frame::clock;
use crate::ui::look::{Look, color, color_at, sp};
use crate::ui::parts::{self, Kind as ButtonKind, icon};
use crate::ui::scroll;
use crate::ui::shell::{Done, Phase, Shell};

/// How wide the sections are at most.
const WIDTH: f32 = 680.0;

/// The languages offered: the system's, then each the app speaks, named in
/// itself.
pub(crate) const LANGUAGES: [(Option<&str>, &str); 3] = [
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

/// A kind of what is kept: its id, its name and what it is, as message ids.
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

/// `bytes` in words, in the language spoken.
fn sized(bytes: u64, look: &Look) -> String {
    storage::said(bytes, look.texts.language())
}

/// What is kept, said as a total.
fn total_words(storage: &Storage, look: &Look) -> String {
    let size = sized(storage::total(&storage.kinds), look);
    look.tr_with("settings-storage-total", &[("size", &size)])
}

/// The disk's bar, in words: its size and its three shares.
fn disk_words(storage: &Storage, look: &Look) -> String {
    let Some(disk) = storage.disk else {
        return look.tr("storage-disk-unknown");
    };
    let (others, snob, free) = storage::shares(disk, storage::total(&storage.kinds));
    look.tr_with(
        "storage-disk-said",
        &[
            ("size", &sized(disk.total, look)),
            ("others", &sized(others, look)),
            ("snob", &sized(snob, look)),
            ("free", &sized(free, look)),
        ],
    )
}

/// What clearing or deleting did, in words.
pub(crate) fn done_words(done: Done, look: &Look) -> String {
    match done {
        Done::Cleared(cleared) => {
            let id = if cleared.whole {
                "settings-cleared"
            } else {
                "settings-cleared-partly"
            };
            look.tr_with(id, &[("size", &sized(cleared.freed.bytes, look))])
        }
        Done::Forgot(forgot) => {
            let size = sized(forgot.freed.bytes, look);
            if forgot.failed > 0 {
                let count = forgot.failed.to_string();
                look.tr_with("forget-failed", &[("size", &size), ("count", &count)])
            } else if forgot.busy > 0 {
                let count = forgot.busy.to_string();
                look.tr_with("forget-busy", &[("size", &size), ("count", &count)])
            } else {
                look.tr_with("forget-done", &[("size", &size)])
            }
        }
    }
}

/// An account of what is kept, by its pk.
fn held(storage: Option<&Storage>, pk: Pk) -> Option<&AccountStorage> {
    storage?.accounts.iter().find(|held| held.pk == pk)
}

/// The name an account's line says.
fn account_name(storage: Option<&Storage>, pk: Pk) -> String {
    held(storage, pk)
        .and_then(|held| held.username.as_ref())
        .map_or_else(|| pk.to_string(), |name| format!("@{name}"))
}

/// What an account's line says under its name.
fn account_note(held: &AccountStorage, look: &Look) -> String {
    look.tr(if held.signed_in {
        "storage-account-note"
    } else {
        "storage-account-signed-out"
    })
}

/// What an account holds, in words.
fn account_sizes(held: &AccountStorage, look: &Look) -> String {
    look.tr_with(
        "storage-account-sizes",
        &[
            ("history", &sized(held.history.bytes, look)),
            ("cache", &sized(held.cache.bytes, look)),
        ],
    )
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
        Row::Logs => all(&[
            "settings-logs",
            "settings-logs-note",
            "settings-open-logs",
            "settings-copy-report",
            "settings-detailed",
            "settings-detailed-note",
            "settings-logs-keywords",
        ]),
        Row::StorageDisk => {
            let said = storage.map(|storage| {
                format!(
                    "{} {}",
                    total_words(storage, look),
                    disk_words(storage, look)
                )
            });
            format!(
                "{} {}",
                said.unwrap_or_default(),
                all(&[
                    "settings-storage-note",
                    "settings-storage-keywords",
                    "storage-disk-others",
                    "storage-disk-free",
                ])
            )
        }
        Row::StorageKind(kind) => {
            let (name, note) = kind_words(kind);
            all(&[name, note])
        }
        Row::Accounts => all(&[
            "settings-accounts",
            "settings-accounts-note",
            "settings-clear-cache",
            "settings-forget-all",
            "storage-account-keywords",
        ]),
        Row::Account(pk) => {
            let note = held(storage, pk).map(|held| account_note(held, look));
            format!(
                "{} {} {}",
                account_name(storage, pk),
                note.unwrap_or_default(),
                all(&[
                    "storage-account-clear",
                    "storage-account-forget",
                    "storage-cache",
                    "storage-account-keywords",
                ])
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

/// What a line is and what it says under it.
fn words_of(id: &str, label: String, note: Option<String>, look: &Look) -> Div {
    let palette = look.palette;
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
        }))
}

/// A line of a section: what it is, what it says under it, and its control.
fn line(id: &str, label: String, note: Option<String>, first: bool, look: &Look) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(14.))
        .px(sp(18.))
        .py(sp(12.))
        .when(!first, |line| {
            line.border_t_1().border_color(color(look.palette.line))
        })
        .child(words_of(id, label, note, look))
}

/// A line that belongs to the one above it: inside the same group, under a
/// separator that does not reach the card's edges.
fn sub_line(id: &str, label: String, note: Option<String>, look: &Look) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(14.))
        .mx(sp(18.))
        .py(sp(12.))
        .border_t_1()
        .border_color(color(look.palette.line))
        .child(words_of(id, label, note, look))
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

/// The language field, and while it is open the languages, drawn over
/// everything under it and put beneath the field where it was last drawn.
/// A press outside them closes them; a choice or Escape too, focus back on
/// the field.
fn languages(
    page: &crate::ui::shell::SettingsPage,
    chosen: Option<&str>,
    look: &Look,
    cx: &mut Context<Shell>,
) -> Div {
    let palette = look.palette;
    let open = page.choosing_language;
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
    let bounds = page.language_bounds.clone();
    let under = bounds.get();
    let list = open.then(|| {
        let outside = cx.listener({
            let bounds = bounds.clone();
            move |shell, event: &MouseDownEvent, _, cx| {
                // A press on the field toggles it by itself.
                if !bounds.get().contains(&event.position) {
                    shell.close_the_languages(cx);
                }
            }
        });
        let options = page.language_options.clone();
        deferred(
            Positioner::side(under)
                .placement(Placement::Bottom)
                .align(Align::Start)
                .offset(px(6.))
                .margin(px(8.))
                .occlude()
                .child(
                    div()
                        .id("settings-languages")
                        .test_support()
                        .role(Role::ListBox)
                        .aria_label(field.clone())
                        .occlude()
                        .w(under.size.width)
                        .p(sp(4.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .rounded(px(shape::CONTROL_RADIUS + 2.))
                        .border_1()
                        .border_color(color(palette.control_line))
                        .bg(color(palette.panel))
                        .shadow_lg()
                        .on_mouse_down_out(outside)
                        .children(LANGUAGES.iter().enumerate().map(|(at_, (tag, id))| {
                            let on = at_ == at;
                            let tag = tag.map(str::to_string);
                            let name = look.tr(id);
                            option(("settings-language-option", at_).into(), on)
                                .track_focus(&options[at_])
                                .role(Role::ListBoxOption)
                                .accessibility_label(name.clone())
                                .aria_selected(on)
                                // The one chosen stands out by itself: a
                                // wash, a bar and a heavier name.
                                .when(on, |row| {
                                    row.relative()
                                        .bg(color_at(palette.text, 0.06))
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
                ),
        )
        .with_priority(POPUP_PRIORITY)
    });
    div()
        .flex_none()
        .w(sp(210.))
        .on_prepaint(move |drawn, window, _| {
            if bounds.replace(drawn) != drawn && open {
                // The list follows the field on the next frame.
                crate::ui::frames::ask(window);
            }
        })
        .child(
            option("settings-language".into(), false)
                .track_focus(&page.language_field)
                .role(Role::ComboBox)
                .accessibility_label(format!("{field}: {name}"))
                .aria_expanded(open)
                .border_1()
                .border_color(color(palette.control_line))
                .bg(color(palette.panel))
                .child(name)
                .child(div().flex_1())
                .child(icon(assets::CHEVRON, 14.).text_color(color(palette.secondary)))
                .on_click(cx.listener(|shell, _, window, cx| {
                    shell.toggle_the_languages(window, cx);
                })),
        )
        .children(list)
}

/// A size, said at the end of a line.
fn size(bytes: u64, look: &Look) -> Div {
    div()
        .flex_none()
        .text_size(sp(13.5))
        .text_color(color(look.palette.secondary))
        .child(sized(bytes, look))
}

/// A square of the legend: filled with `fill`, or with none an empty one.
fn swatch(fill: Option<gpui_kit::Hsla>, look: &Look) -> Div {
    let square = div().flex_none().size(sp(10.)).rounded(px(2.));
    match fill {
        Some(fill) => square.bg(fill),
        None => square
            .border_1()
            .border_color(color(look.palette.control_line)),
    }
}

/// The disk snob's data is on, as one bar: what everything else takes, in
/// a neutral gray; what snob takes, in the accent, never too thin to see;
/// and what is free, the empty track. A legend with the three sizes under
/// it.
fn disk(storage: &Storage, look: &Look) -> Option<Div> {
    let palette = look.palette;
    let disk = storage.disk.filter(|disk| disk.total > 0)?;
    let (others, snob, free) = storage::shares(disk, storage::total(&storage.kinds));
    let share = |bytes: u64| bytes as f32 / disk.total as f32;
    let neutral = color(palette.control_line);
    let accent = color(palette.accent);
    let bar = div()
        .id("storage-disk")
        .test_support()
        .role(Role::Meter)
        .aria_label(disk_words(storage, look))
        .w_full()
        .h(sp(12.))
        .flex()
        .rounded_full()
        .overflow_hidden()
        .border_1()
        .border_color(color(palette.control_line))
        // Each share rounded where it ends the bar: hiding the overflow
        // clips to a rectangle, not to the bar's rounded ends.
        .child(
            div()
                .h_full()
                .w(relative(share(others)))
                .rounded_l_full()
                .when(snob == 0 && free == 0, |share| share.rounded_r_full())
                .bg(neutral),
        )
        .when(snob > 0, |bar| {
            bar.child(
                div()
                    .h_full()
                    .w(relative(share(snob)))
                    .min_w(px(4.))
                    .when(others == 0, |share| share.rounded_l_full())
                    .when(free == 0, |share| share.rounded_r_full())
                    .bg(accent),
            )
        });
    let key = |id: &str, fill, name: String, bytes: u64| {
        div()
            .id(SharedString::from(id.to_string()))
            .test_support()
            .role(Role::Label)
            .aria_label(format!("{name} {}", sized(bytes, look)))
            .flex()
            .items_center()
            .gap(sp(6.))
            .child(swatch(fill, look))
            .child(div().text_color(color(palette.secondary)).child(name))
            .child(
                div()
                    .text_color(color(palette.text))
                    .font_weight(FontWeight::MEDIUM)
                    .child(sized(bytes, look)),
            )
    };
    let legend = div()
        .flex()
        .flex_wrap()
        .gap_x(sp(18.))
        .gap_y(sp(4.))
        .text_size(sp(13.))
        .child(key(
            "storage-disk-others",
            Some(neutral),
            look.tr("storage-disk-others"),
            others,
        ))
        .child(key(
            "storage-disk-snob",
            Some(accent),
            look.tr("storage-disk-snob"),
            snob,
        ))
        .child(key(
            "storage-disk-free",
            None,
            look.tr("storage-disk-free"),
            free,
        ));
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(10.))
            .child(bar)
            .child(legend),
    )
}

/// The buttons of a line, and a ring while what they do runs.
fn actions(id: &str, working: bool, look: &Look, window: &mut Window) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(8.))
        .when(working, |it| {
            it.child(
                div()
                    .id(SharedString::from(format!("{id}-working")))
                    .test_support()
                    .role(Role::ProgressIndicator)
                    .aria_label(look.tr("storage-working"))
                    .child(parts::ring(16., look, window)),
            )
        })
}

/// An account no longer signed in: its initial on a quiet circle, since no
/// picture of it is kept.
fn left_behind(name: &str, look: &Look) -> Div {
    let palette = look.palette;
    let letter = name
        .trim_start_matches('@')
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    div()
        .size(px(28.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(color(palette.control_line))
        .flex()
        .items_center()
        .justify_center()
        .font_weight(FontWeight::BOLD)
        .text_size(px(12.5))
        .text_color(color(palette.secondary))
        .child(letter)
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
    let palette = look.palette;
    let storage = page.storage.as_ref();
    let busy = shell.storage_work.is_some();
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
        .child(languages(page, look.chosen.language.as_deref(), look, cx))
        .into_any_element(),
        Row::Logs => {
            // Said for as long as the screen is shown, not for two seconds.
            let note = if shell.report_copied {
                look.tr("settings-report-copied")
            } else {
                look.tr("settings-logs-note")
            };
            let until = shell.backend().detailed_until();
            let detailed = match until {
                Some(until) => look.tr_with("settings-detailed-until", &[("time", &clock(until))]),
                None => look.tr("settings-detailed-note"),
            };
            div()
                .flex()
                .flex_col()
                .when(!first, |group| {
                    group.border_t_1().border_color(color(palette.line))
                })
                .child(
                    line(
                        "settings-logs-label",
                        look.tr("settings-logs"),
                        Some(note),
                        true,
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
                                .on_click(cx.listener(
                                    |shell, _, window, cx| {
                                        shell.copy_the_report(window, cx);
                                    },
                                )),
                            ),
                    ),
                )
                .child(
                    sub_line(
                        "settings-detailed-label",
                        look.tr("settings-detailed"),
                        Some(detailed),
                        look,
                    )
                    .child(switch(until.is_some(), look, cx)),
                )
                .into_any_element()
        }
        Row::StorageDisk => {
            let Some(storage) = storage else {
                return line(
                    "settings-storage-total",
                    look.tr("settings-storage-measuring"),
                    Some(look.tr("settings-storage-note")),
                    first,
                    look,
                )
                .child(div().flex_none().child(parts::ring(18., look, window)))
                .into_any_element();
            };
            let note = match storage.disk {
                Some(disk) => {
                    look.tr_with("storage-disk-of", &[("size", &sized(disk.total, look))])
                }
                None => look.tr("storage-disk-unknown"),
            };
            div()
                .flex()
                .flex_col()
                .gap(sp(12.))
                .px(sp(18.))
                .py(sp(14.))
                .when(!first, |group| {
                    group.border_t_1().border_color(color(palette.line))
                })
                .child(div().flex().flex_wrap().items_center().child(words_of(
                    "settings-storage-total",
                    total_words(storage, look),
                    Some(note),
                    look,
                )))
                .children(disk(storage, look))
                .child(
                    said(
                        "settings-storage-note".into(),
                        look.tr("settings-storage-note"),
                    )
                    .text_size(sp(12.5))
                    .text_color(color(palette.secondary)),
                )
                .into_any_element()
        }
        Row::StorageKind(kind) => {
            let (name, note) = kind_words(kind);
            let bytes = storage
                .and_then(|storage| storage.kinds.iter().find(|(k, _)| *k == kind))
                .map_or(0, |(_, size)| size.bytes);
            line(name, look.tr(name), Some(look.tr(note)), first, look)
                .child(size(bytes, look))
                .into_any_element()
        }
        Row::Accounts => {
            let note = match shell.storage_done {
                Some((None, done)) => done_words(done, look),
                _ => look.tr("settings-accounts-note"),
            };
            let cache = storage.map_or(0, |storage| {
                storage
                    .kinds
                    .iter()
                    .filter(|(kind, _)| kind.clearable())
                    .map(|(_, size)| size.bytes)
                    .sum()
            });
            let history = storage
                .is_some_and(|storage| storage.accounts.iter().any(|held| held.history.bytes > 0));
            let everyone = look.tr("settings-forget-all");
            line(
                "settings-accounts",
                look.tr("settings-accounts"),
                Some(note),
                first,
                look,
            )
            .child(
                actions(
                    "settings-accounts",
                    shell.storage_work == Some(None),
                    look,
                    window,
                )
                .child(
                    parts::small_button(
                        "settings-clear-cache",
                        look.tr("settings-clear-cache"),
                        cache > 0 && !busy,
                        look,
                    )
                    .on_click(cx.listener(|shell, _, window, cx| {
                        shell.clear_the_cache(None, window, cx);
                    })),
                )
                .child(
                    parts::small_button(
                        "settings-forget-all",
                        everyone.clone(),
                        history && !busy,
                        look,
                    )
                    .on_click(cx.listener(move |shell, _, window, cx| {
                        shell.ask_to_forget(None, everyone.clone(), window, cx);
                    })),
                ),
            )
            .into_any_element()
        }
        Row::Account(pk) => {
            let Some(held) = held(storage, pk) else {
                return div().into_any_element();
            };
            let name = account_name(storage, pk);
            let at = storage
                .and_then(|storage| storage.accounts.iter().position(|held| held.pk == pk))
                .unwrap_or_default();
            let id = format!("storage-account-{at}");
            let note = match shell.storage_done {
                Some((Some(done_for), done)) if done_for == pk => done_words(done, look),
                _ => account_note(held, look),
            };
            let picture = shell
                .accounts()
                .iter()
                .find(|account| account.named.pk == pk)
                .map_or_else(
                    || left_behind(&name, look),
                    |account| {
                        let shown = parts::shown_of(shell.pictures_shown(), account);
                        parts::avatar(account, 28., look, shown).flex_none()
                    },
                );
            let clear = parts::small_button(
                format!("{id}-clear"),
                look.tr("storage-account-clear"),
                held.cache.bytes > 0 && !busy,
                look,
            )
            .accessibility_label(look.tr_with("storage-account-clear-named", &[("account", &name)]))
            .on_click(cx.listener(move |shell, _, window, cx| {
                shell.clear_the_cache(Some(pk), window, cx);
            }));
            let asked = name.clone();
            let forget = parts::small_button(
                format!("{id}-forget"),
                look.tr("storage-account-forget"),
                held.history.bytes > 0 && !busy,
                look,
            )
            .accessibility_label(
                look.tr_with("storage-account-forget-named", &[("account", &name)]),
            )
            .on_click(cx.listener(move |shell, _, window, cx| {
                shell.ask_to_forget(Some(pk), asked.clone(), window, cx);
            }));
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
                .child(picture)
                .child(words_of(&id, name, Some(note), look).min_w(sp(160.)))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(sp(6.))
                        .child(
                            said(format!("{id}-sizes").into(), account_sizes(held, look))
                                .text_size(sp(13.))
                                .text_color(color(palette.secondary)),
                        )
                        .child(
                            actions(&id, shell.storage_work == Some(Some(pk)), look, window)
                                .child(clear)
                                .child(forget),
                        ),
                )
                .into_any_element()
        }
    }
}

/// The question deleting history asks first, while it is asked: over the
/// window, focus kept inside it and starting on the answer that changes
/// nothing.
pub fn forget_question(
    shell: &mut Shell,
    look: &Look,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let palette = look.palette;
    let asking = shell.settings.as_ref()?.forgetting.as_ref()?;
    let (question, body) = match asking.only {
        Some(_) => (
            look.tr_with("forget-title", &[("account", &asking.name)]),
            look.tr("forget-body"),
        ),
        None => (look.tr("forget-title-all"), look.tr("forget-body-all")),
    };
    let (trap, cancel): (FocusHandle, FocusHandle) = (asking.focus.clone(), asking.cancel.clone());
    let dialog = div()
        .id("forget")
        .test_support()
        .role(Role::AlertDialog)
        .aria_label(question.clone())
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
                        .id("forget-title")
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
                        .child(said("forget-body".into(), body)),
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
                                "forget-cancel",
                                look.tr("cancel"),
                                ButtonKind::Secondary,
                                true,
                                look,
                            )
                            .track_focus(&cancel)
                            .on_click(cx.listener(
                                |shell, _, window, cx| {
                                    shell.keep_the_history(window, cx);
                                },
                            )),
                        )
                        .child(
                            parts::button(
                                "forget-confirm",
                                look.tr("forget-confirm"),
                                ButtonKind::Primary,
                                true,
                                look,
                            )
                            .on_click(cx.listener(
                                |shell, _, window, cx| {
                                    shell.forget_the_history(window, cx);
                                },
                            )),
                        ),
                )
                .focus_trap("forget-answers", &trap),
        );
    Some(
        div()
            .id("forget-backdrop")
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
        crate::ui::frames::ask(window);
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
    // The toolkit keeps a child's bounds as laid out before the scroll, so
    // its distance from the viewport's top is already its top in the list.
    let tops: Vec<f32> = (0..shown.len())
        .map(|at| {
            list.handle
                .bounds_for_item(first + at)
                .map_or(0.0, |item| f32::from(item.origin.y - viewport.origin.y))
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
        // Shown while there is more than is in sight, never only while
        // scrolled: what is out of sight is never cut off without a sign.
        .mode(ScrollbarMode::Always)
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
                        // The languages are put under their field where it
                        // was: a scroll that moves it closes them.
                        .on_scroll(cx.listener(|shell, _, _, cx| {
                            shell.close_the_languages(cx);
                        }))
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
