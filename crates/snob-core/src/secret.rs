//! A string that happens to be a credential.
//!
//! It cannot be printed by accident (`Debug` says `<hidden>`, so a struct
//! holding one can derive `Debug`), and it clears itself when dropped. Reading
//! it is spelled [`Secret::expose`], so every place a credential is looked at
//! is one grep.
//!
//! **What this does not do**: it clears only the buffer it still owns. A value
//! is built once, whole, and never appended to, so no outgrown buffer is left
//! behind; a copy made elsewhere (an HTTP header) is beyond reach.

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Serialized as the bare string it wraps.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The plaintext.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Never the value.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<hidden>")
    }
}

/// In constant time: `==` on two credentials must not say where they differ.
impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        crate::watch::sign::same(self.0.as_bytes(), other.0.as_bytes())
    }
}

impl Eq for Secret {}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_value() {
        let secret = Secret::new("71234567890%3AAbCdEfGhIjKl%3A20");
        assert_eq!(format!("{secret:?}"), "<hidden>");
        assert!(!format!("{secret:?}").contains("AbCdEfGh"));
    }

    /// Even nested inside something else, which is how it actually appears.
    #[test]
    fn debug_stays_hidden_inside_a_container() {
        let held = Some(Secret::new("AbCdEfGh"));
        assert!(!format!("{held:?}").contains("AbCdEfGh"));

        let pair = vec![Secret::new("one"), Secret::new("two")];
        assert_eq!(format!("{pair:?}"), "[<hidden>, <hidden>]");
    }

    /// The stored format must not change: it serializes as the bare string,
    /// so a session written before this type existed still reads back.
    #[test]
    fn it_serializes_as_the_plain_string() {
        let secret = Secret::new("value");
        assert_eq!(serde_json::to_string(&secret).unwrap(), r#""value""#);

        let back: Secret = serde_json::from_str(r#""value""#).unwrap();
        assert_eq!(back.expose(), "value");
    }

    #[test]
    fn the_value_is_still_readable_when_asked_for() {
        let secret = Secret::from("abc");
        assert_eq!(secret.expose(), "abc");
        assert!(!secret.is_empty());
        assert!(Secret::new("").is_empty());
    }

    #[test]
    fn two_secrets_are_equal_when_their_values_are() {
        assert_eq!(Secret::new("abc"), Secret::from("abc"));
        assert_ne!(Secret::new("abc"), Secret::new("abd"));
        assert_ne!(Secret::new("abc"), Secret::new("ab"));
    }
}
