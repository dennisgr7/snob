//! What the app remembers between runs that is the app's alone: where its
//! window was, the method chosen before a restart for an update, the theme
//! and language chosen in its settings, and which accounts were shown
//! the automations' introduction (`state::automations::Introduced`).
//! `desktop.json` in the data directory. **Nothing of an account's own is
//! kept here**, only what the app showed it: sessions, lists and budgets are
//! the engine's, in its own files.

use std::io::Read as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use snob_core::Epoch;
use snob_store::paths::{self, AppPaths};

use super::flow::Method;

/// How long a method chosen before a restart is still gone back to: past
/// it, the restart was not the update's, and the app starts as usual.
pub const RESUME_WITHIN_SECS: i64 = 10 * 60;

/// The most of the file that is read.
const MAX_BYTES: u64 = 64 * 1024;

/// Where the window was, in logical pixels of its display.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

/// The smallest window the app is used in.
pub const MIN_WIDTH: f32 = 800.0;
pub const MIN_HEIGHT: f32 = 600.0;

/// The window the app opens with the first time.
pub const FIRST_WIDTH: f32 = 1100.0;
pub const FIRST_HEIGHT: f32 = 760.0;

impl Placement {
    /// This placement if it can be used on a display of `screens`, each
    /// `(x, y, width, height)`: no smaller than the least the app is used
    /// in, and with enough of its top band on one display to be dragged
    /// back. A display unplugged since must not leave the window out of
    /// reach.
    pub fn usable_on(self, screens: &[(f32, f32, f32, f32)]) -> Option<Self> {
        let sane = [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite());
        if !sane || self.width < MIN_WIDTH || self.height < MIN_HEIGHT {
            return None;
        }
        // The band's middle third, 20 px down.
        let (grab_x, grab_y) = (self.x + self.width / 2.0, self.y + 20.0);
        let reachable = screens.iter().any(|(x, y, width, height)| {
            (grab_x - self.width / 6.0) >= *x
                && (grab_x + self.width / 6.0) <= x + width
                && grab_y >= *y
                && grab_y <= y + height
        });
        reachable.then_some(self)
    }
}

/// The theme chosen: the system's, or one of the two whatever the system
/// says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
}

/// How the person chose the app to look, over what the system says. **The
/// contrast is always the system's**: it is asked for by somebody who needs
/// it, and a choice made here must not take it away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Appearance {
    pub theme: ThemeChoice,
    /// A language tag, or the system's language with none. One the app does
    /// not speak reads as none.
    pub language: Option<String>,
}

/// A method chosen, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    pub method: Method,
    pub at: Epoch,
}

/// The file's contents. Every field may be absent: an older or a damaged
/// file reads as nothing remembered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub window: Option<Placement>,
    #[serde(default)]
    pub resume: Option<Resume>,
    /// The account whose session was found dead: asked to sign in again at
    /// the next start, without asking Instagram a second time.
    #[serde(default)]
    pub expired: Option<snob_core::Pk>,
    /// When each account's session was last found to work, by asking
    /// Instagram or by signing in.
    #[serde(default)]
    pub checked: Vec<Checked>,
    /// The browser the last pasted session was said to come from: the one
    /// offered first the next time.
    #[serde(default)]
    pub browser: Option<String>,
    #[serde(default)]
    pub theme: ThemeChoice,
    /// The language chosen, as a tag: the system's when there is none.
    #[serde(default)]
    pub language: Option<String>,
    /// The accounts that went past the automations' introduction, which is
    /// shown once an account.
    #[serde(default)]
    pub introduced: Vec<snob_core::Pk>,
}

/// An account's session, found to work at a moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub pk: snob_core::Pk,
    pub at: Epoch,
}

/// How long a session found to work is taken to: Instagram is asked about
/// it once a day at most.
pub const CHECKED_FOR_SECS: i64 = 24 * 60 * 60;

fn file(paths: &AppPaths) -> PathBuf {
    paths.data_dir().join("desktop.json")
}

impl Prefs {
    /// What was remembered: nothing when there is no file or it does not
    /// read.
    pub fn load(paths: &AppPaths) -> Self {
        let mut text = String::new();
        let read = std::fs::File::open(file(paths))
            .and_then(|file| file.take(MAX_BYTES).read_to_string(&mut text));
        match read {
            Ok(_) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Remembers this. A failure is logged and otherwise ignored: nothing
    /// here is worth stopping for.
    pub fn save(&self, paths: &AppPaths) {
        let saved = serde_json::to_vec_pretty(self)
            .map_err(std::io::Error::other)
            .and_then(|contents| {
                let file = file(paths);
                let temporary = file.with_file_name(".desktop.json.tmp");
                paths::replace_private_alone(&file, &temporary, &contents).map_err(|(_, e)| e)
            });
        if let Err(e) = saved {
            tracing::warn!(error = %e, "what the app remembers could not be saved");
        }
    }

    /// Reads what is remembered, applies `change` and saves the result,
    /// **one change at a time**: the app's changes run on the runtime's two
    /// threads, and two read at once would each save without the other's.
    pub fn change(paths: &AppPaths, change: impl FnOnce(&mut Self)) {
        static CHANGING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _alone = CHANGING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut prefs = Self::load(paths);
        let before = prefs.clone();
        change(&mut prefs);
        if prefs != before {
            prefs.save(paths);
        }
    }

    /// Whether the session of `pk` was found to work less than a day before
    /// `now`. A moment ahead of `now` is a clock that was moved: not taken.
    pub fn checked_lately(&self, pk: snob_core::Pk, now: Epoch) -> bool {
        self.checked.iter().any(|checked| {
            checked.pk == pk && (0..CHECKED_FOR_SECS).contains(&(now.get() - checked.at.get()))
        })
    }

    /// Notes that the session of `pk` worked at `now`.
    pub fn note_checked(&mut self, pk: snob_core::Pk, now: Epoch) {
        self.checked.retain(|checked| checked.pk != pk);
        self.checked.push(Checked { pk, at: now });
    }

    /// The theme and language chosen.
    pub fn appearance(&self) -> Appearance {
        Appearance {
            theme: self.theme,
            language: self.language.clone(),
        }
    }

    /// The method to go straight back to at `now`, when one was chosen
    /// shortly before.
    pub fn resumed(&self, now: Epoch) -> Option<Method> {
        self.resume
            .filter(|resume| (0..=RESUME_WITHIN_SECS).contains(&(now.get() - resume.at.get())))
            .map(|resume| resume.method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE_SCREEN: [(f32, f32, f32, f32); 1] = [(0.0, 0.0, 1920.0, 1080.0)];

    fn placed(x: f32, y: f32, width: f32, height: f32) -> Placement {
        Placement {
            x,
            y,
            width,
            height,
            maximized: false,
        }
    }

    #[test]
    fn a_session_found_to_work_is_taken_to_for_a_day_and_no_longer() {
        let (me, other) = (snob_core::Pk::new(42), snob_core::Pk::new(9001));
        let mut prefs = Prefs::default();
        let at = |secs: i64| Epoch::new(1_800_000_000 + secs);
        assert!(!prefs.checked_lately(me, at(0)));

        prefs.note_checked(me, at(0));
        assert!(prefs.checked_lately(me, at(0)));
        assert!(prefs.checked_lately(me, at(CHECKED_FOR_SECS - 1)));
        assert!(!prefs.checked_lately(me, at(CHECKED_FOR_SECS)));
        assert!(!prefs.checked_lately(other, at(0)), "each account its own");
        assert!(!prefs.checked_lately(me, at(-60)), "a clock moved back");

        prefs.note_checked(me, at(CHECKED_FOR_SECS));
        assert_eq!(prefs.checked.len(), 1, "one moment an account");
    }

    #[test]
    fn what_is_remembered_is_read_back_and_a_damaged_file_is_nothing() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        paths.ensure_dirs().unwrap();
        assert_eq!(Prefs::load(&paths), Prefs::default());

        let prefs = Prefs {
            window: Some(placed(100.0, 80.0, 1100.0, 760.0)),
            resume: Some(Resume {
                method: Method::Paste,
                at: Epoch::new(1_790_000_000),
            }),
            expired: Some(42.into()),
            checked: vec![Checked {
                pk: 42.into(),
                at: Epoch::new(1_790_000_000),
            }],
            browser: Some("Edge".to_string()),
            theme: ThemeChoice::Dark,
            language: Some("es".to_string()),
            introduced: vec![42.into()],
        };
        prefs.save(&paths);
        assert_eq!(Prefs::load(&paths), prefs);

        Prefs::change(&paths, |prefs| prefs.expired = None);
        assert_eq!(Prefs::load(&paths).expired, None);
        assert_eq!(Prefs::load(&paths).window, prefs.window);

        std::fs::write(file(&paths), b"{ not json").unwrap();
        assert_eq!(Prefs::load(&paths), Prefs::default());
    }

    #[test]
    fn a_file_from_before_the_theme_was_chosen_reads_as_the_system_s() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        paths.ensure_dirs().unwrap();
        std::fs::write(
            file(&paths),
            br#"{"window":{"x":100,"y":80,"width":1100,"height":760},"browser":"Edge"}"#,
        )
        .unwrap();
        let prefs = Prefs::load(&paths);
        assert_eq!(prefs.browser.as_deref(), Some("Edge"), "the rest is kept");
        assert_eq!(prefs.appearance(), Appearance::default());
        assert!(prefs.introduced.is_empty(), "nobody was introduced yet");

        Prefs::change(&paths, |prefs| prefs.theme = ThemeChoice::Light);
        let text = std::fs::read_to_string(file(&paths)).unwrap();
        assert!(text.contains(r#""theme": "light""#), "{text}");
        assert_eq!(Prefs::load(&paths).theme, ThemeChoice::Light);
    }

    #[test]
    fn a_method_is_gone_back_to_only_shortly_after_it_was_chosen() {
        let prefs = Prefs {
            resume: Some(Resume {
                method: Method::Browser,
                at: Epoch::new(1_000),
            }),
            ..Prefs::default()
        };
        assert_eq!(prefs.resumed(Epoch::new(1_030)), Some(Method::Browser));
        assert_eq!(
            prefs.resumed(Epoch::new(1_000 + RESUME_WITHIN_SECS)),
            Some(Method::Browser)
        );
        assert_eq!(prefs.resumed(Epoch::new(1_001 + RESUME_WITHIN_SECS)), None);
        // A clock set back is no reason to skip the start screen.
        assert_eq!(prefs.resumed(Epoch::new(900)), None);
        assert_eq!(Prefs::default().resumed(Epoch::new(1_030)), None);
    }

    #[test]
    fn a_window_is_put_back_only_where_it_can_be_reached() {
        let good = placed(100.0, 80.0, 1100.0, 760.0);
        assert_eq!(good.usable_on(&ONE_SCREEN), Some(good));

        // On a second display to the left, and after it was unplugged.
        let left = placed(-1700.0, 100.0, 1100.0, 760.0);
        let two = [(-1920.0, 0.0, 1920.0, 1080.0), ONE_SCREEN[0]];
        assert_eq!(left.usable_on(&two), Some(left));
        assert_eq!(left.usable_on(&ONE_SCREEN), None);

        // Its band above the top of the display, too small, or not numbers.
        assert_eq!(
            placed(100.0, -300.0, 1100.0, 760.0).usable_on(&ONE_SCREEN),
            None
        );
        assert_eq!(
            placed(100.0, 80.0, 400.0, 300.0).usable_on(&ONE_SCREEN),
            None
        );
        assert_eq!(
            placed(f32::NAN, 80.0, 1100.0, 760.0).usable_on(&ONE_SCREEN),
            None
        );
        assert_eq!(good.usable_on(&[]), None);
    }
}
