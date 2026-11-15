//! What the field for a session ID says about what is in it, before anything
//! is sent: **the check `snob login --paste` makes**, on the same code
//! (`Session::from_sessionid`), so what the button accepts is what the
//! engine accepts.

use std::time::Duration;

use snob_core::session::{Session, SessionError, SessionOrigin};

/// How long after the last key the field is judged; a paste is judged at
/// once.
pub const JUDGED_AFTER: Duration = Duration::from_millis(400);

/// What is in the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// Nothing: the button is off, and nothing is said.
    Empty,
    /// Something that cannot be a session ID: the button is off, and the
    /// field says what to copy.
    Invalid,
    /// It has the shape of one: the button is on.
    Valid,
}

/// A User-Agent that passes the session's own check, so only the ID is
/// judged here: the real one is the browser's, picked when signing in.
const ANY_BROWSER: &str = "Mozilla/5.0";

/// Judges what is in the field.
pub fn judge(text: &str) -> Field {
    match Session::from_sessionid(text, ANY_BROWSER, SessionOrigin::Paste) {
        Ok(_) => Field::Valid,
        Err(SessionError::EmptySessionId) => Field::Empty,
        Err(_) => Field::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_is_judged_as_the_command_line_judges_a_paste() {
        assert_eq!(judge(""), Field::Empty);
        assert_eq!(judge("   \n"), Field::Empty);
        assert_eq!(judge("42%3AAbCdEfGh%3A20"), Field::Valid);
        assert_eq!(judge("  42%3AAbCdEfGh%3A20\n"), Field::Valid);
        for wrong in ["hello", "sessionid", "abc%3A42"] {
            assert_eq!(judge(wrong), Field::Invalid, "{wrong}");
        }
    }
}
