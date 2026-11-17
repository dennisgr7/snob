//! Making a job's name: `j`, the moment it was made, and chance.

use snob_store::paths::JobId;

/// A new id, made `at_ms` milliseconds after the epoch.
pub fn new(at_ms: u64) -> std::io::Result<JobId> {
    let mut random = [0u8; 10];
    crate::system::random::fill_random(&mut random)?;
    Ok(encode(at_ms, random))
}

/// The id of `at_ms` and `random`: ten characters of time, the top two of
/// its fifty bits zero, then sixteen of chance.
fn encode(at_ms: u64, random: [u8; 10]) -> JobId {
    let alphabet = JobId::ALPHABET;
    let mut text = String::with_capacity(27);
    text.push('j');
    let time = at_ms & ((1 << 48) - 1);
    for shift in (0..10).rev() {
        text.push(char::from(alphabet[((time >> (shift * 5)) & 31) as usize]));
    }
    let mut chance = 0u128;
    for byte in random {
        chance = (chance << 8) | u128::from(byte);
    }
    for shift in (0..16).rev() {
        text.push(char::from(
            alphabet[((chance >> (shift * 5)) & 31) as usize],
        ));
    }
    JobId::parse(&text).expect("every character is from the alphabet, and there are 26")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_one_the_store_accepts_and_sorts_by_time() {
        let early = encode(1_700_000_000_000, [0xff; 10]);
        let late = encode(1_700_000_000_001, [0; 10]);
        assert!(early < late, "{early} {late}");
        assert_eq!(early.as_str().len(), 27);
        let made = new(1_700_000_000_000).expect("the system has randomness");
        assert!(JobId::parse(made.as_str()).is_some());
        assert_ne!(
            made,
            new(1_700_000_000_000).unwrap(),
            "chance decides the rest"
        );
    }
}
