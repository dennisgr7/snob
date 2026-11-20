//! What the editor's dialogs turn what is typed into: a webhook's address
//! and headers, and whose account an action reads, with the yes that
//! another account's lists need. A schedule is its own
//! ([`super::schedule`]).
//!
//! **What is wrong with it is the engine's to say**: it is judged by
//! `Automation::problems` on a copy that holds the step as typed
//! ([`problems_of`]).
//!
//! No window and no clock: the moment of a yes is handed in.

use std::collections::BTreeMap;
use std::path::PathBuf;

use snob_core::automation::{
    Account, Automation, NodeId, Problem, Read, Step, VideoQuality, Webhook, WebhookFormat,
};
use snob_core::{Epoch, Pk};

fn nothing_as_none(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// An action's settings as they are chosen and typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionForm {
    /// Whose account is read: the viewer's own, or `username`'s.
    pub mine: bool,
    pub username: String,
    /// The yes to walking another account's lists.
    pub consent: bool,
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
            username,
            consent: account.is_some_and(|account| account.consent.is_some()),
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
        let consent = (self.consent && self.needs_consent()).then(|| {
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

        form.consent = true;
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
