//! The one window: where it opens, what it is made of before anything is
//! drawn in it, and what it remembers of where it was.
//!
//! **It opens hidden and is shown once its first frame is drawn**, so
//! it is never a white rectangle on a dark desktop, and it opens where it
//! was last, when that place is still on a display.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyWindowHandle, App, Bounds, Entity, Result, TitlebarOptions, Window, WindowBounds,
    WindowDecorations, WindowOptions, point, px, size,
};

use super::look::Look;
use super::shell::Shell;
use crate::backend::Backend;
use crate::state::prefs::{
    Appearance, FIRST_HEIGHT, FIRST_WIDTH, MIN_HEIGHT, MIN_WIDTH, Placement,
};
use crate::system::System;
use crate::theme::fonts;

/// What the system's taskbar or dock groups the app's windows by.
pub const APP_ID: &str = "dev.dennisgr7.snob";

/// Everything the window needs before it opens: the toolkit, the fonts, the
/// keys, and how the app looks on this system with what was `chosen` over
/// it, so its first frame is in the theme it stays in. Once, at the start.
pub fn init(system: &dyn System, chosen: Appearance, testing: bool, cx: &mut App) {
    gpui_kit::init(cx);
    if let Err(e) = cx.text_system().add_fonts(fonts::all()) {
        tracing::error!(error = %e, "the app's fonts could not be loaded");
    }
    super::bind_keys(cx);
    super::look::wear(Look::chosen(system.settings(), chosen, testing), cx);
}

/// Where the window opens: where it was, when that is still on a display,
/// and otherwise in the middle of the main one.
fn bounds(placement: Option<Placement>, cx: &App) -> WindowBounds {
    let screens: Vec<(f32, f32, f32, f32)> = cx
        .displays()
        .iter()
        .map(|display| {
            let bounds = display.bounds();
            (
                bounds.origin.x.into(),
                bounds.origin.y.into(),
                bounds.size.width.into(),
                bounds.size.height.into(),
            )
        })
        .collect();
    match placement.and_then(|placement| placement.usable_on(&screens)) {
        Some(placement) => {
            let bounds = Bounds {
                origin: point(px(placement.x), px(placement.y)),
                size: size(px(placement.width), px(placement.height)),
            };
            if placement.maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            }
        }
        None => WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(FIRST_WIDTH), px(FIRST_HEIGHT)),
            cx,
        )),
    }
}

/// Where the window is now, to open there the next time.
pub fn placement(window: &Window) -> Placement {
    let (bounds, maximized) = match window.window_bounds() {
        WindowBounds::Windowed(bounds) => (bounds, false),
        WindowBounds::Maximized(bounds) | WindowBounds::Fullscreen(bounds) => (bounds, true),
    };
    Placement {
        x: bounds.origin.x.into(),
        y: bounds.origin.y.into(),
        width: bounds.size.width.into(),
        height: bounds.size.height.into(),
        maximized,
    }
}

/// Opens the window on the app.
pub fn open(
    backend: Arc<dyn Backend>,
    system: Rc<dyn System>,
    placement: Option<Placement>,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Shell>)> {
    let title = Look::now(cx).tr("window-title");
    let options = WindowOptions {
        window_bounds: Some(bounds(placement, cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            // The band is the title bar: the system draws none over it.
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(13.))),
        }),
        show: false,
        window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
        // Where a Linux desktop would draw a title bar of its own, the band
        // is the decoration.
        window_decorations: Some(WindowDecorations::Client),
        app_id: Some(APP_ID.to_string()),
        ..Default::default()
    };
    let (handle, shell) = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Shell::new(backend, system, window, cx))
    })?;
    // Its first frame is drawn by now: showing it is what activating does.
    handle.update(cx, |_, window, _| window.activate_window())?;
    Ok((handle, shell))
}
