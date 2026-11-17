//! The one line of status in the top band: several things can be true at
//! once, and the band says the one that matters most. **What a screen reader
//! is told is decided here too**, so a download does not announce every
//! percent.

use snob_core::Epoch;

/// Something the band can say. In the order it wins: the first that is on is
/// the one shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Offline,
    /// Instagram asked for a pause, until then.
    Paused {
        until: Epoch,
    },
    /// Back from a sleep or a lost network, picking up where it was.
    Resuming,
    /// A read waits on the engine's browser, which is starting: said while
    /// nothing was answered for as long as the browser stays open idle.
    Starting,
    /// The profile shown, or the tab shown of it, is being read: what was
    /// kept of it is shown meanwhile.
    Loading,
    /// An update is being fetched: a whole number from 0 to 100, when the
    /// size is known.
    Downloading {
        percent: Option<u8>,
    },
    /// A newer version is out and the app's package manager installs it.
    UpdateAvailable,
}

impl Status {
    /// Where this stands among the others: lower wins.
    fn rank(&self) -> u8 {
        match self {
            Self::Offline => 0,
            Self::Paused { .. } => 1,
            Self::Resuming => 2,
            Self::Starting => 3,
            Self::Loading => 4,
            Self::Downloading { .. } => 5,
            Self::UpdateAvailable => 6,
        }
    }

    /// Whether `self` and `other` are the same thing to say, whatever their
    /// numbers.
    fn same_kind(&self, other: &Self) -> bool {
        self.rank() == other.rank()
    }
}

/// Everything that is true now, and the one the band shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Band {
    on: Vec<Status>,
}

impl Band {
    /// Turns `status` on, or updates its numbers when it already is.
    pub fn set(&mut self, status: Status) {
        self.clear_kind(&status);
        self.on.push(status);
        self.on.sort_by_key(Status::rank);
    }

    /// Whether something of `status`'s kind is on, shown or not.
    pub fn holds(&self, status: &Status) -> bool {
        self.on.iter().any(|on| on.same_kind(status))
    }

    /// Turns off whatever is on of `status`'s kind.
    pub fn clear_kind(&mut self, status: &Status) {
        self.on.retain(|on| !on.same_kind(status));
    }

    pub fn clear_offline(&mut self) {
        self.clear_kind(&Status::Offline);
    }

    pub fn clear_pause(&mut self) {
        self.clear_kind(&Status::Paused {
            until: Epoch::new(0),
        });
    }

    pub fn clear_download(&mut self) {
        self.clear_kind(&Status::Downloading { percent: None });
    }

    /// Drops a pause that is over at `now`; whether one was dropped.
    pub fn lift_pause_at(&mut self, now: Epoch) -> bool {
        let before = self.on.len();
        self.on
            .retain(|on| !matches!(on, Status::Paused { until } if until.get() <= now.get()));
        self.on.len() != before
    }

    /// What the band shows: the most important of what is on.
    pub fn shown(&self) -> Option<&Status> {
        self.on.first()
    }
}

/// How far apart two announcements of one download are, in percent.
pub const ANNOUNCE_EVERY: u8 = 25;

/// Whether a screen reader is told about the change from `before` to `now`:
/// every change of what is said, and of a download only its start, each
/// quarter and its end.
pub fn announced(before: Option<&Status>, now: Option<&Status>) -> bool {
    match (before, now) {
        (
            Some(Status::Downloading { percent: before }),
            Some(Status::Downloading { percent: now }),
        ) => match (before, now) {
            (Some(before), Some(now)) => {
                now / ANNOUNCE_EVERY > before / ANNOUNCE_EVERY || (*now == 100 && *before < 100)
            }
            (None, Some(_)) => true,
            _ => false,
        },
        (before, now) => before != now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paused(until: i64) -> Status {
        Status::Paused {
            until: Epoch::new(until),
        }
    }

    #[test]
    fn the_band_says_what_matters_most_of_what_is_true() {
        let mut band = Band::default();
        assert_eq!(band.shown(), None);

        band.set(Status::UpdateAvailable);
        band.set(Status::Downloading { percent: Some(10) });
        assert_eq!(
            band.shown(),
            Some(&Status::Downloading { percent: Some(10) })
        );

        band.set(paused(100));
        assert_eq!(band.shown(), Some(&paused(100)));

        band.set(Status::Offline);
        assert_eq!(band.shown(), Some(&Status::Offline));

        // What was under it is still true once it clears.
        band.clear_offline();
        assert_eq!(band.shown(), Some(&paused(100)));
        band.clear_pause();
        assert_eq!(
            band.shown(),
            Some(&Status::Downloading { percent: Some(10) })
        );
        band.clear_download();
        assert_eq!(band.shown(), Some(&Status::UpdateAvailable));
    }

    #[test]
    fn the_engine_starting_says_more_than_a_profile_loading() {
        let mut band = Band::default();
        band.set(Status::Downloading { percent: None });
        band.set(Status::Loading);
        assert_eq!(band.shown(), Some(&Status::Loading));
        band.set(Status::Starting);
        assert_eq!(band.shown(), Some(&Status::Starting));
        band.clear_kind(&Status::Starting);
        assert_eq!(band.shown(), Some(&Status::Loading));
    }

    #[test]
    fn a_status_set_again_replaces_its_numbers() {
        let mut band = Band::default();
        band.set(Status::Downloading { percent: Some(10) });
        band.set(Status::Downloading { percent: Some(63) });
        assert_eq!(
            band.shown(),
            Some(&Status::Downloading { percent: Some(63) })
        );
        band.clear_download();
        assert_eq!(band.shown(), None);
    }

    #[test]
    fn a_pause_is_lifted_when_it_is_over_and_not_before() {
        let mut band = Band::default();
        band.set(paused(100));
        assert!(!band.lift_pause_at(Epoch::new(99)));
        assert_eq!(band.shown(), Some(&paused(100)));
        assert!(band.lift_pause_at(Epoch::new(100)));
        assert_eq!(band.shown(), None);
    }

    #[test]
    fn a_download_is_announced_at_its_start_each_quarter_and_its_end() {
        let at = |percent: u8| Status::Downloading {
            percent: Some(percent),
        };
        let mut said = Vec::new();
        let mut before: Option<Status> = None;
        for percent in 0..=100u8 {
            let now = Some(at(percent));
            if announced(before.as_ref(), now.as_ref()) {
                said.push(percent);
            }
            before = now;
        }
        assert_eq!(said, [0, 25, 50, 75, 100]);
    }

    #[test]
    fn every_other_change_of_status_is_announced() {
        assert!(announced(None, Some(&Status::Offline)));
        assert!(announced(Some(&Status::Offline), None));
        assert!(announced(Some(&Status::Offline), Some(&paused(100))));
        assert!(announced(Some(&paused(100)), Some(&paused(200))));
        assert!(!announced(Some(&Status::Offline), Some(&Status::Offline)));
        assert!(!announced(None, None));
        assert!(!announced(
            Some(&Status::Downloading { percent: None }),
            Some(&Status::Downloading { percent: None })
        ));
    }
}
