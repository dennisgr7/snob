//! What time it is, in the two units this project counts in: **seconds are the
//! domain unit ([`Epoch`]) and milliseconds the rate-control unit
//! ([`EpochMs`])**, and nothing converts between them by hand.
//!
//! The one module of this crate that does I/O: it reads the system clock.

use std::time::Duration;

/// A moment, in seconds since the Unix epoch.
///
/// **No `Deref`, no `From<Epoch> for i64`, no `Add<i64>`, on purpose**: what
/// is added to a moment is a [`Duration`], and the only way to an [`EpochMs`]
/// and back is [`EpochMs::to_epoch`]. Serialized as the bare number. `Default`
/// is the epoch itself, for `#[serde(default)]`; nothing tests for it.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
pub struct Epoch(i64);

/// A moment, in milliseconds since the Unix epoch: the unit of the budget, the
/// pacer and the cooldown. Otherwise as [`Epoch`].
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
pub struct EpochMs(i64);

impl Epoch {
    pub const fn new(seconds: i64) -> Self {
        Self(seconds)
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl EpochMs {
    pub const fn new(milliseconds: i64) -> Self {
        Self(milliseconds)
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    /// The same moment in seconds, floored (`div_euclid`, so a moment before
    /// the epoch does not round towards zero). The one place that converts.
    pub const fn to_epoch(self) -> Epoch {
        Epoch(self.0.div_euclid(1_000))
    }

    /// [`EpochMs::to_epoch`] rounded up, for a moment that means "not before"
    /// (the end of a cooldown): rounded down, a script that waits until it
    /// would wake up to a second early and be refused.
    pub const fn to_epoch_not_before(self) -> Epoch {
        Epoch(self.0.div_euclid(1_000) + (self.0.rem_euclid(1_000) > 0) as i64)
    }
}

impl std::fmt::Display for Epoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::fmt::Display for EpochMs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

/// Saturating: an absurd length stops at the end of time and neither panics
/// nor wraps into the past.
impl std::ops::Add<Duration> for Epoch {
    type Output = Self;

    fn add(self, length: Duration) -> Self {
        Self(self.0.saturating_add(whole_seconds(length)))
    }
}

impl std::ops::Sub<Duration> for Epoch {
    type Output = Self;

    fn sub(self, length: Duration) -> Self {
        Self(self.0.saturating_sub(whole_seconds(length)))
    }
}

/// A moment minus a moment, **signed**, in seconds: a negative answer means a
/// clock that went backwards, which callers handle and a [`Duration`] could
/// not say.
impl std::ops::Sub for Epoch {
    type Output = i64;

    fn sub(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

impl std::ops::Add<Duration> for EpochMs {
    type Output = Self;

    fn add(self, length: Duration) -> Self {
        Self(self.0.saturating_add(whole_milliseconds(length)))
    }
}

impl std::ops::Sub<Duration> for EpochMs {
    type Output = Self;

    fn sub(self, length: Duration) -> Self {
        Self(self.0.saturating_sub(whole_milliseconds(length)))
    }
}

/// Signed milliseconds, as the seconds version is signed.
impl std::ops::Sub for EpochMs {
    type Output = i64;

    fn sub(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

fn whole_seconds(length: Duration) -> i64 {
    i64::try_from(length.as_secs()).unwrap_or(i64::MAX)
}

fn whole_milliseconds(length: Duration) -> i64 {
    i64::try_from(length.as_millis()).unwrap_or(i64::MAX)
}

/// Current moment in seconds, the domain unit.
pub fn now() -> Epoch {
    Epoch(chrono::Utc::now().timestamp())
}

/// Current moment in milliseconds, the rate-control unit.
pub fn now_ms() -> EpochMs {
    EpochMs(chrono::Utc::now().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shape a moment appears in on the wire and on disk is the bare
    /// number, and that is a contract with whoever consumes the tool: `at`,
    /// `taken_at`, `last_reported_at` and the session file's `created_at` all
    /// go through this.
    #[test]
    fn a_moment_is_still_written_as_a_plain_number() {
        let at = Epoch::new(1_786_310_990);

        assert_eq!(at.to_string(), "1786310990");
        assert_eq!(serde_json::to_string(&at).unwrap(), "1786310990");
        assert_eq!(
            serde_json::from_str::<Epoch>("1786310990").unwrap(),
            at,
            "and reads back the same way"
        );
    }

    /// A "not before" moment never names a second before it.
    #[test]
    fn a_moment_not_before_rounds_up_to_the_second() {
        assert_eq!(EpochMs::new(5_000).to_epoch_not_before(), Epoch::new(5));
        assert_eq!(EpochMs::new(5_001).to_epoch_not_before(), Epoch::new(6));
        assert_eq!(EpochMs::new(5_999).to_epoch_not_before(), Epoch::new(6));
        assert_eq!(EpochMs::new(-1).to_epoch_not_before(), Epoch::new(0));
        assert_eq!(EpochMs::new(5_999).to_epoch(), Epoch::new(5));
    }

    /// The same contract for the rate-control unit. Nothing outside the budget
    /// reads one today, and the day something does it must read a number.
    #[test]
    fn a_moment_in_milliseconds_is_still_written_as_a_plain_number() {
        let until = EpochMs::new(1_786_310_990_123);

        assert_eq!(until.to_string(), "1786310990123");
        assert_eq!(serde_json::to_string(&until).unwrap(), "1786310990123");
        assert_eq!(
            serde_json::from_str::<EpochMs>("1786310990123").unwrap(),
            until,
            "and reads back the same way"
        );
    }

    /// A cooldown ending mid-second ends in that second and not in the next
    /// one, and a moment before the epoch floors rather than rounding towards
    /// zero — `/` would answer -1 for the second below, which is a second and a
    /// half in the wrong direction.
    #[test]
    fn a_cooldown_end_floors_to_the_second_it_is_in() {
        assert_eq!(
            EpochMs::new(1_786_310_990_123).to_epoch(),
            Epoch::new(1_786_310_990)
        );
        assert_eq!(EpochMs::new(-1_500).to_epoch(), Epoch::new(-2));
    }

    /// The arithmetic a moment is allowed, and the direction each of it goes
    /// in. What is not here is what the type is for: there is no `Add<i64>` to
    /// test, and no way to subtract a second from a millisecond.
    #[test]
    fn a_moment_takes_a_length_of_time_and_gives_one_back() {
        let at = Epoch::new(1_000);

        assert_eq!(at + Duration::from_secs(60), Epoch::new(1_060));
        assert_eq!(at - Duration::from_secs(60), Epoch::new(940));
        assert_eq!(Epoch::new(1_060) - at, 60);
        assert_eq!(at - Epoch::new(1_060), -60, "and the difference is signed");

        assert_eq!(
            EpochMs::new(1_000) + Duration::from_secs(1),
            EpochMs::new(2_000)
        );
        assert_eq!(EpochMs::new(2_000) - EpochMs::new(1_000), 1_000);
    }

    /// Saturating rather than panicking, in every direction. A hand-edited
    /// `watch.toml` can carry an interval of billions of years.
    #[test]
    fn an_absurd_length_of_time_stops_at_the_end_of_time() {
        assert_eq!(Epoch::new(1_000) + Duration::MAX, Epoch::new(i64::MAX));
        assert_eq!(
            Epoch::new(-1_000) - Duration::MAX,
            Epoch::new(i64::MIN),
            "a length of time longer than a moment can hold is i64::MAX of them"
        );
        assert_eq!(Epoch::new(i64::MIN) - Epoch::new(i64::MAX), i64::MIN);
    }
}
