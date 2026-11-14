//! The temporal diff: what changed in a list between two captures.
//!
//! `gained` and `lost` are this module's words: a difference between two
//! *moments* of one list. The set commands (`unfollowers`, `fans`) answer
//! about two lists at one instant, and the two vocabularies must not mix.
//!
//! Pure. Which two captures may be compared is [`Basis`].

use crate::model::User;
use crate::sets;
use crate::{Epoch, Pk};

/// What changed in one list between two captures.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListDiff {
    /// In the later capture and not the earlier one.
    pub gained: Vec<User>,
    /// In the earlier capture and not the later one.
    pub lost: Vec<User>,
}

impl ListDiff {
    /// Crosses two captures of the same list, **by `pk`**: somebody who
    /// renamed themselves in between is neither a departure nor an arrival.
    pub fn between(before: &[User], after: &[User]) -> Self {
        Self {
            gained: sets::difference(after, before),
            lost: sets::difference(before, after),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.gained.is_empty() && self.lost.is_empty()
    }

    pub fn len(&self) -> usize {
        self.gained.len() + self.lost.len()
    }
}

/// An account that now goes by a different name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub pk: Pk,
    /// The `username_history` row this came from, so a report can say which
    /// renames it announced (migration `007_renames_sent`).
    pub history_id: i64,
    pub from: String,
    pub to: String,
    /// When the change was noticed, not when it happened.
    pub at: Epoch,
}

/// Which two captures a comparison may be made from, if any.
///
/// Decided from a mark and an id alone, so the members are read off the disk
/// only when they are going to be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Something is stored but nothing was ever reported for this list.
    /// **Everybody's first run, and it reports nothing**: the mark is set and
    /// the run stays quiet.
    Baseline { snapshot_id: i64 },
    /// The mark already points at the newest usable capture: nothing was
    /// walked since the last report.
    Unchanged { snapshot_id: i64 },
    /// Two different captures, in this order.
    Compare { before: i64, after: i64 },
}

impl Basis {
    /// Works out what can be compared. `None` when nothing is stored.
    ///
    /// `latest` must come from `usable_snapshots`, which cannot hand back an
    /// incomplete capture: the accounts missing from one would be reported as
    /// departures.
    pub fn decide(mark: Option<i64>, latest: Option<i64>) -> Option<Self> {
        let latest = latest?;
        match mark {
            None => Some(Self::Baseline {
                snapshot_id: latest,
            }),
            // By id, not by date: two captures can share a timestamp.
            //
            // **`>=`, not `==`.** Two overlapping runs (there is no run lock)
            // can finish out of order, and the slower one then holds a
            // capture older than the one already reported: comparing would
            // report arrivals as `lost` and departures as `gained`. It is
            // `Unchanged` **at the mark**, since moving the mark back would
            // report the same window again.
            Some(mark) if mark >= latest => Some(Self::Unchanged { snapshot_id: mark }),
            Some(mark) => Some(Self::Compare {
                before: mark,
                after: latest,
            }),
        }
    }

    /// The capture the mark should point at once this run is over.
    pub fn mark_to(self) -> i64 {
        match self {
            Self::Baseline { snapshot_id } | Self::Unchanged { snapshot_id } => snapshot_id,
            Self::Compare { after, .. } => after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain numbers in, ids out: what these tests are about is which account
    /// is in which list, not how one is spelled.
    fn users(pks: &[u64]) -> Vec<User> {
        pks.iter()
            .map(|&pk| User::named(Pk::new(pk), format!("u{pk}")))
            .collect()
    }

    fn pks(us: &[User]) -> Vec<u64> {
        us.iter().map(|u| u.pk.get()).collect()
    }

    /// **A run that finishes holding an older capture reports nothing.**
    ///
    /// Two runs overlapping is ordinary: no run lock, `once` consults no
    /// schedule, and a walk longer than the timer interval is how people are
    /// told to drive this from cron. Without the `>=` bound, the slower one
    /// would build `Compare { before: 101, after: 100 }` and every arrival
    /// would come out as a departure.
    #[test]
    fn a_capture_older_than_the_mark_is_not_compared_backwards() {
        let basis = Basis::decide(Some(101), Some(100)).expect("something is stored");
        assert_eq!(
            basis,
            Basis::Unchanged { snapshot_id: 101 },
            "an older capture has nothing to add, and must not move the mark back"
        );
        assert_eq!(basis.mark_to(), 101, "the mark does not regress");

        // The forward case is untouched.
        assert_eq!(
            Basis::decide(Some(100), Some(101)),
            Some(Basis::Compare {
                before: 100,
                after: 101
            })
        );
    }

    #[test]
    fn an_arrival_is_gained_and_a_departure_is_lost() {
        let before = users(&[1, 2, 3]);
        let after = users(&[2, 3, 4]);
        let diff = ListDiff::between(&before, &after);

        assert_eq!(pks(&diff.gained), vec![4]);
        assert_eq!(pks(&diff.lost), vec![1]);
        assert_eq!(diff.len(), 2);
    }

    #[test]
    fn two_identical_captures_leave_nothing_to_report() {
        let list = users(&[1, 2, 3]);
        assert!(ListDiff::between(&list, &list).is_empty());
    }

    /// The defect the whole module is arranged to avoid. Renames are something
    /// this monitor reports in their own right, so a rename read as a departure
    /// **and** an arrival would have the same run contradict itself about one
    /// person.
    #[test]
    fn somebody_who_renamed_themselves_neither_arrived_nor_left() {
        let before = users(&[7]);
        let mut after = users(&[7]);
        after[0].username = "renamed_themselves".into();

        assert!(ListDiff::between(&before, &after).is_empty());
    }

    /// Nothing walked yet, nothing to say — and in particular no baseline to
    /// set, because setting one against a capture that does not exist would
    /// mark a run that never happened.
    #[test]
    fn nothing_stored_is_nothing_to_compare() {
        assert_eq!(Basis::decide(None, None), None);
        assert_eq!(Basis::decide(Some(1), None), None);
    }

    /// Everybody's first run. It has to be silent, and it has to leave a mark
    /// so the second one is not also a first run.
    #[test]
    fn a_first_run_is_a_baseline_and_still_moves_the_mark() {
        let basis = Basis::decide(None, Some(9)).unwrap();
        assert_eq!(basis, Basis::Baseline { snapshot_id: 9 });
        assert_eq!(basis.mark_to(), 9);
    }

    /// Told apart from a comparison that found nothing: this one never had to
    /// read a single member row.
    #[test]
    fn a_mark_on_the_newest_capture_is_unchanged() {
        assert_eq!(
            Basis::decide(Some(9), Some(9)),
            Some(Basis::Unchanged { snapshot_id: 9 })
        );
    }

    /// The order matters: `before` is the receipt, `after` is what was just
    /// walked. Swapped, every arrival is reported as a departure.
    #[test]
    fn a_newer_capture_is_compared_against_the_mark_in_that_order() {
        let basis = Basis::decide(Some(4), Some(9)).unwrap();
        assert_eq!(
            basis,
            Basis::Compare {
                before: 4,
                after: 9
            }
        );
        assert_eq!(basis.mark_to(), 9);
    }
}
