//! What a run came to, and the one place its tokens are spelled.
//!
//! The exit status a script reads, the `outcome` column of `watch_runs` and
//! the `"outcome"` field of `--json` share this vocabulary. It lives here
//! because the store writes the column and cannot depend on `snob-cli`.

/// What one run of the monitor came to: `snob-cli`'s `ExitCode` without the
/// numbers, which are a presentation decision (`exit::code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Ok,
    Error,
    /// No session stored, or the one there no longer works.
    NoSession,
    /// Instagram wants the account verified.
    Challenge,
    /// Instagram is throttling requests.
    RateLimited,
    /// The user interrupted the run.
    Interrupted,
}

impl RunOutcome {
    pub const ALL: [RunOutcome; 6] = [
        Self::Ok,
        Self::Error,
        Self::NoSession,
        Self::Challenge,
        Self::RateLimited,
        Self::Interrupted,
    ];

    /// The stable token for this outcome.
    ///
    /// **Frozen**: it feeds `watch_runs.outcome`, whose CHECK constraint
    /// accepts these six values and nothing else.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::NoSession => "no_session",
            Self::Challenge => "challenge",
            Self::RateLimited => "rate_limited",
            Self::Interrupted => "interrupted",
        }
    }

    /// The outcome a stored token names. `None`, never a default, for a token
    /// this build does not write: a newer build wrote it.
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == token)
    }
}

impl std::fmt::Display for RunOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An outcome as a stored row carries it: one this build knows, or the token
/// a newer build wrote, kept as it was so `--json` prints what the column
/// holds.
///
/// **An unknown token is no known outcome**: not `Ok`, and none of those that
/// lift by themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedOutcome {
    Known(RunOutcome),
    Unknown(String),
}

impl RecordedOutcome {
    pub fn from_token(token: &str) -> Self {
        match RunOutcome::from_token(token) {
            Some(outcome) => Self::Known(outcome),
            None => Self::Unknown(token.to_string()),
        }
    }

    /// The token, whichever half this is.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(outcome) => outcome.as_str(),
            Self::Unknown(token) => token,
        }
    }

    /// The outcome this build understands, or `None` for a token it does not.
    pub fn known(&self) -> Option<RunOutcome> {
        match self {
            Self::Known(outcome) => Some(*outcome),
            Self::Unknown(_) => None,
        }
    }
}

impl From<RunOutcome> for RecordedOutcome {
    fn from(outcome: RunOutcome) -> Self {
        Self::Known(outcome)
    }
}

/// An unknown token is unequal to every outcome.
impl PartialEq<RunOutcome> for RecordedOutcome {
    fn eq(&self, other: &RunOutcome) -> bool {
        self.known() == Some(*other)
    }
}

impl std::fmt::Display for RecordedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The token an outcome writes is the token that reads back as it.
    ///
    /// Walked over `ALL` and counted, because a variant dropped from that list
    /// quietly narrows every caller that walks it.
    #[test]
    fn every_outcome_reads_back_from_the_token_it_writes() {
        assert_eq!(RunOutcome::ALL.len(), 6, "an outcome was added or dropped");
        for outcome in RunOutcome::ALL {
            assert_eq!(
                RunOutcome::from_token(outcome.as_str()),
                Some(outcome),
                "{outcome:?} writes {:?} and does not read back from it",
                outcome.as_str()
            );
        }
    }

    /// Two outcomes sharing a token would read back as one of them, and the
    /// round trip above would not notice.
    #[test]
    fn no_two_outcomes_are_spelled_the_same() {
        let mut tokens: Vec<&str> = RunOutcome::ALL.iter().map(|o| o.as_str()).collect();
        tokens.sort_unstable();
        tokens.dedup();
        assert_eq!(tokens.len(), RunOutcome::ALL.len(), "a token is shared");
    }

    /// A token this build does not write is kept as itself, and matches no
    /// outcome at all.
    #[test]
    fn a_token_from_another_build_stays_the_token_it_was() {
        let read = RecordedOutcome::from_token("rate-limited");
        assert_eq!(read, RecordedOutcome::Unknown("rate-limited".into()));
        assert_eq!(read.known(), None);
        assert_ne!(read, RunOutcome::RateLimited);
        assert_ne!(read, RunOutcome::Ok);
        assert_eq!(
            read.as_str(),
            "rate-limited",
            "what is printed back is what is really in the column"
        );
    }

    #[test]
    fn a_token_this_build_writes_reads_back_as_the_outcome() {
        let read = RecordedOutcome::from_token("rate_limited");
        assert_eq!(read.known(), Some(RunOutcome::RateLimited));
        assert_eq!(read, RunOutcome::RateLimited);
        assert_eq!(read.to_string(), "rate_limited");
    }
}
