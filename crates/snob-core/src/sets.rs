//! Set operations between two account lists, **always by `pk`**: a username
//! changes, and crossing by name would turn a rename into a departure and an
//! arrival.

use std::collections::HashSet;

use crate::Pk;
use crate::model::{ListKind, User};

fn pk_set(users: &[User]) -> HashSet<Pk> {
    users.iter().map(|u| u.pk).collect()
}

/// Those in `a` and not in `b`, keeping the order of `a`.
pub fn difference(a: &[User], b: &[User]) -> Vec<User> {
    let in_b = pk_set(b);
    a.iter()
        .filter(|u| !in_b.contains(&u.pk))
        .cloned()
        .collect()
}

/// Those in both, keeping the order of `a`.
pub fn intersection(a: &[User], b: &[User]) -> Vec<User> {
    let in_b = pk_set(b);
    a.iter().filter(|u| in_b.contains(&u.pk)).cloned().collect()
}

/// A crossing of one account's two lists, as `unfollowers`, `fans` and
/// `friends` read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Crossing {
    /// Those it follows who do not follow it back.
    Unfollowers,
    /// Those who follow it and it does not follow.
    Fans,
    /// Those that follow each other with it.
    Friends,
}

impl Crossing {
    pub const ALL: [Self; 3] = [Self::Unfollowers, Self::Fans, Self::Friends];

    /// The list the result comes out of.
    pub fn base(self) -> ListKind {
        match self {
            Self::Unfollowers => ListKind::Following,
            Self::Fans | Self::Friends => ListKind::Followers,
        }
    }

    /// The list it is crossed against, **which must be whole**: an account
    /// missing from it shows up in the result without deserving to.
    pub fn against(self) -> ListKind {
        match self {
            Self::Unfollowers => ListKind::Followers,
            Self::Fans | Self::Friends => ListKind::Following,
        }
    }

    /// The crossing of `base` against `against`, by pk, in `base`'s order.
    pub fn of(self, base: &[User], against: &[User]) -> Vec<User> {
        match self {
            Self::Unfollowers | Self::Fans => difference(base, against),
            Self::Friends => intersection(base, against),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unfollowers => "unfollowers",
            Self::Fans => "fans",
            Self::Friends => "friends",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain numbers in, ids out: the arithmetic under test is about which
    /// accounts are in which list, and spelling `Pk::new` sixteen times says
    /// nothing about it.
    fn users(pks: &[u64]) -> Vec<User> {
        pks.iter()
            .map(|&pk| User::named(Pk::new(pk), format!("u{pk}")))
            .collect()
    }

    fn pks(us: &[User]) -> Vec<u64> {
        us.iter().map(|u| u.pk.get()).collect()
    }

    #[test]
    fn difference_keeps_the_order_of_the_first_list() {
        let a = users(&[3, 1, 2, 4]);
        let b = users(&[2, 4]);
        assert_eq!(pks(&difference(&a, &b)), vec![3, 1]);
    }

    #[test]
    fn intersection_keeps_the_order_of_the_first_list() {
        let a = users(&[3, 1, 2, 4]);
        let b = users(&[4, 2]);
        assert_eq!(pks(&intersection(&a, &b)), vec![2, 4]);
    }

    #[test]
    fn an_empty_list_gives_the_expected_result() {
        let a = users(&[1, 2]);
        assert_eq!(pks(&difference(&a, &[])), vec![1, 2]);
        assert!(intersection(&a, &[]).is_empty());
        assert!(difference(&[], &a).is_empty());
    }

    /// Someone who renamed themselves between captures is still the same
    /// person: crossing by username would show them as a departure and an
    /// arrival at once.
    #[test]
    fn lists_are_crossed_by_id_and_not_by_username() {
        let before = users(&[7]);
        let mut after = users(&[7]);
        after[0].username = "renamed_themselves".into();

        assert!(difference(&before, &after).is_empty());
        assert_eq!(pks(&intersection(&before, &after)), vec![7]);
    }

    #[test]
    fn two_identical_lists_leave_no_difference() {
        let a = users(&[1, 2, 3]);
        assert!(difference(&a, &a).is_empty());
        assert_eq!(pks(&intersection(&a, &a)), vec![1, 2, 3]);
    }

    /// Each crossing reads out of its base list against the other, and the
    /// three with the two lists add up as the commands' counts do.
    #[test]
    fn the_crossings_read_their_base_against_the_other_list() {
        let followers = users(&[1, 2, 3]);
        let following = users(&[2, 3, 4, 5]);
        let of = |crossing: Crossing| {
            let (base, against) = match crossing.base() {
                ListKind::Followers => (&followers, &following),
                ListKind::Following => (&following, &followers),
            };
            crossing
                .of(base, against)
                .iter()
                .map(|u| u.pk.get())
                .collect::<Vec<_>>()
        };
        assert_eq!(of(Crossing::Unfollowers), [4, 5]);
        assert_eq!(of(Crossing::Fans), [1]);
        assert_eq!(of(Crossing::Friends), [2, 3]);
        for crossing in Crossing::ALL {
            assert_ne!(crossing.base(), crossing.against());
        }
    }
}
