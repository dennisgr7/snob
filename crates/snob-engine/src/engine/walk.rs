//! The walk itself: opening a snapshot, filling it page by page, closing it.

use anyhow::Result;
use snob_core::Epoch;
use snob_core::clock::now;
use snob_core::model::{ListKind, User};
use snob_ig::pace::Pace;
use snob_ig::pager::{ListRequest, ListWalker, OverBudget, WalkError};
use snob_store::store::{Store, snapshots};

use crate::app::App;
use crate::engine::target::Target;
use crate::engine::{ListOutcome, ListQuery, Provenance};

/// About how many accounts one request brings back: the most a page is asked
/// for, which the web app gets eight to twelve of.
const ACCOUNTS_PER_REQUEST: u64 = snob_ig::pace::ACCOUNTS_PER_PAGE as u64;

/// About how many pages a list of `accounts` is: at least one, since an
/// empty list is still asked for.
pub fn pages_to_walk(accounts: u64) -> u64 {
    accounts.div_ceil(ACCOUNTS_PER_REQUEST).max(1)
}

/// The most pages a list of `accounts` may take: the app gets as few as
/// [`snob_ig::pace::ACCOUNTS_PER_PAGE_FEWEST`] of the twelve it asks for.
pub fn pages_to_walk_at_most(accounts: u64) -> u64 {
    accounts
        .div_ceil(u64::from(snob_ig::pace::ACCOUNTS_PER_PAGE_FEWEST))
        .max(1)
}

/// About how many requests walking `pages` takes: from the browser, the
/// navigation that opens the list, and each page followed by its `show_many`,
/// as the app sends them; without it, one a page.
pub fn requests_for_pages(pages: u64, browser: bool) -> u64 {
    match pages {
        0 => 0,
        pages if browser => 1 + 2 * pages,
        pages => pages,
    }
}

/// About how many requests walking a list of `accounts` takes, from the
/// browser or not as `browser` says.
pub fn requests_to_walk(accounts: u64, browser: bool) -> u64 {
    requests_for_pages(pages_to_walk(accounts), browser)
}

/// Walks the list, resuming an interrupted one when there is a usable one.
pub async fn fetch(
    app: &mut App,
    args: &ListQuery,
    kind: ListKind,
    target: &Target,
    declared: Option<u64>,
) -> Result<(Vec<User>, ListOutcome)> {
    let opened = open_snapshot(app, args, kind, target, declared)?;
    let id = opened.id;

    let over_budget = match choose_over_budget(app, args, declared, opened.already_stored) {
        Ok(chosen) => chosen,
        Err(e) => {
            let_go(app.db(), id);
            return Err(e);
        }
    };

    // Somebody else's lists give up sooner on a network failure.
    let pace = if target.is_self {
        Pace::default()
    } else {
        Pace::third_party()
    };

    let request = ListRequest {
        pk: target.pk,
        // Empty when the name was never learned, which the pager documents
        // as allowed and simply leaves the referer generic. Never the numeric
        // id: `https://www.instagram.com/42/followers/` is a page no browser
        // is ever on.
        username: target.username.as_deref().unwrap_or_default(),
        direction: kind.into(),
        from: opened.cursor.as_deref(),
        estimated: declared,
        max_pages: args.max_pages,
        already_stored: opened.already_stored,
        over_budget,
    };

    // The monitor's question between two pages, if this is the monitor.
    let stop_between_pages = app.stop_between_pages();
    let (client, db, progress) = app.parts();
    // One store for two callers that never overlap: the pages are saved
    // between requests, and the claim is kept alive only while the walk sleeps
    // on the day's accounts, so other processes see it asleep rather than dead.
    let db = std::cell::RefCell::new(db);
    let still_wanted = || Ok(snapshots::keep_claim(&db.borrow(), id)?);
    let stopping = || {
        let stop = stop_between_pages.is_some_and(|stop| stop());
        if stop {
            progress.warn(crate::sentences::BATTERY_CRITICAL_STOP);
        }
        stop
    };
    let walker = ListWalker::new(client)
        .with_pace(pace)
        .with_heartbeat(&still_wanted)
        .with_stop(&stopping);
    // The machine is kept from sleeping on its own while the walk reads, and
    // let go while it waits for the day's accounts (`power::KeepAwake`).
    let mut awake =
        crate::power::KeepAwake::new("snob is reading an Instagram list", client.is_live());

    let summary = match walker
        .walk(
            request,
            |page, _| {
                let batch: Vec<User> = page.users.iter().map(User::from).collect();
                Ok(
                    snapshots::save_page(&mut db.borrow_mut(), id, &batch, page.next_cursor())?
                        .added,
                )
            },
            |event| {
                awake.follow(&event);
                progress.event(&event);
            },
        )
        .await
    {
        Ok(summary) => summary,
        Err(error) => {
            let_go(&db.borrow(), id);
            return Err(walk_failed(app, error, target, kind));
        }
    };

    snapshots::close(app.db().conn(), id, summary.reason)?;

    // Best-effort: the list just read is the answer, swept or not.
    if summary.reason.yields_complete_list()
        && let Err(e) = snapshots::prune(app.db().conn(), now())
    {
        tracing::debug!(error = %e, "old captures could not be expired");
    }

    // Asked of the store rather than inferred from the stop reason, and asked
    // with the same predicate the next run will use — so "it can be continued"
    // means the next run really would, cursor and resume window included.
    let resumable = snapshots::is_resumable(app.db().conn(), target.pk, kind)?;

    // What Instagram said, said out loud: the walker keeps the error next to
    // the stop reason, and a checkpoint carries the address that clears it.
    let stopped_by = summary.error.map(|error| {
        app.warn(&crate::sentences::what_instagram_said(&error));
        crate::exit::from_ig_error(&error)
    });

    Ok((
        snapshots::members(app.db().conn(), id)?,
        ListOutcome {
            provenance: Provenance::Walked,
            reason: summary.reason,
            // Filled in by `engine::list` from the pacer.
            requests: 0,
            started_at: opened.started_at,
            taken_at: now(),
            account_pk: target.pk,
            snapshot_id: id,
            stopped_by,
            resumable,
            // A walk reads the list as it is, own writes included.
            own_changes: 0,
        },
    ))
}

/// What to do if the day's accounts run out mid-walk, and saying so first when
/// it will matter.
///
/// **It pauses unless `--same-day` said otherwise, and it asks nothing.**
fn choose_over_budget(
    app: &App,
    args: &ListQuery,
    declared: Option<u64>,
    already_stored: usize,
) -> Result<OverBudget> {
    if let Some(chosen) = args.over_budget {
        return Ok(chosen);
    }
    let Some(declared) = declared else {
        return Ok(OverBudget::Pause);
    };
    if !app.client().is_live() {
        return Ok(OverBudget::Pause);
    }
    let needed = declared.saturating_sub(already_stored as u64);
    let left = u64::from(app.client().pacer().accounts_left()?);
    if needed > left {
        app.warn(&crate::sentences::over_the_day(needed, left));
        app.warn(crate::sentences::PAUSING_BY_DEFAULT);
    }
    Ok(OverBudget::Pause)
}

/// What a walk that stopped on an error is reported as.
fn walk_failed(app: &App, error: WalkError, target: &Target, kind: ListKind) -> anyhow::Error {
    match error {
        // Reachable only when the cooldown lands between the check in
        // `engine::list` and the walk: set by another process, or by a 429 on
        // the counter poll just before it (`freshness`).
        WalkError::Cooldown { until_ms, .. } => {
            let held = app.held().ok().flatten().unwrap_or(crate::app::Held {
                until_ms,
                braked: Vec::new(),
            });
            crate::sentences::refuse_cooldown_mid_walk(&held)
        }
        // A crossing walks two lists in the same run, so "the walk failed" would
        // not say which one stopped.
        error => {
            let who = crate::app::target_label(target.username.as_deref());
            anyhow::Error::new(error).context(format!("could not read {who}'s {kind} list"))
        }
    }
}

/// Lets go of the capture after a failure, so the next run resumes it rather
/// than paying for its pages again; see `snapshots::release`.
fn let_go(db: &Store, id: i64) {
    if let Err(e) = snapshots::release(db.conn(), id) {
        tracing::debug!(error = %e, "could not let go of the capture");
    }
}

/// The snapshot this walk will fill, and what is already known about it.
struct Opened {
    id: i64,
    /// Read from the row rather than taken now, so a resumed walk keeps the
    /// moment its **first** page was asked for.
    started_at: Epoch,
    cursor: Option<String>,
    already_stored: usize,
}

/// Continues an interrupted walk when one is still usable, and starts a fresh
/// snapshot otherwise.
fn open_snapshot(
    app: &App,
    args: &ListQuery,
    kind: ListKind,
    target: &Target,
    declared: Option<u64>,
) -> Result<Opened> {
    let conn = app.db().conn();

    let pending = if args.no_resume {
        None
    } else {
        snapshots::resumable(conn, target.pk, kind)?
    };

    if let Some(snapshot) = pending {
        return Ok(Opened {
            id: snapshot.id,
            started_at: snapshot.started_at,
            cursor: snapshot.next_cursor,
            already_stored: snapshot.member_count as usize,
        });
    }

    // Beginning beside it would pay again for every page it has stored, and
    // store two captures of one moment.
    if snapshots::walked_elsewhere(conn, target.pk, kind)? {
        return Err(crate::sentences::walked_elsewhere(
            target.username.as_deref(),
            kind,
        ));
    }
    snapshots::delete_partials(conn, target.pk, kind)?;
    let fresh = snapshots::begin(conn, target.pk, kind, declared)?;
    Ok(Opened {
        id: fresh.id,
        started_at: fresh.started_at,
        cursor: None,
        already_stored: 0,
    })
}
