//! What an automation's action reads, other than the lists and posts the
//! monitor's tick reads: unfollowers, fans and friends (two lists crossed),
//! the friends two accounts have in common (four), who of the people you
//! follow follow an account (its "Followed by", a page of twelve at a time),
//! and its profile picture.
//!
//! **Every list comes out of `engine::list`**, so its consent (the
//! automation's stored yes, as `watch.toml`'s), its pace, the day's budget
//! and a cooldown hold as for any command, and a list crossed against
//! another is refused when it was not read whole, or not at the same moment.
//!
//! **What a "changes" filter compares against is the accounts the last run
//! reported** (`store::automation_marks`), moved only by [`commit`], once
//! the report is made.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use snob_core::automation::{Planned, Read};
use snob_core::model::{ListKind, User};
use snob_core::{Epoch, Pk, sets};
use snob_store::store::{automation_marks, users};

use crate::app::App;
use crate::engine::{self, ListOutcome, ListQuery, Provenance};
use crate::exit::{ExitCode, ExitError};

/// What one action found.
#[derive(Debug, Clone)]
pub struct Found {
    pub node: u32,
    pub what: Read,
    /// The account it was about; `None` for the viewer's own.
    pub username: Option<String>,
    /// The accounts it found that its filters let through.
    pub accounts: Vec<User>,
    /// How many it found before the filters.
    pub total: usize,
    /// What changed since the last run, when a filter asks for it.
    pub changes: Option<Changes>,
    /// When the lists it was worked out from were read, when that was not in
    /// this run (stored, their counters unmoved or unreadable).
    pub stored_since: Option<Epoch>,
    pub picture: Option<SavedPicture>,
}

/// What came and went since the last run's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changes {
    /// The first run: what it found is the baseline, nothing is said.
    Baseline,
    Since {
        since: Epoch,
        added: Vec<User>,
        removed: Vec<User>,
    },
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Baseline => true,
            Self::Since { added, removed, .. } => added.is_empty() && removed.is_empty(),
        }
    }
}

/// A profile picture, saved or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedPicture {
    /// Where it is, when the action saves pictures.
    pub path: Option<PathBuf>,
    /// Whether it was not there before: a new picture, or the first run.
    pub new: bool,
}

impl Found {
    /// Whether it has news to report: a change, a new picture, or, without a
    /// "changes" filter, any account at all.
    pub fn has_news(&self) -> bool {
        match (&self.changes, &self.picture) {
            (_, Some(picture)) => picture.new,
            (Some(changes), None) => !changes.is_empty(),
            (None, None) => !self.accounts.is_empty(),
        }
    }
}

/// The question every list of an action asks: the monitor's, so an
/// automation spends what `snob watch` spends.
fn query(target: Option<String>, yes: bool) -> ListQuery {
    ListQuery {
        target,
        yes,
        refresh: false,
        cache: false,
        max_age: engine::DEFAULT_MAX_AGE,
        no_resume: false,
        max_pages: None,
        over_budget: Some(snob_ig::pager::OverBudget::Pause),
        walk_at_most_every: Some(engine::DEFAULT_MAX_AGE),
    }
}

/// Reads what `planned` asks for, as `app`'s viewer, for the automation
/// `automation`. Lists and posts are the monitor's (`engine::watch::tick`),
/// refused here.
pub async fn read(app: &mut App, automation: &str, planned: &Planned) -> Result<Found> {
    let username = planned
        .account
        .name()
        .map(|name| engine::target::clean(name).to_string());
    let yes = planned.account.consent.is_some();
    let mut stored_since = None;
    let mut picture = None;
    let found = match &planned.what {
        Read::Lists | Read::Posts { .. } => {
            return Err(anyhow!("lists and posts are read by the monitor's tick"));
        }
        Read::Unfollowers => {
            let (base, against, since) =
                crossed(app, &query(username.clone(), yes), ListKind::Following).await?;
            stored_since = since;
            sets::difference(&base, &against)
        }
        Read::Fans => {
            let (base, against, since) =
                crossed(app, &query(username.clone(), yes), ListKind::Followers).await?;
            stored_since = since;
            sets::difference(&base, &against)
        }
        Read::Friends => {
            let (base, against, since) =
                crossed(app, &query(username.clone(), yes), ListKind::Followers).await?;
            stored_since = since;
            sets::intersection(&base, &against)
        }
        Read::FriendsInCommon => {
            let name = username.clone().ok_or_else(about_another)?;
            let (followers, following, theirs) =
                crossed(app, &query(Some(name), yes), ListKind::Followers).await?;
            let their_friends = sets::intersection(&followers, &following);
            let (followers, following, mine) =
                crossed(app, &query(None, false), ListKind::Followers).await?;
            stored_since = theirs.into_iter().chain(mine).min();
            sets::intersection(&their_friends, &sets::intersection(&followers, &following))
        }
        Read::KnownFollowers => {
            let name = username.clone().ok_or_else(about_another)?;
            known_followers(app, &name).await?
        }
        Read::Picture { save_to } => {
            picture = Some(save_picture(app, username.as_deref(), save_to.as_ref()).await?);
            Vec::new()
        }
    };
    let total = found.len();
    let accounts = planned.only.apply(found);
    let changes = if planned.only.changes() && planned.what.finds_accounts() {
        Some(changes_since(app, automation, planned.node, &accounts)?)
    } else {
        None
    };
    Ok(Found {
        node: planned.node,
        what: planned.what.clone(),
        username,
        accounts,
        total,
        changes,
        stored_since,
        picture,
    })
}

fn about_another() -> anyhow::Error {
    anyhow!("this action is about another account, and names none")
}

/// Both lists of an account, the one crossed against read first and refused
/// unless whole, and the two refused unless of the same moment: the base
/// list, the other, and when they were read when not in this run.
async fn crossed(
    app: &mut App,
    query: &ListQuery,
    base: ListKind,
) -> Result<(Vec<User>, Vec<User>, Option<Epoch>)> {
    let against_kind = match base {
        ListKind::Followers => ListKind::Following,
        ListKind::Following => ListKind::Followers,
    };
    let (against, against_outcome) = engine::list(app, query, against_kind).await?;
    whole(against_kind, &against_outcome)?;
    let walked = engine::list(app, query, base).await;
    app.progress().finish();
    let (users, base_outcome) = walked?;
    whole(base, &base_outcome)?;
    engine::cooldown::check_same_moment_dated(&against_outcome, &base_outcome, || {
        app.held().ok().flatten().map(|held| held.until_ms)
    })?;
    let since = [&against_outcome, &base_outcome]
        .iter()
        .filter(|outcome| !matches!(outcome.provenance, Provenance::Walked))
        .map(|outcome| outcome.taken_at)
        .min();
    Ok((users, against, since))
}

/// Refuses a list that was not read whole: crossed, every account missing
/// from it would be reported as what it is not.
fn whole(kind: ListKind, outcome: &ListOutcome) -> Result<()> {
    if outcome.is_complete() {
        return Ok(());
    }
    Err(ExitError::new(
        outcome.exit_code(),
        format!(
            "the {} list was not read whole ({}), so it is not crossed with the other",
            kind.as_str(),
            outcome.reason.as_str()
        ),
    )
    .into())
}

/// The people you follow who follow `name`, as its page's "Followed by"
/// lists them: its profile, then the mutual list a page at a time.
async fn known_followers(app: &mut App, name: &str) -> Result<Vec<User>> {
    if let Some(held) = app.held()? {
        return Err(ExitError::new(
            ExitCode::RateLimited,
            format!(
                "nothing is sent until {}: Instagram pushed back",
                crate::sentences::stored_on(Epoch::new(held.until_ms.get() / 1000))
            ),
        )
        .into());
    }
    let info = app.profile_named(name, None).await?;
    let count = info.mutual.as_ref().map_or(0, |mutual| mutual.count);
    let mutual =
        crate::profile::mutuals(app.client(), info.id, &info.username, count, Vec::new()).await?;
    Ok(mutual.people)
}

/// The account's profile picture, at the best size the web serves, saved in
/// `save_to` as `<name>-<digest>`: a picture already there is not new.
async fn save_picture(
    app: &mut App,
    name: Option<&str>,
    save_to: Option<&PathBuf>,
) -> Result<SavedPicture> {
    if let Some(held) = app.held()? {
        return Err(ExitError::new(
            ExitCode::RateLimited,
            format!(
                "nothing is sent until {}: Instagram pushed back",
                crate::sentences::stored_on(Epoch::new(held.until_ms.get() / 1000))
            ),
        )
        .into());
    }
    let typed = match name {
        Some(name) => name.to_string(),
        None => match app.viewer().username.clone() {
            Some(own) => own,
            None => app
                .client()
                .resolve_username(app.viewer().pk)
                .await?
                .ok_or_else(|| anyhow!("your account's name is not known"))?,
        },
    };
    let info = app.profile_named(&typed, None).await?;
    if info.anonymous_picture == Some(true) {
        return Err(crate::profile::no_picture(&info.username));
    }
    let full_size = crate::profile::full_size_of(&info);
    let picture = crate::profile::picture(
        app.client(),
        info.id,
        &info.username,
        info.profile_pic_url_hd
            .clone()
            .or(info.profile_pic_url.clone()),
        full_size,
    )
    .await?;
    let Some(dir) = save_to else {
        return Ok(SavedPicture {
            path: None,
            new: false,
        });
    };
    let stem = format!(
        "{}-{}",
        snob_core::model::printable(&picture.username),
        crate::media::short_digest(&picture.bytes)
    );
    crate::files::usable_stem(&stem)?;
    let path = dir.join(format!("{stem}.{}", picture.extension()));
    if path.exists() {
        return Ok(SavedPicture {
            path: Some(path),
            new: false,
        });
    }
    std::fs::create_dir_all(dir)?;
    let mut file = crate::files::create_new(&path)?;
    std::io::Write::write_all(&mut file, &picture.bytes)?;
    file.sync_all()?;
    Ok(SavedPicture {
        path: Some(path),
        new: true,
    })
}

/// What came and went among `accounts` since the mark the last run left.
fn changes_since(app: &App, automation: &str, node: u32, accounts: &[User]) -> Result<Changes> {
    let conn = app.db().conn();
    let Some(mark) = automation_marks::get(conn, automation, node)? else {
        return Ok(Changes::Baseline);
    };
    let added = accounts
        .iter()
        .filter(|user| !mark.members.contains(&user.pk))
        .cloned()
        .collect();
    let removed = mark
        .members
        .iter()
        .filter(|pk| !accounts.iter().any(|user| user.pk == **pk))
        .map(|pk| -> Result<User> {
            let name = users::name(conn, *pk)?.unwrap_or_default();
            Ok(User::named(*pk, name))
        })
        .collect::<Result<_>>()?;
    Ok(Changes::Since {
        since: mark.taken_at,
        added,
        removed,
    })
}

/// Moves the mark of what `found` found to now, once its report is made:
/// the next run compares against it. Nothing for an action with no
/// "changes" filter.
pub fn commit(app: &App, automation: &str, found: &Found, at: Epoch) -> Result<()> {
    if found.changes.is_none() {
        return Ok(());
    }
    let members: Vec<Pk> = found.accounts.iter().map(|user| user.pk).collect();
    automation_marks::set(app.db().conn(), automation, found.node, at, &members)?;
    Ok(())
}
