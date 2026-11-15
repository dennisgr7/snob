//! Filtering of account lists.

use std::borrow::Cow;
use std::collections::HashSet;

use crate::model::User;

/// URL fragments that identify Instagram's default avatar: the only way to
/// tell that someone has no picture, since the API hands the generic image's
/// URL back as if it were theirs. When Instagram changes it, add the new one.
pub const DEFAULT_AVATAR_HASHES: [&str; 2] = [
    "44884218_345707102882519_2446069589734326272_n",
    "464760996_1254146839119862_3605321457742435801_n",
];

/// Attributes an account can be filtered by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Attribute {
    Verified,
    Private,
    NoPfp,
}

impl Attribute {
    /// Whether an account has it. **`false` when the data is unknown**:
    /// hiding someone over an attribute they may not have is worse than
    /// showing them needlessly.
    pub fn matches(self, u: &User) -> bool {
        match self {
            Self::Verified => u.is_verified.unwrap_or(false),
            Self::Private => u.is_private.unwrap_or(false),
            Self::NoPfp => u
                .pfp_url
                .as_deref()
                .is_some_and(|url| DEFAULT_AVATAR_HASHES.iter().any(|d| url.contains(d))),
        }
    }
}

/// What to let through.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Anyone matching any of these is dropped.
    pub hide: Vec<Attribute>,
    /// If non-empty, only accounts matching **all** of these pass.
    pub only: Vec<Attribute>,
    /// Usernames that never show up, lowercased.
    pub excluded: HashSet<String>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self.hide.is_empty() && self.only.is_empty() && self.excluded.is_empty()
    }

    pub fn allows(&self, u: &User) -> bool {
        !self.excludes(&u.username)
            && !self.hide.iter().any(|a| a.matches(u))
            && self.only.iter().all(|a| a.matches(u))
    }

    /// Asked per account of every list, so a name already in lowercase (as
    /// Instagram's are) is looked up as it is, with no copy made.
    fn excludes(&self, username: &str) -> bool {
        if self.excluded.is_empty() {
            return false;
        }
        let lowercased = if username
            .bytes()
            .all(|b| b.is_ascii() && !b.is_ascii_uppercase())
        {
            Cow::Borrowed(username)
        } else {
            Cow::Owned(username.to_lowercase())
        };
        self.excluded.contains(&*lowercased)
    }

    pub fn apply(&self, users: Vec<User>) -> Vec<User> {
        if self.is_empty() {
            return users;
        }
        users.into_iter().filter(|u| self.allows(u)).collect()
    }
}

/// Reads a list of usernames, one per line, tolerating a leading at sign,
/// surrounding space, blank lines and `#` comments.
///
/// **And a byte-order mark**, which `str::trim` does not remove (U+FEFF is
/// `Cf`, not `White_Space`): the first name of a file written by Notepad or
/// PowerShell 5.1 would otherwise match nobody, silently. The `@` comes off
/// before the last trim, so `@ alice` is `alice`.
pub fn parse_username_list(contents: &str) -> HashSet<String> {
    contents
        .trim_start_matches('\u{feff}')
        .lines()
        .map(|l| l.trim().trim_matches('\u{feff}').trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.trim_start_matches('@').trim().to_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pk;

    fn user(name: &str) -> User {
        User::named(Pk::new(1), name)
    }

    fn verified() -> User {
        User {
            is_verified: Some(true),
            ..user("celebrity")
        }
    }

    fn private() -> User {
        User {
            is_private: Some(true),
            ..user("reserved")
        }
    }

    fn without_picture() -> User {
        User {
            pfp_url: Some(format!(
                "https://scontent.cdninstagram.com/v/{}.jpg",
                DEFAULT_AVATAR_HASHES[0]
            )),
            ..user("faceless")
        }
    }

    #[test]
    fn an_empty_filter_drops_nobody() {
        let f = Filter::default();
        let list = vec![verified(), private(), user("plain")];
        assert_eq!(f.apply(list).len(), 3);
    }

    #[test]
    fn hide_drops_whoever_matches() {
        let f = Filter {
            hide: vec![Attribute::Verified],
            ..Default::default()
        };
        let left = f.apply(vec![verified(), user("plain")]);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].username, "plain");
    }

    #[test]
    fn only_requires_every_attribute() {
        let f = Filter {
            only: vec![Attribute::Verified, Attribute::Private],
            ..Default::default()
        };
        let both = User {
            is_private: Some(true),
            ..verified()
        };
        assert!(f.allows(&both));
        assert!(!f.allows(&verified()));
        assert!(!f.allows(&private()));
    }

    #[test]
    fn the_default_avatar_is_recognized() {
        assert!(Attribute::NoPfp.matches(&without_picture()));
        assert!(!Attribute::NoPfp.matches(&user("has_a_picture")));

        let own_picture = User {
            pfp_url: Some("https://scontent.cdninstagram.com/v/their_own.jpg".into()),
            ..user("has_a_picture")
        };
        assert!(!Attribute::NoPfp.matches(&own_picture));
    }

    /// Data we do not have cannot make anyone disappear from the list.
    #[test]
    fn unknown_does_not_count_as_matching() {
        let no_data = user("mystery");
        assert!(!Attribute::Verified.matches(&no_data));
        assert!(!Attribute::Private.matches(&no_data));
        assert!(!Attribute::NoPfp.matches(&no_data));

        let f = Filter {
            hide: vec![Attribute::Verified],
            ..Default::default()
        };
        assert!(f.allows(&no_data), "not hidden just in case");
    }

    #[test]
    fn exclusion_ignores_case_and_the_at_sign() {
        let f = Filter {
            excluded: parse_username_list("@Friend\nother"),
            ..Default::default()
        };
        assert!(!f.allows(&user("friend")));
        assert!(!f.allows(&user("FRIEND")));
        assert!(!f.allows(&user("other")));
        assert!(f.allows(&user("stranger")));
    }

    #[test]
    fn the_username_list_tolerates_how_people_write_it() {
        let read = parse_username_list(
            "# people I keep\n\
             @one\n\
             \n\
               two  \n\
             THREE\n\
             @ four\n\
             @\n",
        );
        assert_eq!(read.len(), 4, "{read:?}");
        assert!(read.contains("one"));
        assert!(read.contains("two"));
        assert!(read.contains("three"));
        // A space after the sign is how it is typed when the sign is an
        // afterthought; stripped after the trim, the key would be " four".
        assert!(read.contains("four"), "{read:?}");
        assert!(!read.contains(""), "a bare sign is not an account");
    }

    /// The file the tool is handed on Windows most of the time.
    ///
    /// `str::trim` goes by `White_Space`, which does not contain U+FEFF, so a
    /// plain trim leaves the first key as `"\u{feff}alice"` — a string no
    /// username equals. Only the first line is affected, so the file looks
    /// read: the exclusion applies to everyone in it except line one.
    #[test]
    fn a_byte_order_mark_does_not_hide_the_first_name() {
        let read = parse_username_list("\u{feff}alice\nbob\n");

        assert!(read.contains("alice"), "{read:?}");
        assert!(read.contains("bob"), "{read:?}");
        assert_eq!(read.len(), 2);

        let f = Filter {
            excluded: read,
            ..Default::default()
        };
        assert!(
            !f.allows(&user("alice")),
            "the name the file exists to hide"
        );
    }

    #[test]
    fn hide_beats_only() {
        // If something is on both lists it does not show up: hiding is an
        // explicit refusal and should weigh more.
        let f = Filter {
            hide: vec![Attribute::Verified],
            only: vec![Attribute::Verified],
            ..Default::default()
        };
        assert!(!f.allows(&verified()));
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::with_cases(256) })]

        /// Any exclusion file contents can be read without panicking.
        #[test]
        fn parsing_a_username_list_never_panics_on_any_text(
            text in prop_oneof![
                any::<String>(),
                "[\u{feff}#A-Za-z0-9._! -]{0,80}",
            ],
        ) {
            let _ = parse_username_list(&text);
        }

        /// A name read from a list is compared as it is stored: lowercased and
        /// trimmed. A line that cannot be a username is kept, and matches nobody.
        #[test]
        fn every_name_read_from_a_list_is_trimmed_and_lowercased(
            text in prop_oneof![
                any::<String>(),
                "[\u{feff}#@A-Za-z0-9._! -]{0,80}",
            ],
        ) {
            for name in parse_username_list(&text) {
                prop_assert!(!name.is_empty(), "from {:?}", text);
                prop_assert_eq!(name.trim(), name.as_str(), "from {:?}", text);
                prop_assert_eq!(name.to_lowercase(), name.clone(), "from {:?}", text);
            }
        }
    }
}
