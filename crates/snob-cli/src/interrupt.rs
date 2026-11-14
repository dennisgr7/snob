//! Ctrl+C handling, and the other ways a system asks a program to stop.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use snob_ig::pace::CancelToken;

/// The process's cancellation token. See [`install`].
static TOKEN: OnceLock<CancelToken> = OnceLock::new();

/// Set when the system, rather than a person, asked the process to stop.
static BY_THE_SYSTEM: AtomicBool = AtomicBool::new(false);

/// Set by [`stop_when_stdin_closes`].
static ON_STDIN_EOF: AtomicBool = AtomicBool::new(false);

/// Set by [`before_a_forced_exit`].
static BEFORE_EXIT: OnceLock<fn()> = OnceLock::new();

/// What to undo before the second Ctrl+C ends the process at once, where no
/// destructor runs: the command line puts its terminal back. Set once.
pub fn before_a_forced_exit(undo: fn()) {
    let _ = BEFORE_EXIT.set(undo);
}

/// Makes standard input reaching its end one more way of asking the process to
/// stop, taken as the system's requests are (no message, exit 130).
///
/// **For a parent that cannot send a signal.**
pub fn stop_when_stdin_closes() {
    ON_STDIN_EOF.store(true, Ordering::SeqCst);
}

/// The parent [`stop_when_parent_exits`] opened, until the handler takes it.
static PARENT: std::sync::Mutex<Option<crate::parent::Parent>> = std::sync::Mutex::new(None);

/// Makes the parent process going one more way of asking the process to stop,
/// taken as the system's requests are (no message, exit 130), **when it is
/// worth watching** (`parent::worth_watching`: a program drives this process,
/// and nobody detached it on purpose).
pub fn stop_when_parent_exits(detached_command: bool) {
    if !crate::parent::worth_watching(crate::parent::situation(detached_command)) {
        return;
    }
    let parent = crate::parent::arm();
    *PARENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(parent);
}

/// Says so, once, when the parent [`stop_when_parent_exits`] opened is gone.
fn parent_gone() -> Option<tokio::sync::mpsc::Receiver<()>> {
    let parent = PARENT.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    let (gone, heard) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || {
        parent.wait();
        let _ = gone.blocking_send(());
    });
    Some(heard)
}

/// Says so, once, when standard input has reached its end.
fn stdin_closed() -> tokio::sync::mpsc::Receiver<()> {
    let (closed, heard) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || {
        wait_for_the_end_of_stdin();
        let _ = closed.blocking_send(());
    });
    heard
}

/// Reads standard input until it ends: a closed pipe, the end of a file, or
/// anything that will not read.
#[cfg(unix)]
fn wait_for_the_end_of_stdin() {
    use std::io::Read;
    let mut stdin = std::io::stdin().lock();
    let mut sink = [0u8; 256];
    while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
}

/// The same on Windows, **without a read left waiting**.
#[cfg(windows)]
fn wait_for_the_end_of_stdin() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    // Asked once a quarter of a second: the parent closing the pipe is the
    // run stopping, and a quarter of a second is nothing beside the request
    // in flight it then waits for.
    const EVERY: std::time::Duration = std::time::Duration::from_millis(250);

    let handle = std::io::stdin().as_raw_handle();
    let mut sink = [0u8; 256];
    loop {
        let mut available: u32 = 0;
        // SAFETY: `handle` is this process's standard input, which stays open
        // for the life of the process; the only out-pointer is to a local, and
        // a null buffer of size zero asks for the count alone.
        let peeked = unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if peeked == 0 {
            return;
        }
        if available == 0 {
            std::thread::sleep(EVERY);
            continue;
        }
        let mut read: u32 = 0;
        let wanted = available.min(sink.len() as u32);
        // SAFETY: the same handle; `sink` is a local buffer at least `wanted`
        // bytes long, `read` a local, and no overlapped structure is passed,
        // so the read is synchronous and over when this returns. It does not
        // wait: the pipe already holds `wanted` bytes.
        let ok = unsafe {
            ReadFile(
                handle,
                sink.as_mut_ptr(),
                wanted,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            return;
        }
    }
}

/// Whether this run was canceled: a Ctrl+C the handler heard, a stop the
/// system asked for, or a stop key in the interactive browser.
pub fn interrupted() -> bool {
    TOKEN.get().is_some_and(CancelToken::is_canceled)
}

/// Whether the system asked the process to stop: a service manager, the
/// terminal going away, the session ending.
pub fn asked_by_the_system() -> bool {
    BY_THE_SYSTEM.load(Ordering::SeqCst)
}

/// Who asked the process to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// A person, at the keyboard: Ctrl+C.
    Person,
    /// The system: a service manager stopping the unit, the terminal going
    /// away, the session ending, the machine shutting down.
    System,
}

/// Installs the handler and returns the cancellation token.
///
/// The double-press pattern is required because `tokio::signal::ctrl_c()`
/// **permanently** disables the process default: from the first call onwards, a
/// Ctrl+C no longer kills the program.
///
/// **The system's requests to stop take the same road as Ctrl+C.**
///
/// **Installed once per process, not once per call.**
pub fn install() -> CancelToken {
    TOKEN
        .get_or_init(|| {
            let token = CancelToken::default();
            let copy = token.clone();

            tokio::spawn(async move {
                let mut asked = Requests::listen();
                let Some(first) = asked.next().await else {
                    return;
                };
                if first == Asked::Person {
                    eprintln!(
                        "\nStopping and saving what has been fetched... (Ctrl+C again to quit now)"
                    );
                } else {
                    BY_THE_SYSTEM.store(true, Ordering::SeqCst);
                }
                copy.cancel();

                if let Some(second) = asked.next().await {
                    if second == Asked::Person {
                        eprintln!("\nForced exit.");
                    }
                    // Nothing below this runs a destructor, so a cursor hidden
                    // by the login menu would stay hidden for the rest of the
                    // user's shell session.
                    if let Some(undo) = BEFORE_EXIT.get() {
                        undo();
                    }
                    std::process::exit(130);
                }
            });

            token
        })
        .clone()
}

/// Every way the process is asked to stop, listened to once for its whole life.
struct Requests {
    /// Standard input closing, when [`stop_when_stdin_closes`] asked for it.
    stdin: Option<tokio::sync::mpsc::Receiver<()>>,
    /// The parent going, when [`stop_when_parent_exits`] found it worth
    /// watching.
    parent: Option<tokio::sync::mpsc::Receiver<()>>,
    #[cfg(unix)]
    terminate: Option<tokio::signal::unix::Signal>,
    #[cfg(unix)]
    hangup: Option<tokio::signal::unix::Signal>,
    #[cfg(windows)]
    close: Option<tokio::signal::windows::CtrlClose>,
    #[cfg(windows)]
    logoff: Option<tokio::signal::windows::CtrlLogoff>,
    #[cfg(windows)]
    shutdown: Option<tokio::signal::windows::CtrlShutdown>,
}

/// Waits for `$slot`'s next request, or for ever when it is not listened to
/// or has stopped delivering: one that has stopped must not end every wait at
/// once and spin the loop that waits on it.
macro_rules! next_of {
    ($slot:expr) => {
        async {
            if let Some(slot) = $slot.as_mut()
                && slot.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        }
    };
}

impl Requests {
    fn listen() -> Self {
        let stdin = ON_STDIN_EOF.load(Ordering::SeqCst).then(stdin_closed);
        let parent = parent_gone();
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            // `nohup` left `SIGHUP` ignored: somebody asked for this process to
            // outlive its terminal, and listening would undo it.
            let hangup = if crate::parent::hangup_ignored() {
                None
            } else {
                signal(SignalKind::hangup()).ok()
            };
            Self {
                stdin,
                parent,
                terminate: signal(SignalKind::terminate()).ok(),
                hangup,
            }
        }
        #[cfg(windows)]
        {
            use tokio::signal::windows::{ctrl_close, ctrl_logoff, ctrl_shutdown};
            Self {
                stdin,
                parent,
                close: ctrl_close().ok(),
                logoff: ctrl_logoff().ok(),
                shutdown: ctrl_shutdown().ok(),
            }
        }
    }

    /// The next request, or `None` once Ctrl+C can no longer be heard.
    async fn next(&mut self) -> Option<Asked> {
        #[cfg(unix)]
        {
            tokio::select! {
                heard = tokio::signal::ctrl_c() => heard.ok().map(|()| Asked::Person),
                () = next_of!(self.stdin) => Some(Asked::System),
                () = next_of!(self.parent) => Some(Asked::System),
                () = next_of!(self.terminate) => Some(Asked::System),
                () = next_of!(self.hangup) => Some(Asked::System),
            }
        }
        #[cfg(windows)]
        {
            tokio::select! {
                heard = tokio::signal::ctrl_c() => heard.ok().map(|()| Asked::Person),
                () = next_of!(self.stdin) => Some(Asked::System),
                () = next_of!(self.parent) => Some(Asked::System),
                () = next_of!(self.close) => Some(Asked::System),
                () = next_of!(self.logoff) => Some(Asked::System),
                () = next_of!(self.shutdown) => Some(Asked::System),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two runs in one process share one token, so a Ctrl+C reaches whatever is
    /// running now rather than a token from a tick that finished hours ago.
    #[tokio::test]
    async fn installing_twice_hands_back_the_same_token() {
        let first = install();
        let second = install();

        assert!(!second.is_canceled());
        first.cancel();
        assert!(
            second.is_canceled(),
            "the second caller must be watching the token the handler cancels"
        );
    }
}
