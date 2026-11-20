//! What a run found, as data kept beside the sentences of its history
//! (`automation_runs.found`), so a front says it in its own language and
//! falls back on those sentences for a run kept before, or for a kind it
//! does not know ([`read`]).
//!
//! The names are the ones the automation's report sends
//! (`wire::found_payload`). **No secret and no media address**: counts, and
//! at most [`NAMED`] usernames, each through `printable`. **Its shape is
//! output**: `snob watch automations --json` prints it ("Output contract").

use serde::{Deserialize, Serialize};
use snob_core::model::{User, printable};
use snob_core::watch::Basis;
use snob_core::watch::posts::Basis as PostsBasis;
use snob_core::{Epoch, Pk};

use super::read::{Changes, Found};
use crate::exit;
use crate::watch::posts::PostsSkipped;
use crate::watch::{ListReport, TickReport};

/// How many usernames each list of a finding keeps.
pub const NAMED: usize = 20;

/// One thing a run found, or why it found nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Said {
    /// What an action other than lists and posts found.
    Found {
        /// The action, as a file spells it (`Read::as_str`).
        what: String,
        /// The account it was about; `None` for the viewer's own.
        account: Option<String>,
        /// The accounts its filters let through.
        count: usize,
        /// The accounts it found before the filters.
        total: usize,
        changes: Option<FoundChanges>,
        /// When the lists it was worked out from were read, when not in
        /// this run.
        stored_since: Option<Epoch>,
        picture: Option<Picture>,
        /// The first of the accounts it let through.
        names: Vec<String>,
    },
    /// What the monitor's look at one account came to.
    Tick {
        /// The account's username, when it is known.
        account: Option<String>,
        pk: Pk,
        /// Whether it is the viewer's own account.
        own: bool,
        /// Whether a list it reads, or its posts when it reads nothing else,
        /// could not be looked at.
        refused: bool,
        /// Whether nothing of the account has ever been read to compare.
        nothing_stored: bool,
        followers: Option<ListSaid>,
        following: Option<ListSaid>,
        /// How many of its accounts now go by another name.
        renamed: usize,
        posts: Option<PostsSaid>,
    },
    /// A part of the run that failed, by its exit token (`error`,
    /// `rate_limited`, ...).
    Failed { code: String },
    /// Something in the automation's steps keeps it from running.
    CannotRun,
}

/// What came and went since the last run's report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundChanges {
    /// The first run: what it found is what the next compares against.
    pub baseline: bool,
    pub since: Option<Epoch>,
    pub added: usize,
    pub removed: usize,
    pub added_names: Vec<String>,
    pub removed_names: Vec<String>,
}

/// A profile picture read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Picture {
    /// Not there before: a new picture, or the first run.
    pub new: bool,
    /// Whether the action saves pictures.
    pub saved: bool,
}

/// One list of an account the monitor looks at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSaid {
    /// `baseline`, `unchanged` or `compare`.
    pub basis: String,
    pub total: usize,
    pub added: usize,
    pub removed: usize,
    pub added_names: Vec<String>,
    pub removed_names: Vec<String>,
}

/// An account's posts, as the monitor looked at them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostsSaid {
    /// `baseline` or `compared`; `None` when they were not concluded on.
    pub basis: Option<String>,
    /// Why they were not looked at (`PostsSkipped::as_str`, but `cooldown`,
    /// and `read_recently` for posts read within the hour, which is no
    /// refusal).
    pub skipped: Option<String>,
    pub new: usize,
    /// Whether the read reached back to what was seen before.
    pub complete: bool,
    /// How many files were saved of the new posts.
    pub saved: usize,
}

fn names(users: &[User]) -> Vec<String> {
    users.iter().take(NAMED).map(User::safe_username).collect()
}

/// What an action other than lists and posts found.
pub fn of_found(found: &Found) -> Said {
    let changes = found.changes.as_ref().map(|changes| match changes {
        Changes::Baseline => FoundChanges {
            baseline: true,
            since: None,
            added: 0,
            removed: 0,
            added_names: Vec::new(),
            removed_names: Vec::new(),
        },
        Changes::Since {
            since,
            added,
            removed,
        } => FoundChanges {
            baseline: false,
            since: Some(*since),
            added: added.len(),
            removed: removed.len(),
            added_names: names(added),
            removed_names: names(removed),
        },
    });
    Said::Found {
        what: found.what.as_str().to_string(),
        account: found.username.as_deref().map(printable),
        count: found.accounts.len(),
        total: found.total,
        changes,
        stored_since: found.stored_since,
        picture: found.picture.as_ref().map(|picture| Picture {
            new: picture.new,
            saved: picture.path.is_some(),
        }),
        names: names(&found.accounts),
    }
}

fn list_of(report: &ListReport) -> ListSaid {
    let basis = match report.basis {
        Basis::Baseline { .. } => "baseline",
        Basis::Unchanged { .. } => "unchanged",
        Basis::Compare { .. } => "compare",
    };
    ListSaid {
        basis: basis.to_string(),
        total: report.total,
        added: report.diff.gained.len(),
        removed: report.diff.lost.len(),
        added_names: names(&report.diff.gained),
        removed_names: names(&report.diff.lost),
    }
}

/// What the monitor's look at one account came to.
pub fn of_tick(tick: &TickReport) -> Said {
    let report = &tick.report;
    let skipped = tick.posts_skipped();
    let lists_refused = tick.lists.iter().any(|list| list.skipped.is_some());
    let posts_refused = skipped.is_some_and(|skipped| skipped != PostsSkipped::ReadRecently);
    let refused = lists_refused || (tick.lists.is_empty() && posts_refused);
    // Posts alone, read within the hour: nothing to conclude, and nothing
    // missing either.
    let read_recently = tick.lists.is_empty() && skipped == Some(PostsSkipped::ReadRecently);
    let lists_stored = report.followers.is_some() || report.following.is_some();
    let posts = tick.posts.map(|_| PostsSaid {
        basis: report.posts.as_ref().map(|posts| {
            match posts.basis {
                PostsBasis::Baseline => "baseline",
                PostsBasis::Compared => "compared",
            }
            .to_string()
        }),
        skipped: skipped.map(|skipped| match skipped {
            PostsSkipped::ReadRecently => "read_recently".to_string(),
            PostsSkipped::Cooldown => "cooldown".to_string(),
            other => other.as_str().to_string(),
        }),
        new: report.new_posts(),
        complete: report.posts.as_ref().is_none_or(|posts| posts.complete),
        saved: report.posts.as_ref().map_or(0, |posts| {
            posts.saved.iter().map(|saved| saved.files.len()).sum()
        }),
    });
    Said::Tick {
        account: report.username.as_deref().map(printable),
        pk: report.account_pk,
        own: report.is_self,
        refused,
        nothing_stored: !lists_stored && report.posts.is_none() && !refused && !read_recently,
        followers: report.followers.as_ref().map(list_of),
        following: report.following.as_ref().map(list_of),
        renamed: report.renamed.len(),
        posts,
    }
}

/// A part of the run that failed, by its exit token.
pub fn failed(e: &anyhow::Error) -> Said {
    Said::Failed {
        code: exit::exit_code_for(e).as_str().to_string(),
    }
}

/// What a run kept as `found`; `None` for bad JSON or a kind this build does
/// not know, which a front says in the run's sentences instead.
pub fn read(found: &str) -> Option<Vec<Said>> {
    serde_json::from_str(found).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Pk;
    use snob_core::automation::Read;
    use snob_core::model::ListKind;
    use snob_core::watch::ListDiff;

    use crate::automations::read::SavedPicture;
    use crate::exit::{ExitCode, ExitError};

    fn users(from: u64, many: u64) -> Vec<User> {
        (from..from + many)
            .map(|pk| User::named(Pk::new(pk), format!("user{pk}")))
            .collect()
    }

    fn found(what: Read, accounts: Vec<User>, changes: Option<Changes>) -> Found {
        Found {
            node: 2,
            total: accounts.len() + 1,
            what,
            username: Some("friend\u{200b}".to_string()),
            accounts,
            changes,
            stored_since: Some(Epoch::new(1_000)),
            picture: None,
        }
    }

    #[test]
    fn what_an_action_found_is_kept_with_its_counts_and_at_most_twenty_names() {
        let said = of_found(&found(Read::Fans, users(1, 25), None));
        let Said::Found {
            what,
            account,
            count,
            total,
            names,
            changes,
            stored_since,
            ..
        } = &said
        else {
            panic!("{said:?}");
        };
        assert_eq!(what, "fans");
        assert_eq!(account.as_deref(), Some("friend"), "made printable");
        assert_eq!((*count, *total), (25, 26));
        assert_eq!(names.len(), NAMED);
        assert_eq!(names[0], "user1");
        assert_eq!((changes, *stored_since), (&None, Some(Epoch::new(1_000))));

        let since = of_found(&found(
            Read::Unfollowers,
            users(1, 2),
            Some(Changes::Since {
                since: Epoch::new(900),
                added: users(1, 30),
                removed: users(50, 1),
            }),
        ));
        let Said::Found {
            changes: Some(changes),
            ..
        } = since
        else {
            panic!("{since:?}");
        };
        assert_eq!((changes.added, changes.removed), (30, 1));
        assert_eq!(changes.added_names.len(), NAMED);
        assert_eq!(changes.removed_names, ["user50"]);
        assert_eq!(changes.since, Some(Epoch::new(900)));

        let mut picture = found(Read::Picture { save_to: None }, Vec::new(), None);
        picture.picture = Some(SavedPicture {
            path: Some("/somewhere/friend-abc.jpg".into()),
            new: true,
        });
        let said = of_found(&picture);
        let text = serde_json::to_string(&[&said]).unwrap();
        assert!(!text.contains("somewhere"), "no path is kept: {text}");
        assert_eq!(read(&text), Some(vec![said]));
    }

    #[test]
    fn a_tick_is_kept_with_each_list_its_basis_and_its_posts() {
        use crate::watch::{TickList, TickPosts, WatchReport};

        let followers = ListReport {
            kind: ListKind::Followers,
            basis: Basis::Compare {
                before: 1,
                after: 2,
            },
            since: Some(Epoch::new(1_000)),
            until: Epoch::new(2_000),
            diff: ListDiff {
                gained: users(1, 2),
                lost: users(9, 1),
            },
            total: 120,
        };
        let following = ListReport {
            kind: ListKind::Following,
            basis: Basis::Unchanged { snapshot_id: 3 },
            diff: ListDiff::default(),
            total: 98,
            ..followers.clone()
        };
        let report = WatchReport {
            account_pk: Pk::new(7),
            username: Some("friend".into()),
            is_self: false,
            followers: Some(followers),
            following: Some(following),
            renamed: Vec::new(),
            posts: None,
        };
        let mut tick = TickReport::for_test(report, 0, Epoch::new(2_000));
        tick.lists = [ListKind::Followers, ListKind::Following]
            .into_iter()
            .map(|kind| TickList {
                kind,
                skipped: None,
            })
            .collect();
        tick.posts = Some(TickPosts {
            skipped: Some(PostsSkipped::ReadRecently),
        });
        let said = of_tick(&tick);
        let Said::Tick {
            account,
            pk,
            own,
            refused,
            nothing_stored,
            followers: Some(followers),
            following: Some(following),
            posts: Some(posts),
            ..
        } = &said
        else {
            panic!("{said:?}");
        };
        assert_eq!(account.as_deref(), Some("friend"));
        assert_eq!((*pk, *own), (Pk::new(7), false));
        assert!(!refused && !nothing_stored);
        assert_eq!(followers.basis, "compare");
        assert_eq!(
            (followers.total, followers.added, followers.removed),
            (120, 2, 1)
        );
        assert_eq!(followers.added_names, ["user1", "user2"]);
        assert_eq!(following.basis, "unchanged");
        assert_eq!(following.total, 98);
        assert_eq!(posts.skipped.as_deref(), Some("read_recently"));
        assert_eq!(posts.basis, None);
        let text = serde_json::to_string(&[&said]).unwrap();
        assert_eq!(read(&text), Some(vec![said]));
    }

    #[test]
    fn posts_alone_read_within_the_hour_are_neither_refused_nor_never_stored() {
        use crate::watch::{TickPosts, WatchReport};

        let report = WatchReport {
            account_pk: Pk::new(3),
            username: Some("me".into()),
            is_self: true,
            followers: None,
            following: None,
            renamed: Vec::new(),
            posts: None,
        };
        let mut tick = TickReport::for_test(report, 0, Epoch::new(2_000));
        tick.posts = Some(TickPosts {
            skipped: Some(PostsSkipped::ReadRecently),
        });
        let Said::Tick {
            account,
            pk,
            own,
            refused,
            nothing_stored,
            ..
        } = of_tick(&tick)
        else {
            panic!("a tick");
        };
        assert_eq!(
            (account.as_deref(), pk, own),
            (Some("me"), Pk::new(3), true)
        );
        assert!(!refused && !nothing_stored);

        tick.posts = Some(TickPosts {
            skipped: Some(PostsSkipped::Cooldown),
        });
        let Said::Tick {
            refused,
            nothing_stored,
            posts: Some(posts),
            ..
        } = of_tick(&tick)
        else {
            panic!("a tick");
        };
        assert!(refused && !nothing_stored);
        assert_eq!(posts.skipped.as_deref(), Some("cooldown"));
    }

    #[test]
    fn a_failure_is_kept_as_its_exit_word_and_problems_as_cannot_run() {
        let e: anyhow::Error = ExitError::new(ExitCode::RateLimited, "throttled").into();
        assert_eq!(
            failed(&e),
            Said::Failed {
                code: "rate_limited".into()
            }
        );
        assert_eq!(
            failed(&anyhow::anyhow!("anything")),
            Said::Failed {
                code: "error".into()
            }
        );
        let text = serde_json::to_string(&[Said::CannotRun]).unwrap();
        assert_eq!(text, r#"[{"kind":"cannot_run"}]"#);
        assert_eq!(read(&text), Some(vec![Said::CannotRun]));
    }

    #[test]
    fn a_kind_unknown_reads_as_nothing() {
        assert_eq!(
            read(r#"[{"kind":"cannot_run"},{"kind":"from_a_newer_snob"}]"#),
            None
        );
        assert_eq!(read("not json"), None);
        assert_eq!(read(r#"{"kind":"cannot_run"}"#), None, "an array, always");
        assert_eq!(read("[]"), Some(Vec::new()));
    }
}
