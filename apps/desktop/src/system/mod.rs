//! What the app asks of the operating system and what the system tells it,
//! behind one trait: how the person set the machine up (theme, contrast,
//! motion, transparency, text size), sleep and wake, a shutdown on its way,
//! the network, notifications.
//!
//! **A view never calls Windows, macOS or a Linux desktop itself.** It reads
//! [`Settings`] and takes [`Event`]s, which the test build's [`simulated`]
//! system produces on demand, so a sleep of two hours or a shutdown is a line
//! in a scenario and not something done to the machine.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

#[cfg(feature = "testing")]
pub mod simulated;

/// How the person set the system up, as far as the app follows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub dark: bool,
    /// A contrast theme on Windows, "Increase contrast" on macOS, the
    /// portal's `contrast` on Linux.
    pub more_contrast: bool,
    pub reduce_motion: bool,
    /// The system asks for no transparency, or is saving battery: the band
    /// stays opaque.
    pub reduce_transparency: bool,
    /// The system's text size, where 1 is its default. Followed up to 2.
    pub text_scale: f32,
    /// The language the interface is in: a BCP 47 tag.
    pub locale: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: false,
            more_contrast: false,
            reduce_motion: false,
            reduce_transparency: false,
            text_scale: 1.0,
            locale: "en".to_string(),
        }
    }
}

impl Settings {
    /// The text scale the app draws at: the system's, held between its
    /// default and twice it.
    pub fn text_scale(&self) -> f32 {
        if self.text_scale.is_finite() {
            self.text_scale.clamp(1.0, 2.0)
        } else {
            1.0
        }
    }
}

/// Something the system said.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A setting changed while the app is open: followed at once.
    Settings(Settings),
    /// The machine is about to sleep.
    Sleeping,
    /// It is back, after this long.
    Woke { slept: Duration },
    /// The session is ending: a shutdown, a restart, a sign-out.
    ShuttingDown,
    /// The network came or went. **Only what the system says**: the app
    /// never sends a request to find out.
    Online(bool),
    /// Another start of the app handed over to this one.
    Activate,
    /// A notification was clicked.
    Opened(Notice),
}

/// What a notification is about, and so which screen a click on it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Notice {
    /// The session of the account in use stopped working.
    SessionExpired,
    /// A newer version is fetched and waits for a restart.
    UpdateReady,
}

/// A notification: only for what the person is not looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub about: Notice,
    pub title: String,
    pub body: String,
}

/// The operating system, as the app uses it.
pub trait System: Send + Sync {
    /// How the system is set up now.
    fn settings(&self) -> Settings;

    /// Whether the system says there is a network: `None` when it does not
    /// say, which is taken as there being one.
    fn online(&self) -> Option<bool>;

    /// Shows a notification.
    fn notify(&self, notification: Notification);

    /// Opens a folder in the system's file manager.
    fn open_folder(&self, folder: &Path);

    /// Puts `text` on the clipboard.
    fn copy(&self, text: &str);

    /// The text on the clipboard, when there is some.
    fn pasted(&self) -> Option<String>;

    /// Asks the system to wait before a shutdown, saying `why`, until the
    /// guard is dropped. The system grants seconds at most, and the person
    /// can always force it: nothing may depend on being granted.
    fn hold_shutdown(&self, why: &str) -> Box<dyn Send>;

    /// Tells the system whether somebody is looking at what the app does:
    /// work nobody waits for runs in the system's efficient mode.
    fn attended(&self, attended: bool);

    /// Everything the system says from now on. Called once.
    fn events(&self) -> mpsc::Receiver<Event>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_scale_is_followed_up_to_twice_the_default() {
        let at = |text_scale: f32| {
            Settings {
                text_scale,
                ..Settings::default()
            }
            .text_scale()
        };
        assert_eq!(at(1.0), 1.0);
        assert_eq!(at(1.5), 1.5);
        assert_eq!(at(3.0), 2.0);
        assert_eq!(at(0.5), 1.0);
        assert_eq!(at(f32::NAN), 1.0);
    }
}
