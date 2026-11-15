//! A system a test drives: its settings are whatever the test says, and a
//! sleep, a shutdown or a lost network is an event handed over, with nothing
//! done to the machine. What the app asked of it is kept to be read back.
//! **Only a `testing` build has it.**

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use super::{Event, Notification, Settings, System};

#[derive(Default)]
struct State {
    settings: Settings,
    online: Option<bool>,
    events: Option<mpsc::Sender<Event>>,
    /// What happened before anybody listened, handed over when they do.
    early: Vec<Event>,
    notified: Vec<Notification>,
    opened: Vec<PathBuf>,
    clipboard: Option<String>,
    holds: Vec<String>,
}

/// The simulated system. Clones are the same system.
#[derive(Clone, Default)]
pub struct Simulated {
    state: Arc<Mutex<State>>,
    /// How many shutdown holds are in place now.
    held: Arc<AtomicUsize>,
    attended: Arc<AtomicBool>,
}

impl Simulated {
    pub fn new(settings: Settings) -> Self {
        let system = Self::default();
        system.state().settings = settings;
        system.attended.store(true, Ordering::SeqCst);
        system
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Has the system say `event`, as the real one would: a change of
    /// settings or of the network is also what it answers from then on.
    pub fn say(&self, event: Event) {
        let mut state = self.state();
        match &event {
            Event::Settings(settings) => state.settings = settings.clone(),
            Event::Online(online) => state.online = Some(*online),
            _ => {}
        }
        match &state.events {
            Some(events) => {
                let _ = events.send(event);
            }
            None => state.early.push(event),
        }
    }

    /// Changes one thing about the settings and says so.
    pub fn change(&self, change: impl FnOnce(&mut Settings)) {
        let mut settings = self.state().settings.clone();
        change(&mut settings);
        self.say(Event::Settings(settings));
    }

    /// The notifications shown so far, oldest first.
    pub fn notified(&self) -> Vec<Notification> {
        self.state().notified.clone()
    }

    /// The folders opened so far.
    pub fn opened(&self) -> Vec<PathBuf> {
        self.state().opened.clone()
    }

    /// What is on the clipboard.
    pub fn clipboard(&self) -> Option<String> {
        self.state().clipboard.clone()
    }

    /// Puts `text` on the clipboard, as the person copying it elsewhere.
    pub fn put_on_clipboard(&self, text: &str) {
        self.state().clipboard = Some(text.to_string());
    }

    /// Every reason a shutdown was held for, and how many holds are in place.
    pub fn holds(&self) -> (Vec<String>, usize) {
        (self.state().holds.clone(), self.held.load(Ordering::SeqCst))
    }

    /// Whether the app last said somebody is looking.
    pub fn is_attended(&self) -> bool {
        self.attended.load(Ordering::SeqCst)
    }
}

/// A hold on a shutdown, in place until dropped.
struct Hold(Arc<AtomicUsize>);

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl System for Simulated {
    fn settings(&self) -> Settings {
        self.state().settings.clone()
    }

    fn online(&self) -> Option<bool> {
        self.state().online
    }

    fn notify(&self, notification: Notification) {
        self.state().notified.push(notification);
    }

    fn open_folder(&self, folder: &Path) {
        self.state().opened.push(folder.to_path_buf());
    }

    fn copy(&self, text: &str) {
        self.state().clipboard = Some(text.to_string());
    }

    fn pasted(&self) -> Option<String> {
        self.state().clipboard.clone()
    }

    fn hold_shutdown(&self, why: &str) -> Box<dyn Send> {
        self.state().holds.push(why.to_string());
        self.held.fetch_add(1, Ordering::SeqCst);
        Box::new(Hold(Arc::clone(&self.held)))
    }

    fn attended(&self, attended: bool) {
        self.attended.store(attended, Ordering::SeqCst);
    }

    fn events(&self) -> mpsc::Receiver<Event> {
        let (said, heard) = mpsc::channel();
        let mut state = self.state();
        for event in state.early.drain(..) {
            let _ = said.send(event);
        }
        state.events = Some(said);
        heard
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::system::Notice;

    #[test]
    fn what_the_system_says_is_heard_in_order_early_or_late() {
        let system = Simulated::new(Settings::default());
        system.say(Event::Sleeping);
        let heard = system.events();
        system.say(Event::Woke {
            slept: Duration::from_secs(7200),
        });
        system.say(Event::ShuttingDown);

        assert_eq!(
            heard.try_iter().collect::<Vec<_>>(),
            [
                Event::Sleeping,
                Event::Woke {
                    slept: Duration::from_secs(7200)
                },
                Event::ShuttingDown,
            ]
        );
    }

    #[test]
    fn a_change_said_is_what_the_system_answers_from_then_on() {
        let system = Simulated::new(Settings::default());
        let heard = system.events();
        assert_eq!(system.online(), None);

        system.change(|settings| settings.dark = true);
        system.say(Event::Online(false));

        assert!(system.settings().dark);
        assert_eq!(system.online(), Some(false));
        assert_eq!(heard.try_iter().count(), 2);
    }

    #[test]
    fn what_the_app_asked_for_is_read_back() {
        let system = Simulated::new(Settings::default());
        system.notify(Notification {
            about: Notice::UpdateReady,
            title: "snob".into(),
            body: "A new version is ready.".into(),
        });
        system.open_folder(Path::new("/data/logs"));
        system.copy("a report");
        system.attended(false);

        assert_eq!(system.notified().len(), 1);
        assert_eq!(system.opened(), [PathBuf::from("/data/logs")]);
        assert_eq!(system.pasted().as_deref(), Some("a report"));
        assert!(!system.is_attended());

        let hold = system.hold_shutdown("saving");
        assert_eq!(system.holds(), (vec!["saving".to_string()], 1));
        drop(hold);
        assert_eq!(system.holds().1, 0, "released as soon as it is dropped");
    }
}
