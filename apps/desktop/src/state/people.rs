//! What a list of followers or following knows apart from how it is drawn:
//! which rows' small pictures to ask for from the rows drawn, the lists left
//! that stay in memory, the rows chosen and what copying them gives, and
//! what the line under the title says. **Nothing here sends anything**: a
//! picture asked for is one download from the CDN, and choosing rows only
//! copies their usernames.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use snob_core::{Epoch, Pk};

/// How many rows past each end of those drawn have their picture asked for
/// too, so a short scroll finds them there.
pub const AROUND: usize = 4;

/// How long the rows drawn stay the same before their pictures are asked
/// for: a scroll going on asks for nothing.
pub const SETTLES_AFTER: Duration = Duration::from_millis(150);

/// How many lists left stay in memory, the one shown longest ago let go
/// first.
pub const PARKED_AT_MOST: usize = 3;

/// The rows whose picture to ask for, drawn ones first, then the
/// [`AROUND`] below and above them: each one with no picture that has an
/// address not tried yet; none while no row is drawn. `address` is a row's
/// pk and, when it has no picture, the address its list answered.
pub fn wanted_pictures<T>(
    rows: &[T],
    drawn: Range<usize>,
    tried: &HashMap<Pk, String>,
    address: impl Fn(&T) -> (Pk, Option<&str>),
) -> Vec<(Pk, String)> {
    let end = drawn.end.min(rows.len());
    let start = drawn.start.min(end);
    if start == end {
        return Vec::new();
    }
    let below = end..(end + AROUND).min(rows.len());
    let above = start.saturating_sub(AROUND)..start;
    (start..end)
        .chain(below)
        .chain(above)
        .filter_map(|at| {
            let (pk, from) = address(&rows[at]);
            let from = from.filter(|from| !from.is_empty())?;
            (tried.get(&pk).map(String::as_str) != Some(from)).then(|| (pk, from.to_string()))
        })
        .collect()
}

/// Where the row at the top before a list's rows were replaced is now: the
/// rows to move the scroll by so it stays at the top. None when it is gone
/// or did not move.
pub fn moved_by(top: usize, before: &[Pk], after: &[Pk]) -> Option<isize> {
    let pk = before.get(top)?;
    let now = after.iter().position(|other| other == pk)?;
    let by = now as isize - top as isize;
    (by != 0).then_some(by)
}

/// The lists left that stay in memory, the one shown longest ago first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked<K> {
    keys: Vec<K>,
}

impl<K> Default for Parked<K> {
    fn default() -> Self {
        Self { keys: Vec::new() }
    }
}

impl<K: Clone + PartialEq> Parked<K> {
    /// Keeps `key` as the one shown last, and answers those let go to keep
    /// at most [`PARKED_AT_MOST`].
    pub fn park(&mut self, key: K) -> Vec<K> {
        self.keys.retain(|kept| *kept != key);
        self.keys.push(key);
        let over = self.keys.len().saturating_sub(PARKED_AT_MOST);
        self.keys.drain(..over).collect()
    }

    /// Takes `key` out, to be shown: whether it was kept.
    pub fn take(&mut self, key: &K) -> bool {
        let before = self.keys.len();
        self.keys.retain(|kept| kept != key);
        before != self.keys.len()
    }

    /// Lets go of every one `keep` refuses, and answers them.
    pub fn let_go(&mut self, keep: impl Fn(&K) -> bool) -> Vec<K> {
        let (kept, gone) = std::mem::take(&mut self.keys).into_iter().partition(keep);
        self.keys = kept;
        gone
    }

    /// The lists kept, the one shown longest ago first.
    pub fn keys(&self) -> &[K] {
        &self.keys
    }
}

/// The rows chosen in a list, while choosing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    choosing: bool,
    chosen: HashSet<Pk>,
}

impl Selection {
    /// Whether rows are being chosen.
    pub fn choosing(&self) -> bool {
        self.choosing
    }

    /// Starts choosing, none chosen.
    pub fn start(&mut self) {
        self.choosing = true;
        self.chosen.clear();
    }

    /// Stops choosing, and forgets what was chosen.
    pub fn stop(&mut self) {
        self.choosing = false;
        self.chosen.clear();
    }

    pub fn is_chosen(&self, pk: Pk) -> bool {
        self.chosen.contains(&pk)
    }

    /// Chooses `pk`, or leaves it.
    pub fn toggle(&mut self, pk: Pk) {
        if !self.chosen.remove(&pk) {
            self.chosen.insert(pk);
        }
    }

    /// Chooses every one of `rows`.
    pub fn all(&mut self, rows: impl IntoIterator<Item = Pk>) {
        self.chosen.extend(rows);
    }

    /// Keeps chosen only those still among `rows`, once a list's rows were
    /// replaced: **rows coming in leave what was chosen chosen**.
    pub fn keep_among(&mut self, rows: impl IntoIterator<Item = Pk>) {
        let rows: HashSet<Pk> = rows.into_iter().collect();
        self.chosen.retain(|pk| rows.contains(pk));
    }

    pub fn count(&self) -> usize {
        self.chosen.len()
    }

    /// Whether every one of `rows` is chosen.
    pub fn has_all(&self, rows: impl IntoIterator<Item = Pk>) -> bool {
        rows.into_iter().all(|pk| self.chosen.contains(&pk))
    }

    /// The usernames of the rows chosen, in the list's order, without their
    /// at sign, one a line, and how many: a row with no username is left
    /// out.
    pub fn usernames<'a>(
        &self,
        rows: impl IntoIterator<Item = (Pk, Option<&'a str>)>,
    ) -> (String, usize) {
        let names: Vec<&str> = rows
            .into_iter()
            .filter(|(pk, _)| self.chosen.contains(pk))
            .filter_map(|(_, username)| username)
            .collect();
        (names.join("\n"), names.len())
    }
}

/// A list's rows as a whole answer: stored, or read to its end now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answered {
    pub count: usize,
    pub whole: bool,
    /// When the walk it is the answer of ended.
    pub taken_at: Epoch,
}

/// A read going: the rows read so far, and how many the account says it
/// holds once known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Going {
    pub read: usize,
    pub expected: Option<u64>,
}

/// What the line under a list's title says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Said {
    /// Nothing is known yet.
    Nothing,
    /// Read for the first time: its rows come in as the walk reads them.
    Reading(Going),
    /// Never answered whole, and the read failed.
    Failed,
    /// A whole answer, and whether it is being read again, or that read
    /// failed.
    Answered {
        answered: Answered,
        updating: Option<Going>,
        failed: bool,
    },
}

/// What the line under the title says of a list: its answer, the read
/// going and whether the last one failed.
pub fn said(answered: Option<Answered>, going: Option<Going>, failed: bool) -> Said {
    match (answered, going) {
        (Some(answered), updating) => Said::Answered {
            answered,
            updating,
            failed: failed && updating.is_none(),
        },
        (None, Some(going)) => Said::Reading(going),
        (None, None) if failed => Said::Failed,
        (None, None) => Said::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(n: u64) -> Pk {
        Pk::new(n)
    }

    /// Rows 0 to `count`, with a picture already where `kept` says.
    fn rows(count: u64, kept: &[u64]) -> Vec<(Pk, Option<String>)> {
        (0..count)
            .map(|n| {
                let address = (!kept.contains(&n)).then(|| format!("https://cdn/{n}.jpg"));
                (pk(n), address)
            })
            .collect()
    }

    fn wanted(
        rows: &[(Pk, Option<String>)],
        drawn: Range<usize>,
        tried: &HashMap<Pk, String>,
    ) -> Vec<u64> {
        wanted_pictures(rows, drawn, tried, |(pk, from)| (*pk, from.as_deref()))
            .into_iter()
            .map(|(pk, _)| pk.get())
            .collect()
    }

    #[test]
    fn pictures_are_wanted_for_the_rows_drawn_and_a_few_around_them_only() {
        let rows = rows(40, &[12]);
        let none = HashMap::new();
        assert_eq!(
            wanted(&rows, 10..14, &none),
            [10, 11, 13, 14, 15, 16, 17, 6, 7, 8, 9],
            "drawn first, a kept picture left out, then below and above"
        );
        assert_eq!(wanted(&rows, 0..3, &none), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(wanted(&rows, 37..40, &none), [37, 38, 39, 33, 34, 35, 36]);
        assert!(wanted(&rows, 50..60, &none).is_empty(), "past the end");
        assert!(wanted(&rows, 0..0, &none).is_empty(), "nothing drawn yet");
    }

    #[test]
    fn a_picture_tried_at_an_address_is_not_tried_again_until_the_address_changes() {
        let mut rows = rows(3, &[]);
        let tried: HashMap<Pk, String> = [(pk(1), "https://cdn/1.jpg".to_string())].into();
        assert_eq!(wanted(&rows, 0..3, &tried), [0, 2]);
        rows[1].1 = Some("https://cdn/1-new.jpg".to_string());
        assert_eq!(wanted(&rows, 0..3, &tried), [0, 1, 2]);
    }

    #[test]
    fn the_row_at_the_top_stays_at_the_top_when_a_list_is_replaced() {
        let before = [pk(5), pk(6), pk(7)];
        let after = [pk(1), pk(5), pk(6), pk(7)];
        assert_eq!(moved_by(1, &before, &after), Some(1));
        assert_eq!(moved_by(0, &before, &before), None, "it did not move");
        assert_eq!(moved_by(0, &before, &[pk(9)]), None, "it is gone");
    }

    #[test]
    fn the_lists_left_are_kept_up_to_three_the_one_shown_longest_ago_let_go_first() {
        let mut parked = Parked::default();
        assert!(parked.park('a').is_empty());
        assert!(parked.park('b').is_empty());
        assert!(parked.park('c').is_empty());
        assert!(
            parked.park('a').is_empty(),
            "parked again, it is the newest"
        );
        assert_eq!(parked.park('d'), ['b']);
        assert_eq!(parked.keys(), ['c', 'a', 'd']);
        assert!(parked.take(&'a'));
        assert!(!parked.take(&'a'));
        assert_eq!(parked.let_go(|key| *key != 'c'), ['c']);
        assert_eq!(parked.keys(), ['d']);
    }

    #[test]
    fn usernames_copied_are_the_chosen_ones_in_the_list_s_order_one_a_line() {
        let mut selection = Selection::default();
        selection.start();
        assert!(selection.choosing());
        selection.toggle(pk(3));
        selection.toggle(pk(1));
        selection.toggle(pk(2));
        selection.toggle(pk(2));
        let rows = [
            (pk(1), Some("anna")),
            (pk(2), Some("bob")),
            (pk(3), Some("carla")),
            (pk(4), None),
        ];
        assert_eq!(selection.usernames(rows), ("anna\ncarla".to_string(), 2));
        selection.all(rows.iter().map(|(pk, _)| *pk));
        assert!(selection.has_all(rows.iter().map(|(pk, _)| *pk)));
        assert_eq!(
            selection.usernames(rows),
            ("anna\nbob\ncarla".to_string(), 3),
            "no username, no line"
        );
        selection.stop();
        assert!(!selection.choosing());
        assert_eq!(selection.count(), 0);
    }

    #[test]
    fn rows_coming_in_leave_what_was_chosen_chosen() {
        let mut selection = Selection::default();
        selection.start();
        selection.toggle(pk(1));
        selection.toggle(pk(2));
        selection.keep_among([pk(1), pk(3), pk(4)]);
        assert!(selection.is_chosen(pk(1)));
        assert!(
            !selection.is_chosen(pk(2)),
            "a row gone is no longer chosen"
        );
        assert_eq!(selection.count(), 1);
    }

    #[test]
    fn the_line_under_the_title_says_the_answer_and_the_read_behind_it() {
        let answered = Answered {
            count: 30,
            whole: true,
            taken_at: Epoch::new(1_790_000_000),
        };
        let going = Going {
            read: 12,
            expected: Some(40),
        };
        assert_eq!(said(None, None, false), Said::Nothing);
        assert_eq!(said(None, Some(going), false), Said::Reading(going));
        assert_eq!(said(None, None, true), Said::Failed);
        assert_eq!(
            said(Some(answered), Some(going), false),
            Said::Answered {
                answered,
                updating: Some(going),
                failed: false
            }
        );
        assert_eq!(
            said(Some(answered), None, true),
            Said::Answered {
                answered,
                updating: None,
                failed: true
            },
            "a failed read again leaves the answer, said with why"
        );
    }
}
