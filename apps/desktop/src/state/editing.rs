//! The automation the editor holds: a working copy with its undo and redo,
//! what asking to keep it comes to, and what leaving it asks.
//!
//! **The editor saves instantly**: every method that changes the copy says
//! so by answering true, and the view then keeps the copy through the
//! backend ([`Saves`] sends one at a time, the latest last). **Leaving an
//! unfinished automation is asked about**: [`Editing::leaving`], and
//! discarding puts back what the editor opened with ([`Editing::discarding`]).
//!
//! No window, no clock and no engine: what makes a change is handed in.

use snob_core::automation::{Automation, NodeId, Problem, Stage, Step, Unjoined};

/// What `Ctrl+Z` can go back through: more than anybody undoes in a sitting.
const MOST_UNDOS: usize = 200;

/// Where a step of `stage` is first put, in the canvas's own units: a
/// column for each stage, the next row of it free.
pub fn suggested_place(automation: &Automation, stage: Stage) -> (i32, i32) {
    let column = match stage {
        Stage::Trigger => 0,
        Stage::Action => 1,
        Stage::Filter => 2,
        Stage::Output => 3,
    };
    let row = automation
        .nodes
        .iter()
        .filter(|node| node.step.stage() == stage)
        .count() as i32;
    (column * 230 - 345, row * 100 - 40)
}

/// The automation as the editor holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editing {
    working: Automation,
    opened_as: Automation,
    /// Made by "Create": nothing of it was kept before the editor.
    new: bool,
    past: Vec<Automation>,
    future: Vec<Automation>,
    /// The last change was a letter typed in its name, which the next one
    /// joins rather than being an undo of its own.
    typing: bool,
    /// What the engine said keeps a copy from running among the others.
    asked: Option<(Automation, Vec<Problem>)>,
}

/// What leaving the editor comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaving {
    /// It is finished, or was kept as it was: go.
    Quietly,
    /// It is new and holds nothing: go, and keep nothing of it.
    Forget(String),
    /// It cannot run: ask first, saying what is wrong.
    Ask(Vec<Problem>),
}

/// What discarding what was done puts back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discard {
    /// A new one that was never finished is removed.
    Remove(String),
    /// An old one is kept as it was when the editor opened.
    Restore(Automation),
}

impl Editing {
    /// The editor opened on `automation`; `new` for one just made.
    pub fn open(automation: Automation, new: bool) -> Self {
        Self {
            opened_as: automation.clone(),
            working: automation,
            new,
            past: Vec::new(),
            future: Vec::new(),
            typing: false,
            asked: None,
        }
    }

    pub fn automation(&self) -> &Automation {
        &self.working
    }

    pub fn is_new(&self) -> bool {
        self.new
    }

    /// Whether anything differs from what the editor opened with.
    pub fn touched(&self) -> bool {
        self.working != self.opened_as
    }

    /// Makes the change `change` to a copy and keeps it when it changed
    /// something: what could be undone, and no longer redone.
    fn change(&mut self, typing: bool, change: impl FnOnce(&mut Automation) -> bool) -> bool {
        let mut next = self.working.clone();
        if !change(&mut next) {
            return false;
        }
        if typing && self.typing {
            self.working = next;
        } else {
            self.past.push(std::mem::replace(&mut self.working, next));
            if self.past.len() > MOST_UNDOS {
                self.past.remove(0);
            }
        }
        self.typing = typing;
        self.future.clear();
        true
    }

    /// Puts `step` on the canvas at `at`, and says which it is.
    pub fn insert(&mut self, step: Step, at: (i32, i32)) -> Option<NodeId> {
        let mut made = None;
        self.change(false, |automation| {
            made = Some(automation.insert(step, at));
            true
        });
        made
    }

    pub fn move_to(&mut self, id: NodeId, at: (i32, i32)) -> bool {
        self.change(false, |automation| automation.moved(id, at))
    }

    /// Draws an arrow, or says in what way it cannot go.
    pub fn connect(&mut self, from: NodeId, to: NodeId) -> Result<(), Unjoined> {
        let mut refused = None;
        self.change(false, |automation| match automation.connect(from, to) {
            Ok(()) => true,
            Err(why) => {
                refused = Some(why);
                false
            }
        });
        refused.map_or(Ok(()), Err)
    }

    pub fn disconnect(&mut self, from: NodeId, to: NodeId) -> bool {
        self.change(false, |automation| automation.disconnect(from, to))
    }

    pub fn remove(&mut self, id: NodeId) -> bool {
        self.change(false, |automation| automation.remove(id))
    }

    pub fn set_enabled(&mut self, id: NodeId, enabled: bool) -> bool {
        self.change(false, |automation| automation.set_enabled(id, enabled))
    }

    pub fn configure(&mut self, id: NodeId, step: Step) -> bool {
        self.change(false, |automation| automation.configure(id, step))
    }

    /// A new name; the letters of one name typed in a row are one undo.
    pub fn rename(&mut self, name: &str) -> bool {
        self.change(true, |automation| {
            if automation.name == name {
                return false;
            }
            automation.name = name.to_string();
            true
        })
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Goes back to the copy before the last change.
    pub fn undo(&mut self) -> bool {
        let Some(before) = self.past.pop() else {
            return false;
        };
        self.future
            .push(std::mem::replace(&mut self.working, before));
        self.typing = false;
        true
    }

    /// Does again what was undone.
    pub fn redo(&mut self) -> bool {
        let Some(after) = self.future.pop() else {
            return false;
        };
        self.past.push(std::mem::replace(&mut self.working, after));
        self.typing = false;
        true
    }

    /// What the engine said of `copy`, kept for as long as the copy is
    /// what the editor holds.
    pub fn judged(&mut self, copy: Automation, problems: Vec<Problem>) {
        self.asked = Some((copy, problems));
    }

    /// What keeps the automation from running: what the engine said of
    /// this very copy, with its clashes among the others, else what the
    /// copy says of itself.
    pub fn problems(&self) -> Vec<Problem> {
        match &self.asked {
            Some((copy, problems)) if *copy == self.working => problems.clone(),
            _ => self.working.problems(),
        }
    }

    /// The steps the problems are about.
    pub fn flagged(&self) -> Vec<NodeId> {
        let mut flagged: Vec<NodeId> = self.problems().iter().filter_map(Problem::node).collect();
        flagged.sort_unstable();
        flagged.dedup();
        flagged
    }

    /// What leaving now comes to.
    pub fn leaving(&self) -> Leaving {
        if self.new && !self.touched() {
            return Leaving::Forget(self.working.id.clone());
        }
        let problems = self.problems();
        if problems.is_empty() {
            Leaving::Quietly
        } else {
            Leaving::Ask(problems)
        }
    }

    /// What is done to throw away the changes.
    pub fn discarding(&self) -> Discard {
        if self.new {
            Discard::Remove(self.working.id.clone())
        } else {
            Discard::Restore(self.opened_as.clone())
        }
    }
}

/// What the backend is asked to do to what is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    Keep(Automation),
    /// Remove the automation with this id.
    Forget(String),
}

impl Order {
    fn id(&self) -> &str {
        match self {
            Self::Keep(automation) => &automation.id,
            Self::Forget(id) => id,
        }
    }
}

/// What is being kept: **one request at a time, in order**, and of the
/// copies of one automation that wait, only the latest, so a slow disk
/// never writes an older copy over a newer one and never keeps what was
/// removed after it.
#[derive(Debug, Default)]
pub struct Saves {
    in_flight: bool,
    queued: Vec<Order>,
}

impl Saves {
    /// `order` is to be done: now when nothing is on its way, else after
    /// what is.
    pub fn ask(&mut self, order: Order) -> Option<Order> {
        if !self.in_flight {
            self.in_flight = true;
            return Some(order);
        }
        self.queued
            .retain(|waiting| !(waiting.id() == order.id() && matches!(waiting, Order::Keep(_))));
        self.queued.push(order);
        None
    }

    /// What was on its way is done: the order to send next, if one waited.
    pub fn done(&mut self) -> Option<Order> {
        let next = (!self.queued.is_empty()).then(|| self.queued.remove(0));
        self.in_flight = next.is_some();
        next
    }

    pub fn busy(&self) -> bool {
        self.in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::automation::{Account, Only, Read};
    use snob_core::watch::schedule::Spec;
    use std::time::Duration;

    fn every_day() -> Step {
        Step::Schedule {
            when: Spec {
                every: Some(Duration::from_secs(86_400)),
                ..Spec::default()
            },
        }
    }

    fn finished() -> Editing {
        let mut editing = Editing::open(Automation::new("a1", "Daily"), false);
        let trigger = editing.insert(every_day(), (0, 0)).unwrap();
        let action = editing
            .insert(
                Step::Read {
                    account: Some(Account::own(None)),
                    what: Read::Lists,
                },
                (260, 0),
            )
            .unwrap();
        editing.connect(trigger, action).unwrap();
        editing
    }

    #[test]
    fn every_change_says_it_changed_something_and_a_no_op_says_it_did_not() {
        let mut editing = Editing::open(Automation::new("a1", "Daily"), false);
        let id = editing.insert(every_day(), (0, 0)).unwrap();
        assert!(editing.move_to(id, (10, 20)));
        assert!(!editing.move_to(id, (10, 20)), "nowhere new");
        assert!(editing.set_enabled(id, false));
        assert!(!editing.set_enabled(id, false));
        assert!(!editing.remove(99), "no such step");
        assert!(editing.remove(id));
        assert!(editing.automation().nodes.is_empty());
    }

    #[test]
    fn undo_restores_the_previous_automation_and_redo_does_it_again() {
        let mut editing = finished();
        let whole = editing.automation().clone();
        assert!(editing.remove(1));
        assert_ne!(*editing.automation(), whole);
        assert!(editing.undo());
        assert_eq!(
            *editing.automation(),
            whole,
            "the step and its arrow are back"
        );
        assert!(editing.can_redo());
        assert!(editing.redo());
        assert!(editing.automation().node(1).is_none());
        assert!(!editing.redo(), "nothing more to do again");
        while editing.undo() {}
        assert_eq!(*editing.automation(), Automation::new("a1", "Daily"));
        assert!(!editing.undo());
    }

    #[test]
    fn a_new_change_after_an_undo_forgets_what_could_have_been_redone() {
        let mut editing = finished();
        editing.undo();
        assert!(editing.can_redo());
        assert!(editing.set_enabled(1, false));
        assert!(!editing.can_redo());
    }

    #[test]
    fn the_letters_of_a_name_are_one_undo() {
        let mut editing = Editing::open(Automation::new("a1", "Daily"), false);
        for typed in ["Daily r", "Daily re", "Daily rep"] {
            assert!(editing.rename(typed));
        }
        assert!(!editing.rename("Daily rep"));
        assert!(editing.undo());
        assert_eq!(editing.automation().name, "Daily");
        assert!(!editing.can_undo());
        // Anything else in between starts a new one.
        editing.rename("A");
        editing.insert(every_day(), (0, 0));
        editing.rename("AB");
        assert!(editing.undo() && editing.automation().name == "A");
    }

    #[test]
    fn an_arrow_that_cannot_be_drawn_says_why_and_changes_nothing() {
        let mut editing = finished();
        let before = editing.automation().clone();
        assert_eq!(editing.connect(2, 1), Err(Unjoined::WrongWay));
        assert_eq!(editing.connect(1, 2), Err(Unjoined::Already));
        assert_eq!(editing.connect(1, 9), Err(Unjoined::Missing));
        assert_eq!(*editing.automation(), before);
        assert!(editing.disconnect(1, 2));
        assert!(!editing.disconnect(1, 2));
    }

    #[test]
    fn a_step_keeps_its_stage_when_it_is_configured() {
        let mut editing = finished();
        let filter = editing
            .insert(Step::Filter { only: Only::Public }, (0, 0))
            .unwrap();
        assert!(!editing.configure(filter, every_day()), "not a filter");
        let private = Step::Filter {
            only: Only::Private,
        };
        assert!(editing.configure(filter, private));
    }

    #[test]
    fn leaving_a_finished_automation_asks_nothing_and_an_unfinished_one_says_what_is_wrong() {
        let mut editing = finished();
        assert_eq!(editing.leaving(), Leaving::Quietly);
        editing.disconnect(1, 2);
        let Leaving::Ask(problems) = editing.leaving() else {
            panic!("asked nothing");
        };
        assert_eq!(problems, vec![Problem::NoAction]);
        assert!(editing.flagged().is_empty(), "it is about no step");

        let without_account = Step::Read {
            account: None,
            what: Read::Fans,
        };
        editing.insert(without_account, (0, 0));
        editing.connect(1, 3).unwrap();
        assert_eq!(editing.flagged(), vec![3], "the step without an account");
    }

    #[test]
    fn a_new_automation_with_nothing_in_it_is_forgotten_on_leaving() {
        let mut editing = Editing::open(Automation::new("n1", "New automation"), true);
        assert_eq!(editing.leaving(), Leaving::Forget("n1".to_string()));
        editing.insert(every_day(), (0, 0));
        assert!(matches!(editing.leaving(), Leaving::Ask(_)));
        editing.undo();
        assert_eq!(editing.leaving(), Leaving::Forget("n1".to_string()));
    }

    #[test]
    fn discarding_removes_a_new_automation_and_puts_back_an_old_one() {
        let mut new = Editing::open(Automation::new("n1", "New"), true);
        new.insert(every_day(), (0, 0));
        assert_eq!(new.discarding(), Discard::Remove("n1".to_string()));

        let before = Automation::new("a1", "Daily");
        let mut old = Editing::open(before.clone(), false);
        old.insert(every_day(), (0, 0));
        old.rename("Other");
        assert_eq!(old.discarding(), Discard::Restore(before));
    }

    #[test]
    fn what_the_engine_said_counts_only_while_the_copy_is_the_one_it_judged() {
        let mut editing = finished();
        let clash = Problem::NoName;
        editing.judged(editing.automation().clone(), vec![clash.clone()]);
        assert_eq!(editing.problems(), vec![clash]);
        editing.set_enabled(1, false);
        let alone = editing.automation().problems();
        assert_eq!(
            editing.problems(),
            alone,
            "a newer copy is judged by itself"
        );
    }

    #[test]
    fn saves_go_one_at_a_time_and_the_latest_waiting_goes_last() {
        let one = |name: &str| Order::Keep(Automation::new("a1", name));
        let mut saves = Saves::default();
        assert_eq!(saves.ask(one("1")), Some(one("1")));
        assert_eq!(saves.ask(one("2")), None);
        assert_eq!(saves.ask(one("3")), None, "the second never goes");
        assert!(saves.busy());
        assert_eq!(saves.done(), Some(one("3")));
        assert!(saves.busy());
        assert_eq!(saves.done(), None);
        assert!(!saves.busy());
        assert_eq!(saves.ask(one("4")), Some(one("4")));
    }

    #[test]
    fn a_removal_waits_for_the_save_on_its_way_and_nothing_after_it_is_lost() {
        let keep = |id: &str, name: &str| Order::Keep(Automation::new(id, name));
        let mut saves = Saves::default();
        assert!(saves.ask(keep("a1", "1")).is_some());
        assert_eq!(saves.ask(Order::Forget("a1".to_string())), None);
        assert_eq!(saves.ask(keep("a2", "x")), None);
        assert_eq!(saves.done(), Some(Order::Forget("a1".to_string())));
        assert_eq!(saves.done(), Some(keep("a2", "x")));
        assert_eq!(saves.done(), None);
    }

    #[test]
    fn a_new_step_is_put_in_the_column_of_its_stage_below_the_ones_there() {
        let mut automation = Automation::new("a1", "Daily");
        let first = suggested_place(&automation, Stage::Trigger);
        automation.insert(every_day(), first);
        let second = suggested_place(&automation, Stage::Trigger);
        assert_eq!(first.0, second.0);
        assert!(second.1 > first.1);
        assert!(suggested_place(&automation, Stage::Action).0 > first.0);
        let filter = suggested_place(&automation, Stage::Filter);
        assert!(suggested_place(&automation, Stage::Output).0 > filter.0);
    }
}
