//! Where automations are kept: one JSON file each under the configuration
//! directory's `automations/`, written whole under a lock, and `watch.toml`
//! read as one more ([`from_watch`]) and written back from one ([`to_watch`]).
//!
//! **No secrets live here**, as in `watch.toml`: a webhook's headers are
//! plain text at a guessable path, and a token belongs in the keyring.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use snob_core::automation::{
    Account, Automation, Edge, Node, NodeId, Problem, Read, SCHEMA, Step, WATCH_TOML, Webhook,
};

use crate::config::{
    AccountConfig, ConfigError, ConsentConfig, PostsConfig, WatchConfig, WebhookConfig,
};
use crate::paths::AppPaths;

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("\"{0}\" cannot name an automation")]
    BadId(String),
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} could not be read: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error(
        "{path} says it is schema {found}, and this version of snob understands up to {SCHEMA}"
    )]
    Unknown { path: PathBuf, found: u32 },
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Paths(#[from] crate::paths::PathError),
}

/// What one file holds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema: u32,
    automation: Automation,
}

/// The directory the files are in.
pub fn dir(paths: &AppPaths) -> PathBuf {
    paths.config_dir().join("automations")
}

/// Whether `id` may name a file: 1 to 40 of `[a-z0-9-]`, and never
/// `watch`, which is `watch.toml`'s.
pub fn possible_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && id != WATCH_TOML
}

fn path_of(paths: &AppPaths, id: &str) -> Result<PathBuf, AutomationError> {
    if !possible_id(id) {
        return Err(AutomationError::BadId(id.to_string()));
    }
    Ok(dir(paths).join(format!("{id}.json")))
}

/// Every automation kept in a file, by id. A file that cannot be read is
/// left out and said in the second half, never a reason to lose the rest.
pub fn list(paths: &AppPaths) -> (Vec<Automation>, Vec<AutomationError>) {
    let mut found = Vec::new();
    let mut failed = Vec::new();
    let entries = match std::fs::read_dir(dir(paths)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (found, failed),
        Err(source) => {
            failed.push(AutomationError::Read {
                path: dir(paths),
                source,
            });
            return (found, failed);
        }
    };
    let mut ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(".json")
                .filter(|id| possible_id(id))
                .map(str::to_string)
        })
        .collect();
    ids.sort();
    for id in ids {
        match load(paths, &id) {
            Ok(Some(automation)) => found.push(automation),
            Ok(None) => {}
            Err(e) => failed.push(e),
        }
    }
    (found, failed)
}

/// The automation kept as `id`, if there is one.
pub fn load(paths: &AppPaths, id: &str) -> Result<Option<Automation>, AutomationError> {
    let path = path_of(paths, id)?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(AutomationError::Read { path, source }),
    };
    let file: File = serde_json::from_str(&text).map_err(|e| AutomationError::Invalid {
        path: path.clone(),
        message: e.to_string(),
    })?;
    if file.schema != SCHEMA {
        return Err(AutomationError::Unknown {
            path,
            found: file.schema,
        });
    }
    let mut automation = file.automation;
    // The name of the file is the id: a copied file answers to its own.
    automation.id = id.to_string();
    Ok(Some(automation))
}

/// Writes it whole, one writer at a time.
pub fn save(paths: &AppPaths, automation: &Automation) -> Result<PathBuf, AutomationError> {
    let path = path_of(paths, &automation.id)?;
    crate::paths::create_private_dir(&dir(paths))?;
    let text = serde_json::to_string_pretty(&File {
        schema: SCHEMA,
        automation: automation.clone(),
    })
    .map_err(|e| AutomationError::Invalid {
        path: path.clone(),
        message: e.to_string(),
    })?;
    let temporary = path.with_extension("json.new");
    crate::paths::replace_private_alone(&path, &temporary, text.as_bytes())
        .map_err(|(path, source)| AutomationError::Write { path, source })?;
    Ok(path)
}

/// Removes it; `false` when there was none.
pub fn remove(paths: &AppPaths, id: &str) -> Result<bool, AutomationError> {
    let path = path_of(paths, id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(AutomationError::Write { path, source }),
    }
}

/// Where a step of `watch.toml`'s automation is drawn: the trigger, then a
/// column of actions, then the webhook.
const COLUMN: i32 = 320;
const ROW: i32 = 140;

/// `watch.toml` as an automation: its schedule the one trigger, each entry's
/// lists and posts an action of their own, and its webhook the output every
/// action leads to. A file with no entry reads the own lists.
pub fn from_watch(config: &WatchConfig) -> Automation {
    let mut automation = Automation::new(WATCH_TOML, "snob watch");
    let trigger = automation.insert(
        Step::Schedule {
            when: config.spec(),
        },
        (0, 0),
    );
    let own = [AccountConfig::lists_of("self".to_string(), None, None)];
    let entries: &[AccountConfig] = if config.accounts.is_empty() {
        &own
    } else {
        &config.accounts
    };
    let mut actions: Vec<NodeId> = Vec::new();
    for entry in entries {
        let account = if entry.is_own() {
            Account::own(entry.viewer)
        } else {
            Account {
                username: Some(entry.target.clone()),
                viewer: entry.viewer,
                consent: entry.consent.map(|consent| consent.agreed_at),
            }
        };
        let mut reads = Vec::new();
        if entry.lists {
            reads.push(Read::Lists);
        }
        if let Some(posts) = &entry.posts {
            reads.push(Read::Posts {
                save_to: posts.save_to.clone(),
                quality: posts.quality,
            });
        }
        for what in reads {
            let row = i32::try_from(actions.len()).unwrap_or(0);
            let id = automation.insert(
                Step::Read {
                    account: Some(account.clone()),
                    what,
                },
                (COLUMN, row.saturating_mul(ROW)),
            );
            automation.edges.push(Edge {
                from: trigger,
                to: id,
            });
            actions.push(id);
        }
    }
    if let Some(webhook) = &config.webhook {
        let output = automation.insert(
            Step::Webhook(Webhook {
                url: webhook.url.clone(),
                headers: webhook.headers.clone(),
                heartbeat: webhook.heartbeat,
                format: webhook.format,
            }),
            (2 * COLUMN, 0),
        );
        for action in &actions {
            automation.edges.push(Edge {
                from: *action,
                to: output,
            });
        }
    }
    automation
}

/// The automation as `watch.toml`, or every step the file cannot hold ([`Problem::NotInWatchToml`]): a
/// second trigger or webhook, a disabled step, a filter, an action other
/// than lists and posts, or an arrow the file has no way to say (an action
/// no trigger leads to, or one that does not lead to the webhook when there
/// is one).
pub fn to_watch(automation: &Automation) -> Result<WatchConfig, Vec<Problem>> {
    let mut problems = Vec::new();
    let mut config = WatchConfig {
        schema: crate::config::SCHEMA,
        every: None,
        on: Vec::new(),
        at: Vec::new(),
        cron: None,
        jitter: None,
        webhook: None,
        accounts: Vec::new(),
    };
    let not_here = |node: &Node| Problem::NotInWatchToml { node: node.id };

    let mut trigger: Option<NodeId> = None;
    let mut output: Option<NodeId> = None;
    for node in &automation.nodes {
        if !node.enabled {
            problems.push(not_here(node));
            continue;
        }
        match &node.step {
            Step::Schedule { when } if trigger.is_none() => {
                trigger = Some(node.id);
                config.every = when.every;
                config.on.clone_from(&when.on);
                config.at.clone_from(&when.at);
                config.cron.clone_from(&when.cron);
                config.jitter = when.jitter;
            }
            Step::Webhook(webhook) if output.is_none() => {
                output = Some(node.id);
                config.webhook = Some(WebhookConfig {
                    url: webhook.url.clone(),
                    headers: webhook.headers.clone(),
                    heartbeat: webhook.heartbeat,
                    format: webhook.format,
                });
            }
            Step::Schedule { .. } | Step::Webhook(_) | Step::Filter { .. } => {
                problems.push(not_here(node));
            }
            Step::Read { .. } => {}
        }
    }

    let joined = |from: NodeId, to: NodeId| automation.edges.contains(&Edge { from, to });
    for node in automation.nodes.iter().filter(|node| node.enabled) {
        let Step::Read { account, what } = &node.step else {
            continue;
        };
        let fits = matches!(what, Read::Lists | Read::Posts { .. })
            && trigger.is_some_and(|trigger| joined(trigger, node.id))
            && output.is_none_or(|output| joined(node.id, output));
        let Some(account) = account.as_ref().filter(|_| fits) else {
            problems.push(not_here(node));
            continue;
        };
        let target = account.name().unwrap_or("self").to_string();
        let found = config.accounts.iter().position(|entry| {
            entry.viewer == account.viewer && entry.target.eq_ignore_ascii_case(&target)
        });
        let at = found.unwrap_or_else(|| {
            config.accounts.push(AccountConfig {
                target: target.clone(),
                viewer: account.viewer,
                consent: account
                    .consent
                    .filter(|_| account.name().is_some())
                    .map(|agreed_at| ConsentConfig { agreed_at }),
                lists: false,
                posts: None,
            });
            config.accounts.len() - 1
        });
        let entry = &mut config.accounts[at];
        match what {
            Read::Lists => entry.lists = true,
            Read::Posts { save_to, quality } => {
                entry.posts = Some(PostsConfig {
                    save_to: save_to.clone(),
                    quality: *quality,
                });
            }
            _ => {}
        }
    }
    if problems.is_empty() {
        Ok(config)
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use snob_core::automation::{Only, WebhookFormat};
    use snob_core::{Epoch, Pk};

    fn config() -> WatchConfig {
        WatchConfig {
            every: Some(Duration::from_secs(6 * 3600)),
            jitter: Some(Duration::from_secs(600)),
            webhook: Some(WebhookConfig {
                url: "https://n8n.local/webhook/snob".into(),
                headers: [("X-Team".to_string(), "ops".to_string())].into(),
                heartbeat: true,
                format: WebhookFormat::Slack,
            }),
            accounts: vec![
                AccountConfig::lists_of("self".into(), Some(Pk::new(7)), None),
                AccountConfig {
                    target: "friend".into(),
                    viewer: Some(Pk::new(7)),
                    consent: Some(ConsentConfig {
                        agreed_at: Epoch::new(1_700_000_000),
                    }),
                    lists: true,
                    posts: Some(PostsConfig {
                        save_to: Some(PathBuf::from(if cfg!(windows) {
                            "C:\\media"
                        } else {
                            "/media"
                        })),
                        quality: None,
                    }),
                },
            ],
            ..WatchConfig::default()
        }
    }

    #[test]
    fn watch_toml_reads_as_a_finished_automation_and_writes_back_the_same() {
        let config = config();
        let automation = from_watch(&config);
        assert_eq!(automation.id, WATCH_TOML);
        assert_eq!(automation.problems(), []);
        let program = automation.program().unwrap();
        assert_eq!(
            program.reads.len(),
            3,
            "the own lists, and a friend's lists and posts"
        );
        assert!(program.reads.iter().all(|read| read.delivered));
        assert_eq!(to_watch(&automation), Ok(config));
    }

    #[test]
    fn a_file_with_no_entry_reads_the_own_lists() {
        let automation = from_watch(&WatchConfig {
            every: Some(Duration::from_secs(86_400)),
            ..WatchConfig::default()
        });
        let program = automation.program().unwrap();
        assert_eq!(program.reads.len(), 1);
        assert_eq!(program.reads[0].what, Read::Lists);
        assert_eq!(program.reads[0].account, Account::own(None));
    }

    #[test]
    fn a_step_watch_toml_cannot_hold_is_named() {
        let mut automation = from_watch(&config());
        let action = automation.nodes[1].id;
        let filter = automation.insert(Step::Filter { only: Only::Public }, (0, 0));
        automation.set_enabled(action, false);
        let problems = to_watch(&automation).unwrap_err();
        assert!(problems.contains(&Problem::NotInWatchToml { node: filter }));
        assert!(problems.contains(&Problem::NotInWatchToml { node: action }));
    }

    #[test]
    fn an_automation_is_kept_whole_and_named_by_its_file() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        let mut automation = from_watch(&config());
        automation.id = "ab12".into();
        automation.name = "Friends".into();
        save(&paths, &automation).unwrap();
        assert_eq!(load(&paths, "ab12").unwrap(), Some(automation.clone()));
        let (all, failed) = list(&paths);
        assert_eq!(all, [automation]);
        assert!(failed.is_empty());
        assert!(remove(&paths, "ab12").unwrap());
        assert!(!remove(&paths, "ab12").unwrap());
        assert!(matches!(
            save(&paths, &Automation::new(WATCH_TOML, "no")),
            Err(AutomationError::BadId(_))
        ));
        assert!(matches!(
            load(&paths, "../x"),
            Err(AutomationError::BadId(_))
        ));
    }
}
