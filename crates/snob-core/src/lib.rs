//! snob's domain: models, set arithmetic, the diff between two captures, the
//! monitor's schedule, a report's signature and the request budget's interface.
//!
//! **No I/O.** Nothing here opens a database, makes a request, reads a file or
//! names a directory; [`clock`] is the single exception. That is what lets
//! `snob-ig` build without SQLite, the keyring backends or a TOML parser.
//!
//! An account is identified by its numeric `pk`, never by its username.

pub mod budget;
pub mod clock;
pub mod duration;
pub mod filters;
pub mod model;
pub mod secret;
pub mod session;
pub mod sets;
pub mod watch;

pub use clock::{Epoch, EpochMs};

/// Stable numeric identifier of an Instagram account.
///
/// **No `Deref`, no `From<Pk> for u64` and no arithmetic, on purpose**: an id
/// that can be added to a count is an alias. [`Pk::get`] is the one way out.
/// No `ToSql`/`FromSql` either (this crate must not compile SQLite): the
/// database goes through `snob_store::store::pk_to_sql` / `pk_from_sql`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Pk(u64);

impl Pk {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for Pk {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// The bare digits.
impl std::fmt::Display for Pk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::str::FromStr for Pk {
    type Err = std::num::ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::Pk;

    /// Every shape this appears in on the wire and on screen is the bare
    /// number, and that is a contract with whoever consumes the tool.
    #[test]
    fn an_id_is_still_written_as_a_plain_number() {
        let pk = Pk::new(4_340_136_074);

        assert_eq!(pk.to_string(), "4340136074");
        assert_eq!(serde_json::to_string(&pk).unwrap(), "4340136074");
        assert_eq!(
            serde_json::from_str::<Pk>("4340136074").unwrap(),
            pk,
            "and reads back the same way"
        );
        assert_eq!("4340136074".parse::<Pk>().unwrap(), pk);
    }
}
