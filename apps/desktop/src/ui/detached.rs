//! A list moved out to a window of its own: the same rows, words, pictures
//! and choosing as in the panel (`screens::people::body`), still coming in
//! as the walk reads them, and a click on one opens that profile in the
//! app's window. **The walk is the one the panel started**: moving the list
//! out reads nothing again, and closing its window stops it, as leaving its
//! page does. Escape closes its options, then stops choosing.

use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Context, Entity, FocusHandle, Render, Result, Subscription,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div, size,
};

use super::Dismiss;
use super::look::{Look, color};
use super::screens::people;
use super::shell::{PeopleList, Shell};
use crate::theme::fonts::SANS;
use crate::ui::look::sp;

/// How large the window opens, in logical pixels.
const WIDTH: f32 = 440.0;
const HEIGHT: f32 = 640.0;

/// The view of a list's own window.
pub struct Detached {
    list: Entity<PeopleList>,
    /// The app's view and its window, which a click on a row opens a
    /// profile in.
    shell: WeakEntity<Shell>,
    main: AnyWindowHandle,
    title: FocusHandle,
    _watching: [Subscription; 3],
}

impl Render for Detached {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::now(cx).clone();
        let palette = look.palette;
        let (shell, main) = (self.shell.clone(), self.main);
        let copy = {
            let (shell, list) = (shell.clone(), self.list.clone());
            Rc::new(move |_: &mut Window, cx: &mut App| {
                shell
                    .update(cx, |shell, cx| shell.copy_the_usernames(&list, cx))
                    .ok();
            })
        };
        let open: people::Open = Rc::new(move |who, _, cx| {
            main.update(cx, |_, window, cx| {
                window.activate_window();
                shell
                    .update(cx, |shell, cx| shell.open_profile(who, window, cx))
                    .ok();
            })
            .ok();
        });
        let acts = people::Acts {
            open,
            copy,
            detach: None,
        };
        let list = self.list.clone();
        div()
            .id("detached")
            .key_context("Snob")
            .on_action(move |_: &Dismiss, window, cx| {
                list.update(cx, |list, cx| list.dismiss(window, cx));
            })
            .size_full()
            .flex()
            .flex_col()
            .bg(color(palette.panel))
            .font_family(SANS)
            .text_size(sp(15.))
            .text_color(color(palette.text))
            .child(people::body(
                &self.list,
                acts,
                &self.title,
                &look,
                window,
                cx,
            ))
    }
}

/// Opens `list` in a window of its own, its title the list's. The window
/// closes with the app's.
pub fn open(
    list: Entity<PeopleList>,
    shell: WeakEntity<Shell>,
    main: AnyWindowHandle,
    cx: &mut App,
) -> Result<AnyWindowHandle> {
    let look = Look::now(cx).clone();
    let named = {
        let read = list.read(cx);
        people::title(&read.owner, read.side, &look)
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(gpui_kit::px(WIDTH), gpui_kit::px(HEIGHT)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("{named} \u{2014} snob").into()),
            ..Default::default()
        }),
        window_min_size: Some(size(gpui_kit::px(320.), gpui_kit::px(360.))),
        app_id: Some(super::window::APP_ID.to_string()),
        ..Default::default()
    };
    let (handle, _) = gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| {
            let redraw = cx.observe(&list, |_, _, cx| cx.notify());
            // Looked at or away from, the app's rest is judged again: this
            // window in front keeps it awake, and its pictures coming.
            let looked = {
                let shell = shell.clone();
                cx.observe_window_activation(window, move |_, _, cx| {
                    let shell = shell.clone();
                    cx.defer(move |cx| {
                        main.update(cx, |_, window, cx| {
                            shell
                                .update(cx, |shell, cx| shell.stirred_elsewhere(window, cx))
                                .ok();
                        })
                        .ok();
                    });
                })
            };
            // The app's window gone, its view goes, and this window with it.
            let gone = match shell.upgrade() {
                Some(shell) => cx.observe_release_in(&shell, window, |_, _, window, _| {
                    window.remove_window();
                }),
                None => cx.observe(&list, |_, _, _| {}),
            };
            Detached {
                list,
                shell,
                main,
                title: cx.focus_handle(),
                _watching: [redraw, gone, looked],
            }
        })
    })?;
    Ok(handle)
}
