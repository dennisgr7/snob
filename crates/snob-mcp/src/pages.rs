//! The long lists `snob mcp` handed back one page of, held for the pages after
//! it: **a call for a later page never walks anything**, and the page it gets
//! has to be a page of the same list the first call read.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// How many lists are held at once: the few an agent pages through in one
/// conversation. The oldest goes first.
const HELD: usize = 4;

/// How long a list is held after it was read.
const HELD_FOR: Duration = Duration::from_secs(30 * 60);

/// The most held at once, counted as the compact JSON of the items. An
/// account is held without its picture's address (`answer::LEFT_OUT`): 100
/// to 250 bytes, so the longest walk there is (`pager::HARD_PAGE_CAP` pages
/// of 12, 24,000 accounts) is 6 MiB at most, and this is a few long lists
/// and a bound on what a server started and forgotten in the background
/// keeps.
const HELD_BYTES: usize = 16 << 20;

/// The lists held, newest first.
#[derive(Default)]
pub struct Held {
    lists: Mutex<VecDeque<List>>,
}

/// A small picture a call hands back for the model to look at
/// (`answer::take_pictures`), or the word that there is none.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    /// The number, in the call's listing, of what it is a picture of.
    pub of: u64,
    /// What the answer says of it beside the JSON: `{of, mime, bytes}`, or
    /// `{of, missing}`.
    pub said: Value,
    /// Its kind and its bytes in base64, when there is a picture.
    pub image: Option<(String, String)>,
}

/// A whole list as a call read it.
#[derive(Debug, Default, PartialEq)]
pub struct Whole {
    /// Each item as its compact JSON, which is how pages are measured
    /// (`answer::page`).
    pub items: Vec<String>,
    /// What the first page said about the whole list (`answer::ABOUT`):
    /// where it came from and when, and whether it is all of it, which every
    /// later page says again.
    pub about: Map<String, Value>,
    /// The small pictures the call read with it, each handed back with the
    /// page that holds what it is of.
    pub pictures: Vec<Picture>,
    /// The `limit` of the call that read it, for a later page asked for
    /// without that call's arguments: a job's (`job::held`).
    pub limit: Option<usize>,
}

struct List {
    /// The call that read it (`tools::Page::of`).
    of: String,
    at: Instant,
    whole: Arc<Whole>,
    bytes: usize,
}

impl Held {
    /// Holds `whole`, the whole list the call `of` read, in place of what an
    /// earlier run of that call read.
    pub fn keep(&self, of: String, whole: Whole) {
        self.keep_at(of, whole, Instant::now());
    }

    /// The whole list the call `of` read, while it is held.
    pub fn get(&self, of: &str) -> Option<Arc<Whole>> {
        self.get_at(of, Instant::now())
    }

    /// Forgets every list: what was read as an account is not handed back
    /// once the server has signed one out.
    pub fn clear(&self) {
        self.lock().clear();
    }

    fn keep_at(&self, of: String, whole: Whole, now: Instant) {
        let pictures = whole
            .pictures
            .iter()
            .map(|picture| picture.image.as_ref().map_or(0, |(_, data)| data.len()));
        let bytes = whole.items.iter().map(String::len).sum::<usize>()
            + pictures.sum::<usize>()
            + Value::Object(whole.about.clone()).to_string().len();
        let mut lists = self.lock();
        lists.retain(|list| list.of != of && !expired(list, now));
        if bytes > HELD_BYTES {
            return;
        }
        lists.push_front(List {
            of,
            at: now,
            whole: Arc::new(whole),
            bytes,
        });
        lists.truncate(HELD);
        while lists.iter().map(|list| list.bytes).sum::<usize>() > HELD_BYTES {
            lists.pop_back();
        }
    }

    fn get_at(&self, of: &str, now: Instant) -> Option<Arc<Whole>> {
        let mut lists = self.lock();
        lists.retain(|list| !expired(list, now));
        lists
            .iter()
            .find(|list| list.of == of)
            .map(|list| Arc::clone(&list.whole))
    }

    /// The lists, whatever a call that panicked while holding them left:
    /// every change above leaves them whole, and losing a page is better
    /// than losing the server.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<List>> {
        self.lists
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn expired(list: &List, now: Instant) -> bool {
    now.saturating_duration_since(list.at) >= HELD_FOR
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Whole {
        Whole {
            items: (0..n).map(|n| format!(r#"{{"pk":{n}}}"#)).collect(),
            ..Whole::default()
        }
    }

    #[test]
    fn a_list_is_held_for_its_own_call_and_replaced_by_a_new_read() {
        let held = Held::default();
        let now = Instant::now();
        held.keep_at("a".into(), items(3), now);
        assert_eq!(held.get_at("a", now).unwrap().items.len(), 3);
        assert!(held.get_at("b", now).is_none());

        held.keep_at("a".into(), items(5), now);
        assert_eq!(held.get_at("a", now).unwrap().items.len(), 5);
        assert_eq!(held.lock().len(), 1, "one list per call");
    }

    #[test]
    fn the_oldest_list_goes_first() {
        let held = Held::default();
        let now = Instant::now();
        for of in ["a", "b", "c", "d", "e"] {
            held.keep_at(of.into(), items(1), now);
        }
        assert!(held.get_at("a", now).is_none());
        for of in ["b", "c", "d", "e"] {
            assert!(held.get_at(of, now).is_some(), "{of}");
        }
    }

    #[test]
    fn a_list_is_forgotten_once_its_time_is_up() {
        let held = Held::default();
        let then = Instant::now();
        held.keep_at("a".into(), items(1), then);
        assert!(held.get_at("a", then + HELD_FOR / 2).is_some());
        assert!(held.get_at("a", then + HELD_FOR).is_none());
        assert!(held.lock().is_empty(), "and no longer kept");
    }

    #[test]
    fn what_is_held_stays_under_its_bytes() {
        let held = Held::default();
        let now = Instant::now();
        let big = |n: usize| Whole {
            items: vec!["x".repeat(n)],
            ..Whole::default()
        };
        held.keep_at("too big".into(), big(HELD_BYTES + 1), now);
        assert!(held.get_at("too big", now).is_none());

        held.keep_at("a".into(), big(HELD_BYTES / 2), now);
        held.keep_at("b".into(), big(HELD_BYTES / 2), now);
        held.keep_at("c".into(), big(1), now);
        assert!(held.get_at("a", now).is_none(), "the oldest made room");
        assert!(held.get_at("b", now).is_some());
        assert!(held.get_at("c", now).is_some());
    }

    #[test]
    fn nothing_is_held_once_the_lists_are_cleared() {
        let held = Held::default();
        held.keep("a".into(), items(3));
        held.keep("b".into(), items(3));
        held.clear();
        assert!(held.get("a").is_none());
        assert!(held.get("b").is_none());
    }

    /// The longest walk there is, each account as long as one can be without
    /// its picture's address, is held, and so is a second list beside it.
    #[test]
    fn the_longest_walk_fits_in_what_is_held() {
        let account = serde_json::json!({
            "pk": "99999999999",
            "username": "u".repeat(30),
            "full_name": "\u{1F600}".repeat(30),
            "is_private": false,
            "is_verified": false,
        })
        .to_string();
        assert!(account.len() <= 250, "{} bytes", account.len());
        let walk = || Whole {
            items: vec![account.clone(); 2_000 * 12],
            ..Whole::default()
        };
        let held = Held::default();
        held.keep("a".into(), walk());
        held.keep("b".into(), walk());
        assert_eq!(held.get("a").unwrap().items.len(), 24_000);
        assert!(held.get("b").is_some());
    }
}
