//! What the app asks of the operating system and what the system tells it,
//! behind one trait: how the person set the machine up (theme, contrast,
//! motion, transparency, text size), sleep and wake, a shutdown on its way,
//! the network, notifications, the clipboard, the folders files go to, and
//! an account's address on Instagram opened in the person's browser.
//!
//! **A view never calls Windows, macOS or a Linux desktop itself.** It reads
//! [`Settings`] and takes [`Event`]s, which the test build's [`simulated`]
//! system produces on demand, so a sleep of two hours or a shutdown is a line
//! in a scenario and not something done to the machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::App;
use tokio::sync::mpsc;

pub mod native;
#[cfg(feature = "testing")]
pub mod simulated;
#[cfg(windows)]
mod windows;

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

/// The operating system, as the app uses it. **Used from the window's
/// thread alone**, which is where the real one has to be asked.
pub trait System {
    /// How the system is set up now.
    fn settings(&self) -> Settings;

    /// Whether the system says there is a network: `None` when it does not
    /// say, which is taken as there being one.
    fn online(&self) -> Option<bool>;

    /// Shows a notification.
    fn notify(&self, notification: Notification, cx: &mut App);

    /// Opens a folder in the system's file manager.
    fn open_folder(&self, folder: &Path, cx: &mut App);

    /// Puts `text` on the clipboard.
    fn copy(&self, text: &str, cx: &mut App);

    /// The text on the clipboard, when there is some.
    fn pasted(&self, cx: &mut App) -> Option<String>;

    /// Asks the system to wait before a shutdown, saying `why`, until the
    /// guard is dropped. The system grants seconds at most, and the person
    /// can always force it: nothing may depend on being granted.
    fn hold_shutdown(&self, why: &str) -> Box<dyn Send>;

    /// Tells the system whether somebody is looking at what the app does:
    /// work nobody waits for runs in the system's efficient mode.
    fn attended(&self, attended: bool);

    /// Runs the app's own process at the efficient end of what the system
    /// offers while `resting`, and back (`state::rest`): never the browsers
    /// or the command line it starts.
    fn rest(&self, resting: bool);

    /// Starts the app again in place of this one: after an update.
    fn restart(&self, cx: &mut App);

    /// Everything the system says from now on, to one listener: asked again,
    /// by a window opened again, the new one takes over.
    fn events(&self) -> mpsc::UnboundedReceiver<Event>;

    /// Puts a picture on the clipboard: `bytes` of a PNG, a JPEG or a WebP.
    fn copy_image(&self, bytes: Vec<u8>, cx: &mut App);

    /// The person's folder for downloads, as the system names it.
    fn downloads(&self) -> Option<PathBuf>;

    /// Asks the person, in the system's own dialog, for a folder to put a
    /// file in: none when they cancel.
    fn choose_folder(&self, cx: &mut App) -> tokio::sync::oneshot::Receiver<Option<PathBuf>>;

    /// Opens `url` in the person's default browser, as any link would:
    /// **only an account's address on Instagram** ([`profile_address`]),
    /// anything else refused ([`opens`]). Their browser is handed the
    /// address and nothing else: snob neither drives it nor reads it.
    fn open_url(&self, url: &str, cx: &mut App);
}

/// Where Instagram shows `username`'s profile: none for what cannot be a
/// username (`snob_core::model::possible_username`), so the address is
/// never another page's.
pub fn profile_address(username: &str) -> Option<String> {
    let name = username.strip_prefix('@').unwrap_or(username);
    snob_core::model::possible_username(name).then(|| format!("{PROFILES}{name}/"))
}

/// What every address the app opens begins with.
const PROFILES: &str = "https://www.instagram.com/";

/// Whether `url` is one the app may open: an account's address on
/// Instagram, as [`profile_address`] writes it, and nothing more.
pub fn opens(url: &str) -> bool {
    url.strip_prefix(PROFILES)
        .and_then(|rest| rest.strip_suffix('/'))
        .is_some_and(|name| !name.starts_with('@') && profile_address(name).as_deref() == Some(url))
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

    #[test]
    fn an_address_is_made_of_a_name_that_can_be_a_username_alone() {
        assert_eq!(
            profile_address("someone").as_deref(),
            Some("https://www.instagram.com/someone/")
        );
        assert_eq!(
            profile_address("@user.name_9").as_deref(),
            Some("https://www.instagram.com/user.name_9/")
        );
        for wrong in [
            "",
            "@",
            ".",
            "..",
            "a/b",
            "a b",
            "a?b=c",
            "\u{e9}mile",
            &"a".repeat(31),
        ] {
            assert_eq!(profile_address(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn only_an_account_s_address_on_instagram_is_opened() {
        assert!(opens("https://www.instagram.com/someone/"));
        for refused in [
            "https://www.instagram.com/",
            "https://www.instagram.com/someone",
            "https://www.instagram.com/@someone/",
            "https://www.instagram.com/someone/?next=x",
            "https://www.instagram.com/someone/p/",
            "https://www.instagram.com/../",
            "https://www.instagram.com/a%2Fb/",
            "http://www.instagram.com/someone/",
            "https://instagram.com/someone/",
            "https://www.instagram.com.example.com/someone/",
            "https://example.com/someone/",
            "file:///C:/Windows/",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!opens(refused), "{refused}");
        }
    }
}
