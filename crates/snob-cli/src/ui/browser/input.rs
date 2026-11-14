//! Keys for the interactive views, read through `crossterm` rather than
//! `console::Term::read_key`.
//!
//! Every modified arrow is `ESC [ 1 ; <mod> <letter>`, so Shift+Left arrives as
//! `ESC[1;2D` and is read as: an unrecognized sequence (ignored), then `2`
//! (ignored), then **`D`**, the download key.
//!
//! **Without DEC private mode 2004, pasted text runs as commands.**
//!
//! - **A wait that ends by itself.** `read_key` blocks with no timeout and
//!   there is no `poll` beside it.
//! - **Resize.** On Unix it is `SIGWINCH`, folded into the same `mio` poll
//!   tokio already runs.
//! - **Raw mode that lasts.** `read_single_key` calls `tcsetattr` twice around
//!   every keypress, so between two keys the terminal is back in canonical mode
//!   with `ECHO` on: anything typed while a browser is off fetching megabytes
//!   from the CDN would echo into the frame.
//!
//! **Focus is taken, and only to do less.**
//!
//! What is **not** taken: mouse tracking and the kitty keyboard protocol.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use snob_ig::pace::CancelToken;

/// How long the browser waits before waking up to look at the terminal size.
pub const TICK: Duration = Duration::from_millis(150);

/// How long a view whose terminal has lost the focus waits before waking up on
/// its own: seven times in a second become one every two.
pub const UNFOCUSED_TICK: Duration = Duration::from_secs(2);

/// Whether the terminal has the focus, as it last said.
static FOCUSED: AtomicBool = AtomicBool::new(true);

/// Whether the terminal has the focus, as far as anybody can tell: `true`
/// until it says otherwise.
pub fn focused() -> bool {
    FOCUSED.load(Ordering::Relaxed)
}

/// Forgets what the terminal last said about its focus, when its reports are
/// turned off: taken as focused again, which is what a command in a terminal
/// is until the next view hears otherwise.
pub fn focus_unknown() {
    FOCUSED.store(true, Ordering::Relaxed);
}

/// How long a view waits for something to happen before going round again:
/// [`TICK`] while it is being looked at, [`UNFOCUSED_TICK`] while not.
pub fn tick() -> Duration {
    if focused() { TICK } else { UNFOCUSED_TICK }
}

/// What the browser understands, resolved from a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    Open,
    /// One level up, in a view that has levels: from a folder back to the tray
    /// in the highlights browser and the account list, from a sub-view back to
    /// the card in the profile, and out of the account picker.
    Back,
    Download,
    /// The account picker: which account this view acts as.
    Accounts,
    /// The page of what is under the cursor, on instagram.com, in the browser
    /// the system opens addresses with.
    Web,
    /// The picture under the cursor, drawn in the terminal (`ui::preview`),
    /// in a build that draws them; nothing in one that does not.
    Preview,
    /// The keys of the view on screen, in a box over it (`ui::help`).
    Help,
    Redraw,
    Quit,
    Interrupt,
    None,
}

/// What came back from the terminal, before any binding is applied.
pub enum Raw {
    /// A key going down.
    Key(KeyEvent),
    /// A paste, whole, thanks to bracketed paste.
    Paste(String),
    Resized,
    Tick,
}

/// Waits up to `timeout` for something to happen, and hands it over unbound.
///
/// **A stop the system asked for arrives as a Ctrl+C.**
pub fn read(timeout: Duration) -> std::io::Result<Raw> {
    if crate::interrupt::asked_by_the_system() {
        return Ok(Raw::Key(system_stop()));
    }
    if !event::poll(timeout)? {
        return Ok(Raw::Tick);
    }
    Ok(match event::read()? {
        Event::Resize(..) => Raw::Resized,
        // Read as a resize, which every view answers by drawing: what changed
        // while nobody looked is put on screen as they come back.
        Event::FocusGained => {
            FOCUSED.store(true, Ordering::Relaxed);
            crate::owner::attended(true);
            Raw::Resized
        }
        Event::FocusLost => {
            FOCUSED.store(false, Ordering::Relaxed);
            crate::owner::attended(false);
            Raw::Tick
        }
        Event::Paste(text) => Raw::Paste(text),
        // `KeyEventKind` matters on Windows, where the console reports the
        // release of a key as well as the press.
        Event::Key(key) if key.kind == KeyEventKind::Press => Raw::Key(key),
        _ => Raw::Tick,
    })
}

/// The key a stop the system asked for is read as. See [`read`].
fn system_stop() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

/// Waits up to `timeout` for a key, and binds it.
pub fn next(timeout: Duration) -> std::io::Result<Option<Action>> {
    Ok(bound(read(timeout)?))
}

/// What a view with no text mode makes of what was read: [`next`]'s answer,
/// for a view that waits some other way than through it (one whose wait
/// also fetches a picture, `preview::Gallery::read`).
pub fn bound(raw: Raw) -> Option<Action> {
    match raw {
        Raw::Tick | Raw::Resized => None,
        // Swallowed whole, and that is the entire fix.
        Raw::Paste(_) => Some(Action::None),
        Raw::Key(key) => Some(action_of(key)),
    }
}

/// The error a view leaves with when the keyboard cannot be read.
pub fn unreadable(e: std::io::Error) -> anyhow::Error {
    crate::exit::ExitError::new(
        crate::exit::ExitCode::Error,
        format!("the keyboard could not be read: {e}"),
    )
    .into()
}

/// Binds one key.
pub fn action_of(key: KeyEvent) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let plain = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('c') if ctrl => Action::Interrupt,
        // The universal "what is on screen is wrong, draw it again".
        KeyCode::Char('l') if ctrl => Action::Redraw,
        KeyCode::Up | KeyCode::Char('k') if plain => Action::Up,
        KeyCode::Down | KeyCode::Char('j') if plain => Action::Down,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Home | KeyCode::Char('g') if plain => Action::First,
        KeyCode::End | KeyCode::Char('G') if plain => Action::Last,
        KeyCode::Enter => Action::Open,
        // Right alongside Enter and Left alongside Backspace: in a browser with
        // levels the arrows walk into and out of a folder, the way every
        // two-pane file view binds them.
        KeyCode::Right | KeyCode::Char('l') if plain => Action::Open,
        KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') if plain => Action::Back,
        KeyCode::Char('d') | KeyCode::Char('D') if plain => Action::Download,
        KeyCode::Char('a') if plain => Action::Accounts,
        KeyCode::Char('w') if plain => Action::Web,
        KeyCode::Char('p') if plain => Action::Preview,
        KeyCode::Char('?') if plain => Action::Help,
        KeyCode::Esc | KeyCode::Char('q') if plain => Action::Quit,
        _ => Action::None,
    }
}

/// Whether the keyboard ended a fetch early, and what the person meant by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The future ran to its own end.
    No,
    /// `q`: stop what is in flight and leave the browser cleanly.
    Quit,
    /// Ctrl+C: stop and leave saying so.
    Interrupt,
}

impl Stopped {
    /// The way out of the browser loop, when this asks for one.
    pub fn leave(self) -> Option<crate::exit::ExitCode> {
        match self {
            Stopped::No => None,
            Stopped::Quit => Some(crate::exit::ExitCode::Ok),
            Stopped::Interrupt => Some(crate::exit::ExitCode::Interrupted),
        }
    }
}

/// How often the fetch watcher wakes to look for a stop key.
const WATCH: Duration = Duration::from_millis(50);

/// Runs a fetch with the keyboard still alive.
pub async fn watching_cancel_keys<T>(
    cancel: &CancelToken,
    fut: impl std::future::Future<Output = T>,
) -> (T, Stopped) {
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let done = done.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || {
            while !done.load(Ordering::Relaxed) {
                match read(WATCH) {
                    // Bound by key rather than through `action_of`: Esc also
                    // maps to `Action::Quit` there, and Esc is not a stop —
                    // see `Stopped::Quit`.
                    Ok(Raw::Key(key)) => {
                        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                        if key.code == KeyCode::Char('c') && ctrl {
                            cancel.cancel();
                            return Stopped::Interrupt;
                        }
                        if key.code == KeyCode::Char('q') && key.modifiers.is_empty() {
                            cancel.cancel();
                            return Stopped::Quit;
                        }
                    }
                    Ok(_) => {}
                    // A keyboard that cannot be read is the loop's own error
                    // to hit; the watcher just stands down.
                    Err(_) => return Stopped::No,
                }
            }
            Stopped::No
        })
    };

    let out = fut.await;
    done.store(true, Ordering::Relaxed);
    let stopped = watcher.await.unwrap_or(Stopped::No);
    (out, stopped)
}

/// How far Page Up and Page Down move, given how many rows the list has.
pub fn page(height: usize) -> usize {
    height.saturating_sub(1).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    /// The defect this module exists for. Under `console`, `ESC[1;2D` leaves `2D`
    /// in the queue and the `D` reaches the browser as a download. Shift+Left
    /// is plain-with-shift so it lands on `Back`, which is harmless; Ctrl+Left
    /// is somebody's terminal binding and stays untouched.
    #[test]
    fn a_shifted_arrow_is_not_a_download() {
        assert_eq!(
            action_of(key(KeyCode::Left, KeyModifiers::SHIFT)),
            Action::Back
        );
        assert_eq!(
            action_of(key(KeyCode::Left, KeyModifiers::CONTROL)),
            Action::None
        );
    }

    /// The keys a browser with levels adds: out of a folder, and into one.
    #[test]
    fn left_goes_back_and_right_opens() {
        assert_eq!(
            action_of(key(KeyCode::Left, KeyModifiers::NONE)),
            Action::Back
        );
        assert_eq!(
            action_of(key(KeyCode::Backspace, KeyModifiers::NONE)),
            Action::Back
        );
        assert_eq!(
            action_of(key(KeyCode::Right, KeyModifiers::NONE)),
            Action::Open
        );
        // Esc stays "leave the program", not "leave the folder".
        assert_eq!(
            action_of(key(KeyCode::Esc, KeyModifiers::NONE)),
            Action::Quit
        );
    }

    /// The home row's arrows: `h` walks out and `l` walks in, as Left and
    /// Right do, and Ctrl+L is still the redraw and not an open.
    #[test]
    fn h_and_l_walk_out_and_in() {
        assert_eq!(
            action_of(key(KeyCode::Char('h'), KeyModifiers::NONE)),
            Action::Back
        );
        assert_eq!(
            action_of(key(KeyCode::Char('l'), KeyModifiers::NONE)),
            Action::Open
        );
        assert_eq!(
            action_of(key(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Action::Redraw
        );
        assert_eq!(
            action_of(key(KeyCode::Char('h'), KeyModifiers::CONTROL)),
            Action::None
        );
    }

    /// `?` asks for the keys, which most keyboards type with shift, and `w`
    /// for the page on the web.
    #[test]
    fn a_question_mark_asks_for_the_keys_and_w_for_the_web() {
        assert_eq!(
            action_of(key(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            Action::Help
        );
        assert_eq!(
            action_of(key(KeyCode::Char('?'), KeyModifiers::NONE)),
            Action::Help
        );
        assert_eq!(
            action_of(key(KeyCode::Char('w'), KeyModifiers::NONE)),
            Action::Web
        );
        assert_eq!(
            action_of(key(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            Action::None
        );
    }

    #[test]
    fn a_modified_letter_is_not_the_letter() {
        assert_eq!(
            action_of(key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Action::None
        );
        assert_eq!(
            action_of(key(KeyCode::Char('q'), KeyModifiers::ALT)),
            Action::None
        );
    }

    /// A view nobody is looking at wakes seldom, and one being looked at as
    /// it always did.
    #[test]
    fn a_view_out_of_focus_wakes_seldom() {
        assert_eq!(tick(), TICK, "focused until the terminal says otherwise");
        FOCUSED.store(false, Ordering::Relaxed);
        assert_eq!(tick(), UNFOCUSED_TICK);
        FOCUSED.store(true, Ordering::Relaxed);
        assert_eq!(tick(), TICK);
    }

    /// A stop the system asked for is read as the key every browser already
    /// leaves on.
    #[test]
    fn a_stop_the_system_asked_for_reads_as_ctrl_c() {
        assert_eq!(action_of(system_stop()), Action::Interrupt);
    }

    #[test]
    fn ctrl_c_interrupts_and_ctrl_l_redraws() {
        assert_eq!(
            action_of(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Interrupt
        );
        assert_eq!(
            action_of(key(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            Action::Redraw
        );
    }

    #[test]
    fn the_plain_keys_still_work() {
        assert_eq!(
            action_of(key(KeyCode::Down, KeyModifiers::NONE)),
            Action::Down
        );
        assert_eq!(
            action_of(key(KeyCode::Char('j'), KeyModifiers::NONE)),
            Action::Down
        );
        assert_eq!(
            action_of(key(KeyCode::Enter, KeyModifiers::NONE)),
            Action::Open
        );
        assert_eq!(
            action_of(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Action::Quit
        );
        assert_eq!(
            action_of(key(KeyCode::Char('a'), KeyModifiers::NONE)),
            Action::Accounts
        );
    }

    /// Shift is what makes a capital letter, so it cannot be the modifier that
    /// disqualifies one -- `D` has to keep working.
    #[test]
    fn a_capital_letter_is_still_its_letter() {
        assert_eq!(
            action_of(key(KeyCode::Char('D'), KeyModifiers::SHIFT)),
            Action::Download
        );
        assert_eq!(
            action_of(key(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Action::Last
        );
    }

    /// A keyboard that cannot be read ends the view as an error, saying so.
    #[test]
    fn an_unreadable_keyboard_is_an_error_exit() {
        let error = unreadable(std::io::Error::other("x"));
        let exit = error
            .downcast_ref::<crate::exit::ExitError>()
            .expect("an ExitError");
        assert_eq!(exit.code, crate::exit::ExitCode::Error);
        assert_eq!(error.to_string(), "the keyboard could not be read: x");
    }

    #[test]
    fn a_page_leaves_one_row_of_overlap() {
        assert_eq!(page(20), 19);
        // And never zero, or Page Down would not move.
        assert_eq!(page(1), 1);
    }
}
