//! What the editor's dialogs turn what is typed into: a webhook's address
//! and headers, and whose account an action reads, with the yes that
//! another account's lists need and the names suggested as one is typed.
//! A schedule is its own ([`super::schedule`]).
//!
//! **What is wrong with it is the engine's to say**: it is judged by
//! `Automation::problems` on a copy that holds the step as typed
//! ([`problems_of`]).
//!
//! No window and no clock: the moment of a yes is handed in.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use snob_core::automation::{
    Account, Automation, NodeId, Problem, Read, Step, VideoQuality, Webhook, WebhookFormat,
};
use snob_core::model::possible_username;
use snob_core::{Epoch, Pk};

fn nothing_as_none(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// A name typed without its blanks and its at sign.
fn bare(typed: &str) -> &str {
    let typed = typed.trim();
    typed.strip_prefix('@').unwrap_or(typed)
}

/// Whether two names are one account's: a username is the same whatever
/// its case.
fn same_name(one: &str, other: &str) -> bool {
    bare(one).eq_ignore_ascii_case(bare(other))
}

/// An action's settings as they are chosen and typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionForm {
    /// Whose account is read: the viewer's own, or `username`'s.
    pub mine: bool,
    pub username: String,
    /// The name the yes to walking another account's lists was given for.
    /// **It is that name's alone**: another name typed takes it away
    /// ([`Self::rename`]), and the question is asked again of the new one.
    pub consent: Option<String>,
    pub save_to: String,
    pub quality: Option<VideoQuality>,
    what: Read,
}

impl ActionForm {
    pub fn of(account: Option<&Account>, what: &Read) -> Self {
        let username = account
            .and_then(|account| account.name())
            .unwrap_or_default()
            .to_string();
        let (save_to, quality) = match what {
            Read::Posts { save_to, quality } => (save_to.clone(), *quality),
            Read::Picture { save_to } => (save_to.clone(), None),
            _ => (None, None),
        };
        Self {
            mine: username.is_empty() && !what.is_about_another(),
            consent: account
                .filter(|account| account.consent.is_some())
                .map(|_| username.clone()),
            username,
            save_to: save_to
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            quality,
            what: what.clone(),
        }
    }

    /// The same choices with what was typed in the two fields.
    #[must_use]
    pub fn typed(&self, username: String, save_to: String) -> Self {
        Self {
            username,
            save_to,
            ..self.clone()
        }
    }

    pub fn what(&self) -> &Read {
        &self.what
    }

    /// The name typed, as the yes and the question name it: no blanks and
    /// no at sign.
    pub fn name(&self) -> &str {
        bare(&self.username)
    }

    /// Whether the yes was given, and for the name typed now.
    pub fn given(&self) -> bool {
        self.consent
            .as_deref()
            .is_some_and(|given| same_name(given, &self.username))
    }

    /// The yes ticked for the name typed now, or taken back.
    pub fn consent_to(&mut self, yes: bool) {
        self.consent = yes.then(|| self.username.clone());
    }

    /// Another name typed or chosen: a yes given for the one before goes,
    /// so typing that one again does not tick it back.
    pub fn rename(&mut self, username: &str) {
        if !self
            .consent
            .as_deref()
            .is_some_and(|given| same_name(given, username))
        {
            self.consent = None;
        }
        self.username = username.to_string();
    }

    /// Whether the yes is asked for: somebody else's lists are walked.
    pub fn needs_consent(&self) -> bool {
        !self.mine && !self.username.trim().is_empty() && self.what.walks_lists()
    }

    pub fn saves_files(&self) -> bool {
        matches!(self.what, Read::Posts { .. } | Read::Picture { .. })
    }

    pub fn saves_videos(&self) -> bool {
        matches!(self.what, Read::Posts { .. })
    }

    /// The step as chosen. **The account in use is the reader** the first
    /// time it is said, and a yes given before stays the moment it was
    /// given.
    pub fn step(&self, before: Option<&Account>, in_use: Option<Pk>, now: Epoch) -> Step {
        let username = (!self.mine)
            .then(|| nothing_as_none(&self.username))
            .flatten();
        let consent = (self.given() && self.needs_consent()).then(|| {
            before
                .filter(|before| before.name() == username.as_deref())
                .and_then(|before| before.consent)
                .unwrap_or(now)
        });
        let save_to = nothing_as_none(&self.save_to).map(PathBuf::from);
        let what = match &self.what {
            Read::Posts { .. } => Read::Posts {
                save_to,
                quality: self.quality,
            },
            Read::Picture { .. } => Read::Picture { save_to },
            other => other.clone(),
        };
        Step::Read {
            account: Some(Account {
                username,
                viewer: before.and_then(|before| before.viewer).or(in_use),
                consent,
            }),
            what,
        }
    }
}

/// A webhook as it is typed: its headers a row of two fields each.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookForm {
    pub url: String,
    pub format: WebhookFormat,
    pub heartbeat: bool,
    pub headers: Vec<(String, String)>,
}

impl WebhookForm {
    pub fn of(webhook: &Webhook) -> Self {
        Self {
            url: webhook.url.clone(),
            format: webhook.format,
            heartbeat: webhook.heartbeat,
            headers: webhook
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        }
    }

    /// The webhook as typed: a row without a name says nothing.
    pub fn step(&self) -> Step {
        let headers: BTreeMap<String, String> = self
            .headers
            .iter()
            .filter(|(name, _)| !name.trim().is_empty())
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .collect();
        Step::Webhook(Webhook {
            url: self.url.trim().to_string(),
            headers,
            heartbeat: self.heartbeat,
            format: self.format,
        })
    }
}

/// How long typing in an account's field stays still before the accounts
/// known are asked for: one ask for a burst of keys, not one a key.
pub const SUGGESTED_AFTER: Duration = Duration::from_millis(150);

/// The most known accounts suggested under a field at once.
pub const MOST_SUGGESTED: usize = 6;

/// What the accounts known are asked for while `typed` is in an account's
/// field: nothing while nothing is typed.
pub fn wanted(typed: &str) -> Option<String> {
    let name = bare(typed);
    (!name.is_empty()).then(|| name.to_string())
}

/// A row of the list under an account's field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suggestion {
    /// The account found at this place.
    Known(usize),
    /// "Another person": the name typed, taken as it is.
    Another(String),
}

/// The rows under an account's field for `typed`, from the usernames of
/// the accounts found for it (`found`, in order): each one whose name
/// begins with what is typed now, then, **always last**, "another person"
/// when what is typed can be a username and no row has exactly that name.
pub fn suggested<'a>(
    typed: &str,
    found: impl IntoIterator<Item = Option<&'a str>>,
) -> Vec<Suggestion> {
    let Some(wanted) = wanted(typed) else {
        return Vec::new();
    };
    let lower = wanted.to_ascii_lowercase();
    let mut exact = false;
    let mut rows: Vec<Suggestion> = found
        .into_iter()
        .enumerate()
        .filter_map(|(at, name)| {
            let name = name?;
            exact |= same_name(name, &wanted);
            name.to_ascii_lowercase()
                .starts_with(&lower)
                .then_some(Suggestion::Known(at))
        })
        .collect();
    // Judged as typed: `wanted` lost an at sign already, and the rule takes
    // off one more.
    if possible_username(typed.trim()) && !exact {
        rows.push(Suggestion::Another(wanted));
    }
    rows
}

/// What `automation` would be told is wrong with the step `id` if it held
/// `step`: the engine's rules, applied to a copy.
pub fn problems_of(automation: &Automation, id: NodeId, step: Step) -> Vec<Problem> {
    let mut copy = automation.clone();
    copy.configure(id, step);
    copy.problems()
        .into_iter()
        .filter(|problem| problem.node() == Some(id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_of(step: Step) -> Account {
        let Step::Read { account, .. } = step else {
            panic!("an action");
        };
        account.unwrap()
    }

    #[test]
    fn an_action_on_another_accounts_lists_asks_a_yes_and_keeps_the_moment_it_was_given() {
        let mut form = ActionForm::of(None, &Read::Lists);
        assert!(form.mine && !form.needs_consent());
        form.mine = false;
        assert!(!form.needs_consent(), "no name yet");
        form.username = "nobody".to_string();
        assert!(form.needs_consent());

        let asked = account_of(form.step(None, Some(Pk::new(42)), Epoch::new(100)));
        assert_eq!(asked.consent, None, "no yes was ticked");
        assert_eq!(asked.viewer, Some(Pk::new(42)));

        form.consent_to(true);
        let given = account_of(form.step(Some(&asked), Some(Pk::new(7)), Epoch::new(100)));
        assert_eq!(given.consent, Some(Epoch::new(100)));
        assert_eq!(given.viewer, Some(Pk::new(42)), "the reader stays");

        let later = account_of(form.step(Some(&given), None, Epoch::new(999)));
        assert_eq!(later.consent, Some(Epoch::new(100)), "the first yes");

        form.mine = true;
        let own = account_of(form.step(None, None, Epoch::new(1)));
        assert_eq!((own.username, own.consent), (None, None));
    }

    #[test]
    fn the_yes_belongs_to_the_name_it_was_given_for_and_goes_when_another_is_typed() {
        let mut form = ActionForm::of(None, &Read::Lists);
        form.mine = false;
        form.rename("anna");
        assert_eq!(form.consent, None, "nothing is ticked before it is asked");
        form.consent_to(true);
        assert!(form.given());

        form.rename("@ANNA ");
        assert!(form.given(), "the same account, however it is written");
        form.rename("annabel");
        assert!(!form.given());
        assert_eq!(form.consent, None);
        form.rename("anna");
        assert!(
            !form.given(),
            "typing the first name again does not tick it back"
        );
        let asked = account_of(form.step(None, None, Epoch::new(5)));
        assert_eq!(asked.consent, None);

        let kept = Account {
            username: Some("anna".to_string()),
            viewer: None,
            consent: Some(Epoch::new(1)),
        };
        let opened = ActionForm::of(Some(&kept), &Read::Lists);
        assert!(opened.given(), "a yes kept is the name's it was kept with");
        assert!(!opened.typed("bob".to_string(), String::new()).given());
    }

    #[test]
    fn known_accounts_are_suggested_by_their_first_letters_and_another_person_comes_last() {
        let found = [Some("anna"), Some("Annabel"), None, Some("bob")];
        assert_eq!(
            suggested("@An", found),
            [
                Suggestion::Known(0),
                Suggestion::Known(1),
                Suggestion::Another("An".to_string()),
            ]
        );
        assert_eq!(
            suggested(" ANNA", found),
            [Suggestion::Known(0), Suggestion::Known(1)],
            "a row already has that name"
        );
        assert_eq!(
            suggested("annab", [Some("anna"), Some("Annabel")]),
            [
                Suggestion::Known(1),
                Suggestion::Another("annab".to_string())
            ],
            "what was found for fewer letters is held to what is typed now"
        );
        assert_eq!(
            suggested("ana", []),
            [Suggestion::Another("ana".to_string())]
        );
    }

    #[test]
    fn another_person_is_offered_only_for_what_can_be_a_username() {
        let none: [Option<&str>; 0] = [];
        for typed in [
            "",
            "  ",
            "@",
            "@@anna",
            "an a",
            "ana!",
            ".",
            "..",
            &"a".repeat(31),
        ] {
            assert!(
                !suggested(typed, none)
                    .iter()
                    .any(|row| matches!(row, Suggestion::Another(_))),
                "{typed:?}"
            );
        }
        assert_eq!(wanted("  "), None, "nothing typed asks for nothing");
        assert_eq!(wanted(" @ana "), Some("ana".to_string()));
        assert_eq!(
            suggested("ana.b_1", none),
            [Suggestion::Another("ana.b_1".to_string())]
        );
    }

    #[test]
    fn where_files_are_saved_and_how_good_the_videos_are_belong_to_posts_and_pictures() {
        let posts = Read::Posts {
            save_to: None,
            quality: None,
        };
        let mut form = ActionForm::of(None, &posts);
        assert!(form.saves_files() && form.saves_videos());
        form.save_to = "/home/me/posts".to_string();
        form.quality = Some(VideoQuality::Compat);
        let Step::Read { what, .. } = form.step(None, None, Epoch::new(1)) else {
            panic!("an action");
        };
        let saved = Read::Posts {
            save_to: Some(PathBuf::from("/home/me/posts")),
            quality: Some(VideoQuality::Compat),
        };
        assert_eq!(what, saved);
        let picture = ActionForm::of(None, &Read::Picture { save_to: None });
        assert!(picture.saves_files() && !picture.saves_videos());
        assert!(!ActionForm::of(None, &Read::Fans).saves_files());
    }

    #[test]
    fn an_action_about_another_account_never_starts_as_the_viewers_own() {
        assert!(!ActionForm::of(None, &Read::KnownFollowers).mine);
        assert!(ActionForm::of(None, &Read::Friends).mine);
    }

    #[test]
    fn a_webhook_typed_drops_the_rows_without_a_name_and_the_blanks_around_the_rest() {
        let form = WebhookForm {
            url: " https://example.com/hook ".to_string(),
            format: WebhookFormat::Slack,
            heartbeat: true,
            headers: vec![
                (" X-Team ".to_string(), " snob ".to_string()),
                (String::new(), "orphan".to_string()),
            ],
        };
        let Step::Webhook(webhook) = form.step() else {
            panic!("a webhook");
        };
        assert_eq!(webhook.url, "https://example.com/hook");
        assert_eq!(webhook.headers.len(), 1);
        assert_eq!(webhook.headers["X-Team"], "snob");
        assert_eq!(webhook.format, WebhookFormat::Slack);
        assert!(webhook.heartbeat);
        assert_eq!(WebhookForm::of(&webhook).headers.len(), 1);
    }

    #[test]
    fn what_is_wrong_with_a_step_as_typed_is_what_the_engine_says_of_it() {
        let mut automation = Automation::new("a1", "Daily");
        let hook = automation.insert(Step::Webhook(Webhook::default()), (0, 0));
        let typed = WebhookForm {
            url: "ftp://nowhere".to_string(),
            ..WebhookForm::default()
        };
        let wrong = problems_of(&automation, hook, typed.step());
        assert_eq!(wrong, vec![Problem::NoAddress { node: hook }]);
        let typed = WebhookForm {
            url: "https://example.com".to_string(),
            ..WebhookForm::default()
        };
        assert!(problems_of(&automation, hook, typed.step()).is_empty());
    }
}
