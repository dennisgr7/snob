//! The monitor's domain: what changed between two captures, and when to look.
//!
//! Lists and moments in, changes out; no database, no request, no clock. Going
//! and looking, and telling somebody, is `snob-cli`'s.

pub mod diff;
pub mod outcome;
pub mod posts;
pub mod schedule;
pub mod sign;

pub use diff::{Basis, ListDiff, Rename};
pub use outcome::{RecordedOutcome, RunOutcome};
pub use schedule::{Due, Schedule, ScheduleError, Weekday};

/// Everything one run found about one account's lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub followers: ListDiff,
    pub following: ListDiff,
    /// Accounts in either list that now go by a different name.
    pub renamed: Vec<Rename>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.followers.is_empty() && self.following.is_empty() && self.renamed.is_empty()
    }

    /// How many individual changes this is: what `watch_runs.changes` stores
    /// and what decides whether a webhook is called.
    pub fn len(&self) -> usize {
        self.followers.len() + self.following.len() + self.renamed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::User;
    use crate::{Epoch, Pk};

    fn user(pk: u64) -> User {
        User::named(Pk::new(pk), format!("u{pk}"))
    }

    #[test]
    fn a_run_that_found_nothing_is_empty() {
        assert!(Changes::default().is_empty());
        assert_eq!(Changes::default().len(), 0);
    }

    /// A rename on its own is a change worth sending. Counting only "changes
    /// to the lists" would make a run whose only news was a rename look like a
    /// quiet one and never send it.
    #[test]
    fn a_rename_alone_still_counts_as_a_change() {
        let changes = Changes {
            renamed: vec![Rename {
                pk: Pk::new(7),
                history_id: 7,
                from: "before".into(),
                to: "after".into(),
                at: Epoch::new(1_000),
            }],
            ..Default::default()
        };
        assert!(!changes.is_empty());
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn each_list_is_reachable_by_its_kind() {
        let changes = Changes {
            followers: ListDiff {
                gained: vec![user(1)],
                lost: vec![],
            },
            following: ListDiff {
                gained: vec![],
                lost: vec![user(2)],
            },
            renamed: vec![],
        };

        assert_eq!(changes.followers.gained.len(), 1);
        assert_eq!(changes.following.lost.len(), 1);
        assert_eq!(changes.len(), 2);
    }
}
