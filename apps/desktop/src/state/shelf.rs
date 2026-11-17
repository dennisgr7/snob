//! The editor's panel of steps, apart from how it is drawn: the four kinds
//! of steps, or the steps of one, and which of those a search shows.
//!
//! **An automation with no trigger is held at the triggers**, with no way
//! back to the kinds: every automation starts with one. Once it has one,
//! the panel shows the kinds and goes anywhere, and it is held again if the
//! last one goes.
//!
//! No window and no clock: whether the automation has a trigger is handed in.

use snob_core::automation::{Automation, Stage};

use super::commands::matches;
use super::settings::fold;

/// What the panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    /// The four kinds, each a way in.
    Kinds,
    /// The steps of one kind.
    In(Stage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shelf {
    at: At,
    held: bool,
    forward: bool,
}

/// Whether `automation` has a trigger, enabled or not: what lets the panel
/// go past the triggers.
pub fn has_a_trigger(automation: &Automation) -> bool {
    automation
        .nodes
        .iter()
        .any(|node| node.step.stage() == Stage::Trigger)
}

impl Shelf {
    /// The panel as the editor opens on an automation with a trigger or not.
    pub fn new(has_a_trigger: bool) -> Self {
        if has_a_trigger {
            Self {
                at: At::Kinds,
                held: false,
                forward: false,
            }
        } else {
            Self {
                at: At::In(Stage::Trigger),
                held: true,
                forward: true,
            }
        }
    }

    pub fn at(&self) -> At {
        self.at
    }

    /// Whether it is held at the triggers, with no way back.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Whether it last went in rather than back out: the side what it shows
    /// comes in from.
    pub fn forward(&self) -> bool {
        self.forward
    }

    /// Follows the automation: held at the triggers while it has none, and
    /// at the kinds once one comes. Whether that moved it.
    pub fn follow(&mut self, has_a_trigger: bool) -> bool {
        match (has_a_trigger, self.held) {
            (false, false) => {
                let moved = self.at != At::In(Stage::Trigger);
                *self = Self::new(false);
                moved
            }
            (true, true) => {
                *self = Self::new(true);
                true
            }
            _ => false,
        }
    }

    /// Into the steps of `stage`. Whether it moved: never while held.
    pub fn enter(&mut self, stage: Stage) -> bool {
        if self.held || self.at == At::In(stage) {
            return false;
        }
        self.at = At::In(stage);
        self.forward = true;
        true
    }

    /// Back to the kinds. Whether it moved: never while held.
    pub fn back(&mut self) -> bool {
        if self.held || self.at == At::Kinds {
            return false;
        }
        self.at = At::Kinds;
        self.forward = false;
        true
    }
}

/// Whether a step named `name`, which says `note` of itself, is what
/// `typed` asks for: every word typed in one or the other, whatever the
/// case and the accents. Nothing typed asks for every step.
pub fn found(name: &str, note: &str, typed: &str) -> bool {
    matches(&fold(&format!("{name} {note}")), &fold(typed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::automation::Step;

    #[test]
    fn an_automation_with_no_trigger_is_held_at_the_triggers_until_one_comes() {
        let mut shelf = Shelf::new(false);
        assert_eq!(shelf.at(), At::In(Stage::Trigger));
        assert!(shelf.held());
        assert!(!shelf.back(), "no way back while held");
        assert!(!shelf.enter(Stage::Action), "no way elsewhere either");
        assert_eq!(shelf.at(), At::In(Stage::Trigger));

        assert!(shelf.follow(true), "a trigger came");
        assert_eq!(shelf.at(), At::Kinds);
        assert!(!shelf.held());
        assert!(!shelf.follow(true), "nothing more to do");
    }

    #[test]
    fn with_a_trigger_the_kinds_go_in_and_back_from_each_side() {
        let mut shelf = Shelf::new(true);
        assert_eq!(shelf.at(), At::Kinds);
        assert!(shelf.enter(Stage::Filter));
        assert_eq!(shelf.at(), At::In(Stage::Filter));
        assert!(shelf.forward());
        assert!(!shelf.enter(Stage::Filter), "already there");
        assert!(shelf.back());
        assert_eq!(shelf.at(), At::Kinds);
        assert!(!shelf.forward());
        assert!(!shelf.back(), "already at the kinds");
    }

    #[test]
    fn the_last_trigger_gone_holds_it_again_wherever_it_was() {
        let mut shelf = Shelf::new(true);
        shelf.enter(Stage::Output);
        assert!(shelf.follow(false));
        assert_eq!(shelf.at(), At::In(Stage::Trigger));
        assert!(shelf.held());

        let mut inside = Shelf::new(true);
        inside.enter(Stage::Trigger);
        assert!(!inside.follow(false), "already at the triggers: no move");
        assert!(inside.held());
    }

    #[test]
    fn a_disabled_trigger_is_still_a_trigger() {
        let mut automation = Automation::new("a1", "One");
        assert!(!has_a_trigger(&automation));
        let manual = automation.insert(Step::Manual {}, (0, 0));
        automation.set_enabled(manual, false);
        assert!(has_a_trigger(&automation));
    }

    #[test]
    fn a_search_finds_a_step_by_its_name_or_its_note_without_accents() {
        let (name, note) = (
            "Manually",
            "Only when you run it, from the r\u{e9}sum\u{e9}",
        );
        assert!(found(name, note, ""));
        assert!(found(name, note, "manual"));
        assert!(found(name, note, "RUN resume"));
        assert!(found(name, note, "resume manual"));
        assert!(!found(name, note, "webhook"));
    }
}
