//! A crossing of one account's two lists (`snob_core::sets::Crossing`):
//! both walked as one account, **the list crossed against first, and only a
//! whole one crossed**, then the two crossed by pk. What `unfollowers`,
//! `fans` and `friends` print and the app's lists of them show; and a scan,
//! both lists whole and their five counts, as `scan` prints and the app's
//! home shows.

use anyhow::Result;
use snob_core::model::{ListKind, User};
use snob_core::sets::Crossing;

use super::{ListOutcome, ListQuery};
use crate::app::App;

/// Both lists of a crossing and what it came to.
#[derive(Debug)]
pub struct Crossed {
    pub result: Vec<User>,
    pub base_len: usize,
    pub against_len: usize,
    pub base_outcome: ListOutcome,
    pub against_outcome: ListOutcome,
}

/// The list crossed against was not read whole: every account missing from
/// it would show up in the result without deserving to, so there is none.
#[derive(Debug)]
pub struct Incomplete {
    /// The crossing it was read for; `None` for a scan, which crosses both.
    pub crossing: Option<Crossing>,
    pub kind: ListKind,
    pub outcome: ListOutcome,
}

impl std::fmt::Display for Incomplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = self.crossing.map_or("crossings", Crossing::as_str);
        write!(
            f,
            "the {} list could not be read in full, so its {what} would be wrong",
            self.kind
        )
    }
}

impl std::error::Error for Incomplete {}

/// Walks both lists of `crossing` as `app`, through `walk` (the engine's
/// `list`, or a front's wrapping of it that shows the walk), and crosses
/// them. An incomplete list crossed against is [`Incomplete`]; two lists not
/// read at the same moment are refused as `cooldown::check_same_moment_dated`
/// refuses them.
pub async fn cross(
    app: &mut App,
    query: &ListQuery,
    crossing: Crossing,
    mut walk: impl AsyncFnMut(&mut App, &ListQuery, ListKind) -> Result<(Vec<User>, ListOutcome)>,
) -> Result<Crossed> {
    let (against, against_outcome) = walk(app, query, crossing.against()).await?;
    if !against_outcome.is_complete() {
        return Err(Incomplete {
            crossing: Some(crossing),
            kind: crossing.against(),
            outcome: against_outcome,
        }
        .into());
    }
    let (base, base_outcome) = walk(app, query, crossing.base()).await?;
    super::cooldown::check_same_moment_dated(&against_outcome, &base_outcome, || {
        app.held().ok().flatten().map(|held| held.until_ms)
    })?;
    Ok(Crossed {
        result: crossing.of(&base, &against),
        base_len: base.len(),
        against_len: against.len(),
        base_outcome,
        against_outcome,
    })
}

/// Both lists of one account, read whole, as `scan` reads them.
#[derive(Debug)]
pub struct Scanned {
    pub followers: Vec<User>,
    pub following: Vec<User>,
    pub followers_outcome: ListOutcome,
    pub following_outcome: ListOutcome,
}

/// The five counts of a scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub followers: usize,
    pub following: usize,
    pub friends: usize,
    pub fans: usize,
    pub unfollowers: usize,
}

impl Scanned {
    /// The five counts of the accounts `allows` keeps ([`counts`]).
    pub fn counts(&self, allows: impl Fn(&User) -> bool) -> Counts {
        counts(&self.followers, &self.following, allows)
    }
}

/// The five counts of the accounts `allows` keeps, crossed by pk first and
/// filtered after, so each is what its own command answers with the same
/// filter.
pub fn counts(followers: &[User], following: &[User], allows: impl Fn(&User) -> bool) -> Counts {
    let count = |crossing: Crossing| {
        let (base, against) = match crossing.base() {
            ListKind::Followers => (followers, following),
            ListKind::Following => (following, followers),
        };
        crossing
            .of(base, against)
            .iter()
            .filter(|u| allows(u))
            .count()
    };
    let (friends, fans, unfollowers) = (
        count(Crossing::Friends),
        count(Crossing::Fans),
        count(Crossing::Unfollowers),
    );
    Counts {
        followers: fans + friends,
        following: unfollowers + friends,
        friends,
        fans,
        unfollowers,
    }
}

/// Walks both lists of one account as `app`, followers first so an
/// incomplete list is found out before the second walk is spent; **either
/// read short is [`Incomplete`]**, since every crossing reads both.
pub async fn scan(
    app: &mut App,
    query: &ListQuery,
    mut walk: impl AsyncFnMut(&mut App, &ListQuery, ListKind) -> Result<(Vec<User>, ListOutcome)>,
) -> Result<Scanned> {
    let whole = |kind: ListKind, outcome: ListOutcome| -> Result<ListOutcome> {
        if outcome.is_complete() {
            Ok(outcome)
        } else {
            Err(Incomplete {
                crossing: None,
                kind,
                outcome,
            }
            .into())
        }
    };
    let (followers, outcome) = walk(app, query, ListKind::Followers).await?;
    let followers_outcome = whole(ListKind::Followers, outcome)?;
    let (following, outcome) = walk(app, query, ListKind::Following).await?;
    let following_outcome = whole(ListKind::Following, outcome)?;
    super::cooldown::check_same_moment_dated(&followers_outcome, &following_outcome, || {
        app.held().ok().flatten().map(|held| held.until_ms)
    })?;
    Ok(Scanned {
        followers,
        following,
        followers_outcome,
        following_outcome,
    })
}
