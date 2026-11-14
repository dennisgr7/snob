//! Durations as a person writes them: `30m`, `6h`, `2d`, `2w`.
//!
//! One parser for every place that reads one (a capture's maximum age, the
//! monitor's interval and jitter, `watch.toml`, which writes them back).

use std::time::Duration;

/// Parses a duration written with a unit suffix. No suffix means seconds.
///
/// Weeks are the largest unit: a month has no fixed length, and belongs to the
/// calendar side of a schedule.
pub fn parse(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let (number, factor) = match text.chars().last() {
        Some('s') => (&text[..text.len() - 1], 1),
        Some('m') => (&text[..text.len() - 1], 60),
        Some('h') => (&text[..text.len() - 1], 3600),
        Some('d') => (&text[..text.len() - 1], 86_400),
        Some('w') => (&text[..text.len() - 1], 604_800),
        Some(c) if c.is_ascii_digit() => (text, 1),
        _ => return Err(unreadable(text)),
    };

    let value: u64 = number.trim().parse().map_err(|_| unreadable(text))?;

    // **Bounded by `i64`, not `u64`**: every consumer compares the seconds
    // against a timestamp, and a value that fits a `u64` but casts negative
    // would turn "never expire" into "walk the list again, every time".
    let seconds = value
        .checked_mul(factor)
        .filter(|s| i64::try_from(*s).is_ok())
        .ok_or_else(|| format!("\"{text}\" is too long to be a duration"))?;

    Ok(Duration::from_secs(seconds))
}

fn unreadable(text: &str) -> String {
    format!("\"{text}\" is not a valid duration (try 30m, 6h, 2d or 2w)")
}

/// Writes a duration back the way somebody would have typed it: the largest
/// unit that divides it exactly, so [`parse`] gives the same duration back.
pub fn format(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds == 0 {
        return "0".to_string();
    }
    for (unit, size) in [('w', 604_800), ('d', 86_400), ('h', 3_600), ('m', 60)] {
        if seconds.is_multiple_of(size) {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_parses_the_usual_durations() {
        assert_eq!(parse("30m").unwrap(), Duration::from_secs(1_800));
        assert_eq!(parse("6h").unwrap(), Duration::from_secs(21_600));
        assert_eq!(parse("2d").unwrap(), Duration::from_secs(172_800));
        assert_eq!(parse("45s").unwrap(), Duration::from_secs(45));
        assert_eq!(parse("90").unwrap(), Duration::from_secs(90));
        assert_eq!(parse(" 6h ").unwrap(), Duration::from_secs(21_600));
    }

    /// "Every two weeks" is a thing people schedule and the one interval cron cannot express.
    #[test]
    fn it_parses_weeks() {
        assert_eq!(parse("1w").unwrap(), Duration::from_secs(604_800));
        assert_eq!(parse("2w").unwrap(), Duration::from_secs(1_209_600));
    }

    #[test]
    fn it_rejects_what_is_not_a_duration() {
        for bad in ["", "h", "six hours", "6x", "-3h", "6.5h", "w"] {
            assert!(parse(bad).is_err(), "\"{bad}\" should be rejected");
        }
    }

    #[test]
    fn a_duration_too_long_to_hold_is_refused_rather_than_wrapped() {
        assert!(parse(&format!("{}w", u64::MAX)).is_err());
    }

    /// The half `checked_mul` does not cover, and the one that is reachable.
    ///
    /// Every reader of one of these compares it against a timestamp, so the
    /// number leaves here as `u64` and arrives as `i64`. Between the two
    /// bounds sits a whole range that multiplies without overflowing and then
    /// casts negative — `now() - taken_at <= max_age` false for every capture
    /// that exists, so the longest maximum age anyone can write is the one
    /// that walks the list every single time.
    #[test]
    fn a_duration_no_timestamp_could_be_compared_against_is_refused_too() {
        let past_i64 = (i64::MAX as u64) / 604_800 + 1;
        let text = format!("{past_i64}w");

        assert!(
            past_i64.checked_mul(604_800).is_some(),
            "the point of this case is that it fits in a u64"
        );
        assert!(parse(&text).is_err(), "{text} parsed");

        // And the largest one that does fit still parses, so the bound is
        // where it says it is rather than a round number near it.
        let largest = (i64::MAX as u64) / 604_800;
        assert_eq!(
            parse(&format!("{largest}w")).unwrap().as_secs(),
            largest * 604_800
        );
    }

    /// What is written has to read back as what it was, or the configuration
    /// file drifts away from the schedule it describes every time it is saved.
    #[test]
    fn writing_and_reading_back_gives_the_same_duration() {
        for text in ["0", "45s", "30m", "6h", "2d", "2w"] {
            let parsed = parse(text).unwrap();
            assert_eq!(parse(&format(parsed)).unwrap(), parsed, "{text}");
        }
    }

    /// The largest unit that divides exactly, so a person reads "6h" rather
    /// than "360m".
    #[test]
    fn it_writes_the_unit_somebody_would_have_typed() {
        assert_eq!(format(Duration::from_secs(21_600)), "6h");
        assert_eq!(format(Duration::from_secs(1_209_600)), "2w");
        assert_eq!(format(Duration::from_secs(90)), "90s");
        assert_eq!(format(Duration::from_secs(0)), "0");
    }
}
