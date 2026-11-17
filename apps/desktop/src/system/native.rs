//! The system the app runs on, asked through GPUI: the theme, sleep and
//! wake, notifications, the clipboard, the file manager.
//!
//! What GPUI does not say needs a call of the platform's own: more
//! contrast, less motion and transparency, the text size, whether there is
//! a network, the session ending. **Windows is asked (`windows.rs`); on
//! macOS and Linux these stay at their defaults**, and the app behaves as
//! on a system that set none.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
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
    says: Says,
    /// What was said before anybody listened, kept for the first who does.
    first: RefCell<Option<mpsc::UnboundedReceiver<Event>>>,
    _listening: Vec<Subscription>,
    #[cfg(windows)]
    windows: super::windows::Windows,
}

/// What says an event to whoever listens now, from any thread: the system's
/// own callbacks, and a second start of the app handing over. **One listener
/// at a time**: a window opened again takes over from the one that closed.
#[derive(Clone)]
pub struct Says(Arc<Mutex<mpsc::UnboundedSender<Event>>>);

impl Says {
    pub fn say(&self, event: Event) {
        // Nobody listening is a window closed, or the app on its way out.
        let _ = self.0.lock().unwrap_or_else(|e| e.into_inner()).send(event);
    }

    /// A listener of its own, in place of whoever listened before.
    fn listen(&self) -> mpsc::UnboundedReceiver<Event> {
        let (said, heard) = mpsc::unbounded_channel();
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = said;
        heard
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
        let says = Says(Arc::new(Mutex::new(said)));
        #[cfg(windows)]
        let windows = {
            let says = says.clone();
            super::windows::Windows::listen(Arc::clone(&settings), move |event| says.say(event))
        };

        let slept_at = Rc::new(RefCell::new(None::<SystemTime>));
        let sleeping = {
            let (says, slept_at) = (says.clone(), Rc::clone(&slept_at));
            cx.on_system_sleep(move |_| {
                *slept_at.borrow_mut() = Some(SystemTime::now());
                says.say(Event::Sleeping);
            })
        };
        let waking = {
            let says = says.clone();
            cx.on_system_wake(move |_| {
                // By the wall clock: the one that went on while the machine
                // did not.
                let slept = slept_at
                    .borrow_mut()
                    .take()
                    .and_then(|at| at.elapsed().ok())
                    .unwrap_or(Duration::ZERO);
                says.say(Event::Woke { slept });
            })
        };
        {
            let says = says.clone();
            cx.on_system_notification_response(move |response, _| {
                if let Some(about) = notice(&response.tag) {
                    says.say(Event::Opened(about));
                }
            });
        }

        Self {
            settings,
            says,
            first: RefCell::new(Some(heard)),
            _listening: vec![sleeping, waking],
            #[cfg(windows)]
            windows,
        }
    }

    /// What says an event from another thread.
    pub fn says(&self) -> Says {
        self.says.clone()
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
            self.says.say(Event::Settings(settings));
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
        snob_engine::browser::owner::attended(attended);
    }

    fn rest(&self, resting: bool) {
        use snob_engine::system::power::qos::{Mode, this_program};
        tracing::debug!(resting, "the app rests");
        this_program(if resting { Mode::Eco } else { Mode::Normal });
    }

    fn restart(&self, cx: &mut App) {
        cx.restart();
    }

    fn events(&self) -> mpsc::UnboundedReceiver<Event> {
        // Asked again by a window opened again: it takes over.
        self.first
            .borrow_mut()
            .take()
            .unwrap_or_else(|| self.says.listen())
    }

    fn copy_image(&self, bytes: Vec<u8>, cx: &mut App) {
        use gpui_kit::{Image, ImageFormat};
        let format = match snob_engine::media::picture_type(&bytes) {
            Some("image/png") => ImageFormat::Png,
            Some("image/webp") => ImageFormat::Webp,
            _ => ImageFormat::Jpeg,
        };
        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(format, bytes)));
    }

    fn downloads(&self) -> Option<PathBuf> {
        directories::UserDirs::new()?
            .download_dir()
            .map(Path::to_path_buf)
    }

    fn choose_folder(&self, cx: &mut App) -> tokio::sync::oneshot::Receiver<Option<PathBuf>> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        let asked = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.background_executor()
            .spawn(async move {
                let chosen = match asked.await {
                    Ok(Ok(Some(mut paths))) => paths.pop(),
                    _ => None,
                };
                let _ = answer.send(chosen);
            })
            .detach();
        answered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_said_goes_to_whoever_listens_now() {
        let (said, mut first) = mpsc::unbounded_channel();
        let says = Says(Arc::new(Mutex::new(said)));
        says.say(Event::Sleeping);
        assert_eq!(first.try_recv().ok(), Some(Event::Sleeping));

        let mut second = says.listen();
        says.say(Event::Activate);
        assert_eq!(second.try_recv().ok(), Some(Event::Activate));
        assert!(first.try_recv().is_err(), "the one before hears no more");
    }

    #[test]
    fn a_notification_clicked_is_told_apart_by_its_tag() {
        for about in [Notice::SessionExpired, Notice::UpdateReady] {
            assert_eq!(notice(tag(about)), Some(about));
        }
        assert_eq!(notice("something else"), None);
    }
}
