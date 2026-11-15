//! What the tests of the window share: the app opened on a scripted backend
//! and a simulated system, and how a test looks at it and acts on it.
#![allow(dead_code)]

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, Role, TestAppContext, Window};
use snob_desktop::backend::scripted::Scripted;
use snob_desktop::state::flow::Screen;
use snob_desktop::system::Settings;
use snob_desktop::system::simulated::Simulated;
use snob_desktop::ui;
use snob_desktop::ui::shell::Shell;

/// How long a test waits for the window to get somewhere, on the test's own
/// clock: longer than every wait the app makes by itself.
pub const SOON: Duration = Duration::from_secs(20);

pub struct App {
    pub window: AnyWindowHandle,
    pub shell: Entity<Shell>,
    pub backend: Scripted,
    pub system: Simulated,
}

pub fn open_with(cx: &mut TestAppContext, backend: Scripted, settings: Settings) -> App {
    let system = Simulated::new(settings);
    let (window, shell) = cx.update(|cx| {
        ui::window::init(&system, true, cx);
        ui::window::open(Arc::new(backend.clone()), Rc::new(system.clone()), None, cx)
            .expect("the window opens")
    });
    App {
        window,
        shell,
        backend,
        system,
    }
}

pub fn open(cx: &mut TestAppContext, backend: Scripted) -> App {
    open_with(cx, backend, Settings::default())
}

impl App {
    /// Waits until the screen shown is one `wanted` takes.
    pub async fn until_screen(&self, cx: &mut TestAppContext, wanted: impl Fn(&Screen) -> bool) {
        let shell = self.shell.clone();
        cx.wait_for(self.window, SOON, move |_, cx| {
            wanted(shell.read(cx).shown())
        })
        .await;
    }

    /// Waits until `wanted` holds of the window.
    pub async fn until(
        &self,
        cx: &mut TestAppContext,
        wanted: impl FnMut(&mut Window, &mut gpui_kit::App) -> bool,
    ) {
        cx.wait_for(self.window, SOON, wanted).await;
    }

    pub fn in_window<R>(
        &self,
        cx: &mut TestAppContext,
        act: impl FnOnce(&mut Window, &mut gpui_kit::App) -> R,
    ) -> R {
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            act(window, cx)
        })
        .expect("the window is open")
    }

    pub fn click(&self, cx: &mut TestAppContext, id: &'static str) {
        self.in_window(cx, |window, cx| window.click(id, cx));
    }

    pub fn press(&self, cx: &mut TestAppContext, key: &'static str) {
        self.in_window(cx, |window, cx| window.press(key, cx));
    }

    /// The title of the screen shown, as a screen reader is told it, and
    /// whether focus is on it.
    pub fn title(&self, cx: &mut TestAppContext) -> (String, bool) {
        self.in_window(cx, |window, _| {
            let title = window.find("title");
            assert_eq!(title.role(), Some(Role::Heading));
            (
                title.label().unwrap_or_default().to_string(),
                title.focused() == Some(true),
            )
        })
    }

    pub fn label_of(&self, cx: &mut TestAppContext, id: &'static str) -> String {
        self.in_window(cx, |window, _| {
            window.find(id).label().unwrap_or_default().to_string()
        })
    }
}

/// Whether the control `id` is disabled.
pub fn disabled(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| window.find(id).disabled() == Some(true))
}

/// Whether focus is on `id`.
pub fn focused(app: &App, cx: &mut TestAppContext, id: &'static str) -> bool {
    app.in_window(cx, |window, _| window.find(id).focused() == Some(true))
}
