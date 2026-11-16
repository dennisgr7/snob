//! When the app rests: its own process in the system's efficient mode
//! (`System::rest`) while nobody looks at its window and nothing on it has
//! moved for [`QUIET_FOR`], its being looked away from included. **It wakes
//! before anything is drawn**: a frame drawn is something moving, whether an
//! animation, the pointer over the window or an answer arriving.

use std::time::Duration;

/// How long the window has to be still, and not looked at, before the app
/// rests: long enough that a look at another window and back never rests it.
pub const QUIET_FOR: Duration = Duration::from_secs(10);

/// What is known of the window now.
#[derive(Debug, Clone, Copy)]
pub struct Seen {
    /// Whether its window is the one in front, not minimized.
    pub looked_at: bool,
    /// Whether somebody is waiting on something the app does meanwhile: the
    /// login window it opened.
    pub waited_on: bool,
    /// How long since anything moved: its last frame, or its being looked
    /// at or away from.
    pub still_for: Duration,
}

/// What follows from what is seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Tell the system the app rests.
    Rest,
    /// Tell the system the app is back.
    Wake,
    /// Look again after this long, when it may rest.
    LookIn(Duration),
    /// Nothing changes.
    Stay,
}

/// Whether the app rests now.
#[derive(Debug, Default)]
pub struct Rest {
    resting: bool,
}

impl Rest {
    pub fn is_resting(&self) -> bool {
        self.resting
    }

    /// What to do about what is `seen`; the state follows what it says.
    pub fn judge(&mut self, seen: Seen) -> Next {
        let quiet = !seen.looked_at && !seen.waited_on;
        let rests = quiet && seen.still_for >= QUIET_FOR;
        if rests != self.resting {
            self.resting = rests;
            return if rests { Next::Rest } else { Next::Wake };
        }
        if quiet && !rests {
            Next::LookIn(QUIET_FOR - seen.still_for)
        } else {
            Next::Stay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(looked_at: bool, waited_on: bool, still_for: u64) -> Seen {
        Seen {
            looked_at,
            waited_on,
            still_for: Duration::from_secs(still_for),
        }
    }

    #[test]
    fn a_window_left_still_and_unlooked_at_rests_after_the_quiet() {
        let mut rest = Rest::default();
        assert_eq!(
            rest.judge(seen(false, false, 3)),
            Next::LookIn(Duration::from_secs(7))
        );
        assert_eq!(rest.judge(seen(false, false, 10)), Next::Rest);
        assert!(rest.is_resting());
        assert_eq!(rest.judge(seen(false, false, 60)), Next::Stay);
    }

    #[test]
    fn a_frame_drawn_wakes_it_and_the_quiet_starts_again() {
        let mut rest = Rest::default();
        rest.judge(seen(false, false, 10));
        assert_eq!(rest.judge(seen(false, false, 0)), Next::Wake);
        assert_eq!(rest.judge(seen(false, false, 0)), Next::LookIn(QUIET_FOR));
    }

    #[test]
    fn being_looked_at_wakes_it_and_never_rests() {
        let mut rest = Rest::default();
        rest.judge(seen(false, false, 10));
        assert_eq!(rest.judge(seen(true, false, 60)), Next::Wake);
        assert_eq!(rest.judge(seen(true, false, 600)), Next::Stay);
    }

    #[test]
    fn a_login_window_somebody_waits_on_keeps_it_awake() {
        let mut rest = Rest::default();
        assert_eq!(rest.judge(seen(false, true, 600)), Next::Stay);
        assert!(!rest.is_resting());
    }
}
