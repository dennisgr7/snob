//! Where the engine runs: a `tokio` runtime of the app's own, with the two
//! workers the command line gives it, on threads that are not the window's.
//!
//! **No engine future is polled on the window's thread, and nothing there
//! reads the keyring or a file of any size.** A view hands work over with
//! [`Runtime::run`] and awaits the answer, which arrives through a channel
//! that any executor can wait on.

use std::future::Future;

use tokio::sync::{oneshot, watch};

/// The engine's runtime. Dropping it stops what runs on it.
pub struct Runtime {
    runtime: tokio::runtime::Runtime,
}

impl Runtime {
    /// Starts it: two workers, so a blocked task cannot starve the one that
    /// stops a run, as in the command line's `main`.
    pub fn start() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("snob-engine")
            .enable_all()
            .build()?;
        Ok(Self { runtime })
    }

    /// Runs `work` on the runtime and answers with what it returned. The
    /// future is made and polled on a thread of the runtime's own, so it
    /// need not be `Send`: the engine's are written for a command that runs
    /// one of them to its end.
    ///
    /// A receiver that is dropped lets the work finish: what an engine call
    /// started is not abandoned halfway; a run is stopped by its token.
    pub fn run<T, Fut>(&self, work: impl FnOnce() -> Fut + Send + 'static) -> oneshot::Receiver<T>
    where
        T: Send + 'static,
        Fut: Future<Output = T>,
    {
        let (answer, answered) = oneshot::channel();
        let handle = self.runtime.handle().clone();
        self.runtime.spawn_blocking(move || {
            let _ = answer.send(handle.block_on(work()));
        });
        answered
    }

    /// Runs `work` to its end on the calling thread. **For the start and the
    /// end of the process only**, before there is a window and after it is
    /// gone.
    pub fn block_on<T>(&self, work: impl Future<Output = T>) -> T {
        self.runtime.block_on(work)
    }

    /// The runtime itself, for what lives as long as the app: a listener.
    pub fn handle(&self) -> &tokio::runtime::Handle {
        self.runtime.handle()
    }
}

/// The newest value of something that changes while it is watched: a
/// download's progress. **A reader that falls behind sees the latest value
/// and none of those between**, so a slow frame never queues work.
pub struct Latest<T>(watch::Sender<T>);

impl<T> Latest<T> {
    pub fn new(first: T) -> (Self, watch::Receiver<T>) {
        let (said, heard) = watch::channel(first);
        (Self(said), heard)
    }

    pub fn say(&self, value: T) {
        // Nobody watching any more is no failure.
        let _ = self.0.send(value);
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    #[test]
    fn work_runs_off_the_calling_thread_and_its_answer_comes_back() {
        let runtime = Runtime::start().unwrap();
        let here = std::thread::current().id();
        let answered = runtime.run(move || async move {
            // Not `Send`, as an engine future may be.
            let local = Rc::new(41);
            tokio::task::yield_now().await;
            (*local + 1, std::thread::current().id() != here)
        });
        assert_eq!(answered.blocking_recv().unwrap(), (42, true));
    }

    #[test]
    fn a_reader_that_falls_behind_sees_only_the_latest() {
        let (progress, mut watched) = Latest::new(0u8);
        for percent in 1..=100 {
            progress.say(percent);
        }
        assert!(watched.has_changed().unwrap());
        assert_eq!(*watched.borrow_and_update(), 100);
        assert!(!watched.has_changed().unwrap());
    }
}
