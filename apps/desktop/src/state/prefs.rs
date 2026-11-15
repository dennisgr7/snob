//! What the app remembers between runs that is the app's alone: where its
//! window was, and the method chosen before a restart for an update.
//! `desktop.json` in the data directory. **Nothing about an account is kept
//! here**: sessions, lists and budgets are the engine's, in its own files.

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
}

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
        };
        prefs.save(&paths);
        assert_eq!(Prefs::load(&paths), prefs);

        std::fs::write(file(&paths), b"{ not json").unwrap();
        assert_eq!(Prefs::load(&paths), Prefs::default());
    }

    #[test]
    fn a_method_is_gone_back_to_only_shortly_after_it_was_chosen() {
        let prefs = Prefs {
            window: None,
            resume: Some(Resume {
                method: Method::Browser,
                at: Epoch::new(1_000),
            }),
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
