//! The diagnostic report a person copies into an issue: what the system is,
//! the newest lines of the log, and the last crash. **Nobody is named in
//! it**: every account, the person's own included, is replaced by a stand-in
//! that is the same account wherever it appears, so the lines still tell a
//! story. The log holds no secret to begin with (`snob_engine::log::scrub`);
//! this takes out who.

use std::collections::BTreeMap;

/// How many of the newest log lines a report carries.
pub const LINES: usize = 300;

/// The report: `about` (the app, its version, the system), the last crash
/// when there was one, and `lines`, oldest first.
pub fn diagnostic(about: &str, lines: &[String], crash: Option<&str>) -> String {
    let mut names = Names::default();
    let mut report = String::new();
    report.push_str("snob desktop: diagnostic report\n");
    report.push_str(&names.anonymous(about));
    report.push('\n');
    if let Some(crash) = crash {
        report.push_str("\n--- last crash ---\n");
        report.push_str(&names.anonymous(crash));
        report.push('\n');
    }
    report.push_str(&format!("\n--- log, newest {} lines ---\n", lines.len()));
    for line in lines {
        report.push_str(&names.anonymous(line));
        report.push('\n');
    }
    report
}

/// The stand-ins handed out so far: the same account is the same stand-in
/// through a whole report.
#[derive(Default)]
struct Names {
    users: BTreeMap<String, usize>,
    ids: BTreeMap<String, usize>,
}

/// What comes before an account's id wherever a line gives one.
const BEFORE_AN_ID: [&str; 6] = [
    "account ",
    "accounts/",
    "accounts\\",
    "pk=",
    "\"pk\":",
    "ds_user_id=",
];

impl Names {
    fn user(&mut self, name: &str) -> String {
        let next = self.users.len() + 1;
        let n = *self.users.entry(name.to_ascii_lowercase()).or_insert(next);
        format!("@user{n}")
    }

    fn id(&mut self, id: &str) -> String {
        let next = self.ids.len() + 1;
        let n = *self.ids.entry(id.to_string()).or_insert(next);
        format!("<account {n}>")
    }

    /// `text` with every `@name` and every account id replaced, and this
    /// user's home directory with `~`.
    fn anonymous(&mut self, text: &str) -> String {
        let text = without_home(text);
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while !rest.is_empty() {
            // An id, after one of the words that announce it.
            if let Some(before) = BEFORE_AN_ID
                .iter()
                .find(|before| rest.starts_with(**before))
            {
                let after = &rest[before.len()..];
                let digits = after.bytes().take_while(u8::is_ascii_digit).count();
                if digits > 0 {
                    out.push_str(before);
                    out.push_str(&self.id(&after[..digits]));
                    rest = &after[digits..];
                    continue;
                }
            }
            // A username: an at sign that starts a word, and what Instagram
            // allows in a name.
            if let Some(after) = rest.strip_prefix('@') {
                let starts_a_word = out
                    .chars()
                    .next_back()
                    .is_none_or(|before| !before.is_alphanumeric() && before != '.');
                let name = after
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
                    .count();
                if starts_a_word && name > 0 {
                    out.push_str(&self.user(&after[..name]));
                    rest = &after[name..];
                    continue;
                }
            }
            let mut chars = rest.chars();
            if let Some(next) = chars.next() {
                out.push(next);
            }
            rest = chars.as_str();
        }
        out
    }
}

/// `text` with this user's home directory, which names them, as `~`.
fn without_home(text: &str) -> String {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|home| home.to_string_lossy().into_owned())
        .filter(|home| home.len() > 3);
    match home {
        Some(home) => text
            .replace(&home, "~")
            .replace(&home.replace('\\', "\\\\"), "~")
            .replace(&home.replace('\\', "/"), "~"),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nobody_is_named_and_each_account_keeps_one_stand_in() {
        let lines = [
            r#"{"msg":"walked","target":"@someone","viewer":"@me","pk":9001}"#.to_string(),
            "account 42 signed in; reading accounts/42/snob.db".to_string(),
            "@someone again, and @Someone, and mail to dev@example.invalid".to_string(),
        ];
        let report = diagnostic("snob desktop 0.9.0 on Windows 11", &lines, None);

        for named in ["someone", "@me", "9001", "42"] {
            assert!(!report.contains(named), "{named} is still in:\n{report}");
        }
        assert!(report.contains(r#""target":"@user1","viewer":"@user2","pk":<account 1>"#));
        assert!(
            report.contains("account <account 2> signed in; reading accounts/<account 2>/snob.db")
        );
        assert!(report.contains("@user1 again, and @user1"), "{report}");
        // Not every at sign is an account, and not every number an id.
        assert!(report.contains("dev@example.invalid"));
        assert!(report.contains("Windows 11"));
        assert!(report.contains("newest 3 lines"));
    }

    #[test]
    fn the_last_crash_is_carried_without_who_it_happened_to() {
        let report = diagnostic(
            "snob desktop",
            &[],
            Some("panicked at src/main.rs:12\nwhile showing @someone"),
        );
        assert!(report.contains("--- last crash ---\npanicked at src/main.rs:12"));
        assert!(report.contains("while showing @user1"));
    }

    #[test]
    fn the_home_directory_does_not_name_the_person() {
        let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        else {
            return;
        };
        let home = home.to_string_lossy().into_owned();
        let line = format!("the log is at {home}/data/logs");
        let report = diagnostic("snob desktop", &[line], None);
        assert!(report.contains("the log is at ~/data/logs"), "{report}");
    }
}
