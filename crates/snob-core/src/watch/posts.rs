//! Which posts on a watched account's grid are new, from what was read and what
//! was seen before.
//!
//! **A post is news when its pk was never seen and it was taken after the
//! floor**, the newest `taken_at` seen in any earlier read. The pk alone is not
//! enough: after a deletion the posts behind move up with pks never seen, and
//! so does an old post newly pinned or brought back from the archive; all are
//! older than the floor, so they are written down as seen and not reported.
//! The floor is Instagram's time, never this machine's clock.
//!
//! A collaboration accepted days later carries its old `taken_at` and is not
//! reported (AGENTS.md, "Known walls").
//!
//! Pure: no clock, no database, no network.

use std::collections::HashSet;

use crate::Epoch;

/// A post as one read found it on the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub pk: u64,
    pub taken_at: Epoch,
    /// Pinned to the top of the grid, where an old post can sit above new ones.
    pub pinned: bool,
}

/// What one read of the grid found, in the grid's order.
#[derive(Debug, Clone, Copy)]
pub struct Read<'a> {
    pub posts: &'a [Seen],
    /// The grid has no more pages: everything the account shows was read.
    pub ended: bool,
    /// The read stopped at a page holding a post seen before.
    pub stopped_at_known: bool,
}

/// Whether there was anything to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Nothing was ever seen of this account: what was read becomes what is
    /// known, and nothing is news, as on a list's first run.
    Baseline,
    /// What was read was compared against what was seen before.
    Compared,
}

/// What a read amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compared {
    pub basis: Basis,
    /// The posts that are news, in the grid's order.
    pub news: Vec<u64>,
    /// The floor after this read: never lower than the one before.
    pub floor: Epoch,
    /// Whether the read reached back to what was known. `false` when it read
    /// every page it was allowed and found neither a post seen before, one at
    /// or under the floor, nor the end of the grid: older posts newer than
    /// the floor may be missing, and are never reported, since the floor
    /// moves past them.
    pub complete: bool,
}

/// Compares a read against what was known of the account.
///
/// `known` holds which of the read's posts were seen before; `floor` is `None`
/// when no baseline was ever laid.
pub fn compare(known: &HashSet<u64>, floor: Option<Epoch>, read: &Read<'_>) -> Compared {
    let newest_read = read.posts.iter().map(|post| post.taken_at).max();
    let Some(floor) = floor else {
        return Compared {
            basis: Basis::Baseline,
            news: Vec::new(),
            floor: newest_read.unwrap_or_default(),
            complete: true,
        };
    };

    let news = read
        .posts
        .iter()
        .filter(|post| !known.contains(&post.pk) && post.taken_at > floor)
        .map(|post| post.pk)
        .collect();

    // A pinned post sits on the first page whatever its age, so it says
    // nothing about where the read reached.
    let reached_the_known = read
        .posts
        .iter()
        .filter(|post| !post.pinned)
        .any(|post| known.contains(&post.pk) || post.taken_at <= floor);

    Compared {
        basis: Basis::Compared,
        news,
        floor: newest_read.map_or(floor, |newest| newest.max(floor)),
        complete: read.ended || read.stopped_at_known || reached_the_known,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(pk: u64, taken_at: i64) -> Seen {
        Seen {
            pk,
            taken_at: Epoch::new(taken_at),
            pinned: false,
        }
    }

    fn pinned(pk: u64, taken_at: i64) -> Seen {
        Seen {
            pinned: true,
            ..post(pk, taken_at)
        }
    }

    fn read(posts: &[Seen]) -> Read<'_> {
        Read {
            posts,
            ended: false,
            stopped_at_known: false,
        }
    }

    fn known(pks: &[u64]) -> HashSet<u64> {
        pks.iter().copied().collect()
    }

    #[test]
    fn a_first_read_is_a_baseline_and_names_nothing() {
        let posts = [post(3, 300), post(2, 200)];
        let compared = compare(&HashSet::new(), None, &read(&posts));
        assert_eq!(compared.basis, Basis::Baseline);
        assert!(compared.news.is_empty());
        assert_eq!(compared.floor, Epoch::new(300));
        assert!(compared.complete);
    }

    #[test]
    fn a_post_newer_than_everything_seen_is_news() {
        let posts = [post(4, 400), post(3, 300), post(2, 200)];
        let compared = compare(&known(&[3, 2]), Some(Epoch::new(300)), &read(&posts));
        assert_eq!(compared.basis, Basis::Compared);
        assert_eq!(compared.news, vec![4]);
        assert_eq!(compared.floor, Epoch::new(400));
        assert!(compared.complete);
    }

    #[test]
    fn an_old_post_that_moved_up_after_a_deletion_is_not_news() {
        // 3 was deleted; 1, never seen because it sat on the second page,
        // moved up into the first.
        let posts = [post(2, 200), post(1, 100)];
        let compared = compare(&known(&[2]), Some(Epoch::new(300)), &read(&posts));
        assert!(compared.news.is_empty());
        assert_eq!(
            compared.floor,
            Epoch::new(300),
            "the floor never moves back"
        );
    }

    #[test]
    fn a_newly_pinned_old_post_is_not_news() {
        let posts = [pinned(1, 100), post(3, 300), post(2, 200)];
        let compared = compare(&known(&[3, 2]), Some(Epoch::new(300)), &read(&posts));
        assert!(compared.news.is_empty());
    }

    #[test]
    fn a_post_already_seen_is_never_news_again() {
        // Even taken after the floor, which a clock moved on the owner's side
        // or a post edited could make it.
        let posts = [post(4, 400)];
        let compared = compare(&known(&[4]), Some(Epoch::new(300)), &read(&posts));
        assert!(compared.news.is_empty());
    }

    #[test]
    fn two_posts_in_the_same_second_are_told_apart_by_pk() {
        let posts = [post(5, 400), post(4, 400)];
        let compared = compare(&known(&[4]), Some(Epoch::new(300)), &read(&posts));
        assert_eq!(compared.news, vec![5]);
    }

    #[test]
    fn a_read_that_reached_nothing_known_says_it_is_not_complete() {
        let posts = [post(9, 900), post(8, 800), pinned(1, 100)];
        let compared = compare(&known(&[1]), Some(Epoch::new(300)), &read(&posts));
        assert_eq!(compared.news, vec![9, 8]);
        assert!(
            !compared.complete,
            "a pinned post seen before says nothing about where the read reached"
        );

        let ended = Read {
            ended: true,
            ..read(&posts)
        };
        assert!(compare(&known(&[1]), Some(Epoch::new(300)), &ended).complete);
        let stopped = Read {
            stopped_at_known: true,
            ..read(&posts)
        };
        assert!(compare(&known(&[1]), Some(Epoch::new(300)), &stopped).complete);
    }

    #[test]
    fn the_first_post_of_an_empty_account_is_news() {
        let nothing = compare(&HashSet::new(), None, &read(&[]));
        assert_eq!(nothing.floor, Epoch::new(0));

        let posts = [post(1, 100)];
        let compared = compare(
            &HashSet::new(),
            Some(nothing.floor),
            &Read {
                ended: true,
                ..read(&posts)
            },
        );
        assert_eq!(compared.news, vec![1]);
    }
}
