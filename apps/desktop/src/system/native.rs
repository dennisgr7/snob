//! The system the app runs on, asked through GPUI: the theme, sleep and
//! wake, notifications, the clipboard, the file manager.
//!
//! What GPUI does not say needs a call of the platform's own: more
//! contrast, less motion and transparency, the text size, whether there is
//! a network, the session ending. **Windows is asked (`windows.rs`); on
//! macOS and Linux these stay at their defaults**, and the app behaves as
//! on a system that set none.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gpui_kit::{App, ClipboardItem, Subscription, SystemNotification, WindowAppearance};
use tokio::sync::mpsc;

use super::{Event, Notice, Notification, Settings, System};

/// What a notification is told apart by when it is clicked.
fn tag(about: Notice) -> &'static str {
    match about {
        Notice::SessionExpired => "session-expired",
        Notice::UpdateReady => "update-ready",
    }
}

fn notice(tag: &str) -> Option<Notice> {
    [Notice::SessionExpired, Notice::UpdateReady]
        .into_iter()
        .find(|about| self::tag(*about) == tag)
}

fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// The language the system is set to, as a BCP 47 tag.
fn locale() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en".to_string())
}

/// The real system.
pub struct Native {
    settings: Arc<Mutex<Settings>>,
    said: mpsc::UnboundedSender<Event>,
    heard: RefCell<Option<mpsc::UnboundedReceiver<Event>>>,
    _listening: Vec<Subscription>,
    #[cfg(windows)]
    windows: super::windows::Windows,
}

/// Something that says what the system said from another thread: a second
/// start of the app handing over.
#[derive(Clone)]
pub struct Says(mpsc::UnboundedSender<Event>);

impl Says {
    pub fn say(&self, event: Event) {
        // Nobody listening is the app on its way out.
        let _ = self.0.send(event);
    }
}

impl Native {
    pub fn new(cx: &mut App) -> Self {
        let settings = Arc::new(Mutex::new(Settings {
            dark: is_dark(cx.window_appearance()),
            locale: locale(),
            ..Settings::default()
        }));
        let (said, heard) = mpsc::unbounded_channel();
        #[cfg(windows)]
        let windows = super::windows::Windows::listen(Arc::clone(&settings), said.clone());

        let slept_at = Rc::new(RefCell::new(None::<SystemTime>));
        let sleeping = {
            let (said, slept_at) = (said.clone(), Rc::clone(&slept_at));
            cx.on_system_sleep(move |_| {
                *slept_at.borrow_mut() = Some(SystemTime::now());
                let _ = said.send(Event::Sleeping);
            })
        };
        let waking = {
            let said = said.clone();
            cx.on_system_wake(move |_| {
                // By the wall clock: the one that went on while the machine
                // did not.
                let slept = slept_at
                    .borrow_mut()
                    .take()
                    .and_then(|at| at.elapsed().ok())
                    .unwrap_or(Duration::ZERO);
                let _ = said.send(Event::Woke { slept });
            })
        };
        {
            let said = said.clone();
            cx.on_system_notification_response(move |response, _| {
                if let Some(about) = notice(&response.tag) {
                    let _ = said.send(Event::Opened(about));
                }
            });
        }

        Self {
            settings,
            said,
            heard: RefCell::new(Some(heard)),
            _listening: vec![sleeping, waking],
            #[cfg(windows)]
            windows,
        }
    }

    /// What says an event from another thread.
    pub fn says(&self) -> Says {
        Says(self.said.clone())
    }

    /// The window's appearance changed: the system's theme did.
    pub fn appearance_changed(&self, appearance: WindowAppearance) {
        let dark = is_dark(appearance);
        let changed = {
            let mut settings = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            let changed = settings.dark != dark;
            settings.dark = dark;
            changed.then(|| settings.clone())
        };
        if let Some(settings) = changed {
            let _ = self.said.send(Event::Settings(settings));
        }
    }
}

impl System for Native {
    fn settings(&self) -> Settings {
        self.settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn online(&self) -> Option<bool> {
        #[cfg(windows)]
        return self.windows.online();
        #[cfg(not(windows))]
        None
    }

    fn notify(&self, notification: Notification, cx: &mut App) {
        cx.show_system_notification(SystemNotification {
            tag: tag(notification.about).into(),
            title: notification.title.into(),
            body: notification.body.into(),
            actions: Vec::new(),
        });
    }

    fn open_folder(&self, folder: &Path, cx: &mut App) {
        cx.open_with_system(folder);
    }

    fn copy(&self, text: &str, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
    }

    fn pasted(&self, cx: &mut App) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    fn hold_shutdown(&self, why: &str) -> Box<dyn Send> {
        #[cfg(windows)]
        return self.windows.hold_shutdown(why);
        #[cfg(not(windows))]
        {
            let _ = why;
            Box::new(())
        }
    }

    fn attended(&self, attended: bool) {
        snob_engine::owner::attended(attended);
    }

    fn restart(&self, cx: &mut App) {
        cx.restart();
    }

    fn events(&self) -> mpsc::UnboundedReceiver<Event> {
        self.heard.borrow_mut().take().unwrap_or_else(|| {
            // Asked twice: the second listener hears nothing.
            mpsc::unbounded_channel().1
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_clicked_is_told_apart_by_its_tag() {
        for about in [Notice::SessionExpired, Notice::UpdateReady] {
            assert_eq!(notice(tag(about)), Some(about));
        }
        assert_eq!(notice("something else"), None);
    }
}
