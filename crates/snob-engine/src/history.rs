//! The captures of an account's lists stored on this computer, and who
//! joined, left or was renamed between two of them: what `snob snapshots`
//! and `snob changes` print, the app's history shows and an agent asks for.
//!
//! **Sends nothing and writes nothing**: the account's database is read with
//! `Store::read_existing`, which neither creates nor migrates it, so this
//! answers in a cooldown too, and no mark moves, so it is not the monitor.
//!
//! **Only whole captures are compared** (`snapshots::usable`): the accounts a
//! walk stopped short of would read as departures.

use std::collections::HashSet;

use anyhow::{Result, bail};
use snob_core::model::{ListKind, printable};
use snob_core::watch::{ListDiff, Rename};
use snob_core::{Epoch, Pk};
use snob_store::paths::AccountPaths;
use snob_store::store::{Store, accounts, snapshots, watch as watch_store};

use crate::exit::{ExitCode, ExitError};

pub use snob_store::store::snapshots::{Listed, Snapshot};

/// An account's database, read without writing, and the account asked about
/// in it.
pub struct Stored {
    store: Store,
    /// The account the captures are of: the viewer's own, or the one named.
    pub pk: Pk,
    /// The name it was asked by, cleaned; `None` for the viewer's own.
    pub named: Option<String>,
}

/// `account`'s database and the account `target` names in it, by the name
/// the store has for it, or the viewer's own; `None` when nothing of it is
/// stored here.
pub fn open(account: &AccountPaths, target: Option<&str>) -> Result<Option<Stored>> {
    let Some(store) = Store::read_existing(account)? else {
        return Ok(None);
    };
    let Some(name) = target else {
        return Ok(Some(Stored {
            store,
            pk: account.pk(),
            named: None,
        }));
    };
    let name = crate::engine::target::clean(name);
    Ok(
        accounts::find_pk_by_username(store.conn(), name)?.map(|pk| Stored {
            store,
            pk,
            named: Some(name.to_string()),
        }),
    )
}

/// `open`, refusing with `nothing_stored` when nothing is stored; the hint
/// names the command line's walk, which a front may say otherwise.
pub fn open_or_refuse(account: &AccountPaths, target: Option<&str>) -> Result<Stored> {
    open(account, target)?.ok_or_else(|| nothing_stored(target))
}

/// Nothing stored to answer from: no list of this account was ever walked
/// here, or of the account named.
pub fn nothing_stored(target: Option<&str>) -> anyhow::Error {
    let (whose, walk) = match target {
        Some(name) => {
            let name = printable(crate::engine::target::clean(name));
            (format!("@{name}"), format!("snob followers {name}"))
        }
        None => ("your account".to_string(), "snob scan".to_string()),
    };
    ExitError::new(
        ExitCode::Error,
        format!("nothing of {whose} is stored on this computer"),
    )
    .with_reason("nothing_stored")
    .with_hint(format!("a walk stores it: \"{walk}\""))
    .into()
}

impl Stored {
    /// Every capture of the account's lists, of `kind` or of both, newest
    /// first, the partial ones too.
    pub fn captures(&self, kind: Option<ListKind>) -> Result<Vec<Listed>> {
        Ok(snapshots::listed(self.store.conn(), self.pk, kind)?)
    }

    /// The members of capture `id`.
    pub fn members(&self, id: i64) -> Result<Vec<snob_core::model::User>> {
        Ok(snapshots::members(self.store.conn(), id)?)
    }

    /// The whole capture `id` of this account's lists, or the refusal saying
    /// why it cannot be compared.
    pub fn usable(&self, id: i64) -> Result<Snapshot> {
        match snapshots::find_usable(self.store.conn(), id)? {
            Some(snapshot) if snapshot.account_pk == self.pk => Ok(snapshot),
            _ => Err(ExitError::new(
                ExitCode::Error,
                format!(
                    "capture {id} is not a whole capture of this account's lists, so it cannot \
                     be compared"
                ),
            )
            .with_reason("no_such_snapshot")
            .with_hint("\"snob snapshots\" lists the captures, and which are whole")
            .into()),
        }
    }

    /// The lists to compare: the one `list` names, the one `from` or `to` is
    /// a capture of, or both.
    pub fn kinds(&self, list: Option<ListKind>, pick: &Pick) -> Result<Vec<ListKind>> {
        let id = match *pick {
            Pick::Between { from, .. } => Some(from),
            Pick::UpTo(to) => Some(to),
            Pick::Newest | Pick::Since(_) => None,
        };
        let by_id = match id {
            Some(id) => Some((id, self.usable(id)?.kind)),
            None => None,
        };
        Ok(match (list, by_id) {
            (Some(named), Some((id, by_id))) if named != by_id => {
                bail!("capture {id} is of the {by_id} list, not {named}")
            }
            (_, Some((_, kind))) | (Some(kind), None) => vec![kind],
            (None, None) => vec![ListKind::Followers, ListKind::Following],
        })
    }

    /// Compares two captures of each of `kinds`, as `pick` chooses them. A
    /// friend is in both lists and is one person: renamed once.
    pub fn compare_all(&self, kinds: &[ListKind], pick: &Pick) -> Result<Vec<Compared>> {
        let mut compared = Vec::new();
        for kind in kinds {
            compared.push(self.compare(*kind, pick)?);
        }
        let mut seen = HashSet::new();
        for one in &mut compared {
            one.renamed.retain(|rename| seen.insert(rename.pk));
        }
        Ok(compared)
    }

    /// Picks the two captures of `kind` and compares them.
    pub fn compare(&self, kind: ListKind, pick: &Pick) -> Result<Compared> {
        // Newest first.
        let whole = snapshots::usable(self.store.conn(), self.pk, kind)?;
        let mut oldest_instead = None;
        let mut alone = None;
        let between = match *pick {
            Pick::Between { from, to } => {
                let from = self.usable(from)?;
                let to = match to {
                    Some(to) => self.usable(to)?,
                    None => whole.first().cloned().unwrap_or_else(|| from.clone()),
                };
                if to.kind != from.kind {
                    bail!("captures {} and {} are of different lists", from.id, to.id);
                }
                // A capture against itself is no comparison, as below.
                if from.id == to.id {
                    alone = Some((from, Missing::Later));
                    None
                } else {
                    Some(earlier_first(from, to))
                }
            }
            Pick::UpTo(to) => {
                let to = self.usable(to)?;
                let from = whole
                    .iter()
                    .find(|older| older.taken_at < to.taken_at)
                    .cloned();
                if from.is_none() {
                    alone = Some((to.clone(), Missing::Earlier));
                }
                from.map(|from| (from, to))
            }
            Pick::Newest | Pick::Since(_) => {
                let after = whole.first().cloned();
                let before = match *pick {
                    Pick::Since(at) => {
                        match whole
                            .iter()
                            .find(|capture| capture.taken_at.is_some_and(|t| t <= at))
                        {
                            Some(capture) => Some(capture.clone()),
                            // None that old: the oldest there is, said so.
                            None => {
                                let oldest = whole.last().cloned();
                                // Said only where there is a comparison to
                                // qualify.
                                if whole.len() > 1 {
                                    oldest_instead = oldest.clone();
                                }
                                oldest
                            }
                        }
                    }
                    _ => whole.get(1).cloned(),
                };
                match (before, after) {
                    (Some(before), Some(after)) if before.id != after.id => Some((before, after)),
                    // The newest is as old as `since` reaches: nothing was
                    // taken after it, which is not the same as two never
                    // being stored.
                    (Some(newest), Some(_)) if whole.len() > 1 => {
                        alone = Some((newest, Missing::Later));
                        None
                    }
                    _ => None,
                }
            }
        };

        let Some((before, after)) = between else {
            return Ok(Compared {
                kind,
                between: None,
                diff: ListDiff::default(),
                renamed: Vec::new(),
                oldest_instead,
                alone,
            });
        };
        let diff = ListDiff::between(
            &snapshots::members(self.store.conn(), before.id)?,
            &snapshots::members(self.store.conn(), after.id)?,
        );
        let renamed = watch_store::renames_between(
            self.store.conn(),
            &[before.id, after.id],
            before.taken_at.unwrap_or_default(),
            after.taken_at.unwrap_or_default(),
        )?;
        Ok(Compared {
            kind,
            between: Some((before, after)),
            diff,
            renamed,
            oldest_instead,
            alone: None,
        })
    }
}

/// Which two captures of a list to compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The two newest whole ones.
    Newest,
    /// The newest against the newest taken at or before this moment, else
    /// the oldest stored.
    Since(Epoch),
    /// `from` against `to`, or against the newest.
    Between { from: i64, to: Option<i64> },
    /// The newest taken before `to` against `to`.
    UpTo(i64),
}

/// One list's comparison, or why there is none.
#[derive(Clone, Debug)]
pub struct Compared {
    pub kind: ListKind,
    /// The two captures, earlier first; `None` with fewer than two whole
    /// ones to compare.
    pub between: Option<(Snapshot, Snapshot)>,
    pub diff: ListDiff,
    pub renamed: Vec<Rename>,
    /// The oldest capture, compared from because none was as old as a
    /// `since` asked: a front says so beside the result.
    pub oldest_instead: Option<Snapshot>,
    /// With no comparison, the whole capture there was nothing to compare
    /// with, and on which side the other one is missing; `None` when fewer
    /// than two whole captures are stored at all.
    pub alone: Option<(Snapshot, Missing)>,
}

/// Which capture of a pair there was none of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// None was taken after it.
    Later,
    /// None was taken before it.
    Earlier,
}

fn earlier_first(a: Snapshot, b: Snapshot) -> (Snapshot, Snapshot) {
    if (b.taken_at, b.id) < (a.taken_at, a.id) {
        (b, a)
    } else {
        (a, b)
    }
}
