//! What stored captures say, which a run records and which [`from_store`]
//! reads without sending anything.

use anyhow::Result;
use snob_core::Pk;
use snob_core::model::ListKind;
use snob_core::watch::{Basis, ListDiff, Rename};

use super::{App, Compared, ListReport, WatchReport, accounts, snapshots, store, target, users};

/// Reads the report without touching the network, and without recording it.
///
/// **It takes `&App`, and that is the guard rather than a convention.**
pub fn from_store(app: &App, typed: Option<&str>) -> Result<WatchReport> {
    Ok(look(app, typed)?.1.report)
}

/// Reads the report and records having made it, the way a tick does.
#[doc(hidden)]
pub fn record_from_store(app: &mut App, typed: Option<&str>) -> Result<WatchReport> {
    let (pk, compared) = look(app, typed)?;

    let (_, db, _) = app.parts();
    store::commit_report(
        db,
        pk,
        &compared.marks,
        snob_core::clock::now(),
        compared.rename_cursor,
        &compared.renames_sent,
        None,
        None,
    )?;

    Ok(compared.report)
}

/// The comparison both entry points make, and what committing it would write.
fn look(app: &App, typed: Option<&str>) -> Result<(Pk, Compared)> {
    let (pk, username) = resolve(app, typed)?;

    // From the view, so an interrupted walk cannot become a basis for
    // comparison.
    let mut usable = Vec::new();
    for kind in [ListKind::Followers, ListKind::Following] {
        if let Some(latest) = snapshots::latest_complete(app.db().conn(), pk, kind)? {
            usable.push((kind, latest.id));
        }
    }

    // Read before the comparison, so a rename filed while it runs lands on the
    // next report's side rather than being marked as said without having been
    // said.
    let head = store::history_head(app.db().conn())?;
    let mut compared = compare(app, pk, &usable, head)?;
    compared.report.username = username;
    Ok((pk, compared))
}

/// Compares each named capture against its receipt.
///
/// `head` is the rename history's newest row, read by the caller **before** this
/// runs: a rename filed while the comparison is in progress then lands on the
/// next report's side rather than being filed as said without having been said.
pub(super) fn compare(
    app: &App,
    pk: Pk,
    usable: &[(ListKind, i64)],
    head: i64,
) -> Result<Compared> {
    let mut followers = None;
    let mut following = None;
    for &(kind, snapshot_id) in usable {
        let report = list_report(app, pk, kind, snapshot_id)?;
        match kind {
            ListKind::Followers => followers = report,
            ListKind::Following => following = report,
        }
    }

    // Renames are looked for among the members of **every list this run
    // verified**: somebody only in `following` is exactly the `unfollowers`
    // set.
    let verified: Vec<&ListReport> = [followers.as_ref(), following.as_ref()]
        .into_iter()
        .flatten()
        .filter(|report| report.verified())
        .collect();

    // What this run announces, and the history rows behind them.
    //
    // **Already-sent is asked per row, not per watermark** (`007_renames_sent`).
    let mut renamed: Vec<Rename> = Vec::new();
    let mut announcing: Vec<i64> = Vec::new();
    if !verified.is_empty() {
        let since = store::rename_cursor(app.db().conn(), pk)?;
        let already = store::renames_already_sent(app.db().conn(), pk)?;
        let mut seen = std::collections::HashSet::new();
        for report in &verified {
            for rename in
                store::renames_since(app.db().conn(), report.basis.mark_to(), since, head)?
            {
                if already.contains(&rename.history_id) {
                    continue;
                }
                if seen.insert(rename.pk) {
                    announcing.push(rename.history_id);
                    renamed.push(rename);
                }
            }
        }
    }

    // Whether the cursor may move, which is a different question from whether
    // there was anything to report.
    //
    // Only when **every list this account has a capture of** is accounted for.
    //
    // - A baseline accounts for its list only on the account's **first**
    //   report: the monitor starts now, and history from before it is not news.
    let verified_kinds: Vec<ListKind> = verified.iter().map(|report| report.kind).collect();
    // The lists this report actually spoke about: what the marks name, and the
    // one answer to "which lists did this run report on" that the cursor below
    // reads too.
    //
    // **The cursor hangs off this list and not off a second copy of it.**
    let marks: Vec<(ListKind, i64)> = [followers.as_ref(), following.as_ref()]
        .into_iter()
        .flatten()
        .map(|report| (report.kind, report.basis.mark_to()))
        .collect();

    let first_report = store::mark(app.db().conn(), pk, ListKind::Followers)?.is_none()
        && store::mark(app.db().conn(), pk, ListKind::Following)?.is_none();

    let mut every_list_accounted_for = true;
    for kind in [ListKind::Followers, ListKind::Following] {
        if verified_kinds.contains(&kind) {
            continue;
        }
        if !snapshots::any_capture(app.db().conn(), pk, kind)? {
            continue; // nothing was ever captured, so nothing was missed
        }
        if first_report && marks.iter().any(|&(marked, _)| marked == kind) {
            continue; // the seeding baseline
        }
        every_list_accounted_for = false;
    }

    // Nothing filed after `head` is inside the window: `renames_since` enforces
    // it.
    let covered = (!marks.is_empty() && every_list_accounted_for).then_some(head);

    Ok(Compared {
        report: WatchReport {
            account_pk: pk,
            username: users::name(app.db().conn(), pk)?,
            is_self: app.viewer().pk == pk,
            followers,
            following,
            renamed,
            posts: None,
        },
        marks,
        rename_cursor: covered,
        renames_sent: announcing,
    })
}

fn list_report(app: &App, pk: Pk, kind: ListKind, snapshot_id: i64) -> Result<Option<ListReport>> {
    let conn = app.db().conn();
    // `find_usable`, not `find`: an id that names a walk which stopped short
    // answers `None` here rather than handing back a capture with accounts
    // missing from it.
    let Some(latest) = snapshots::find_usable(conn, snapshot_id)? else {
        return Ok(None);
    };

    let mark = store::mark(conn, pk, kind)?;
    // A receipt whose capture has been pruned carries no baseline, so it is
    // handed to `decide` as the absence it is and the next report starts over.
    let Some(basis) = Basis::decide(mark.and_then(|m| m.snapshot_id), Some(latest.id)) else {
        return Ok(None);
    };

    let diff = match basis {
        // Nothing is reported and nothing is read: a baseline has no earlier
        // capture, and an unchanged list has nothing new in it.
        Basis::Baseline { .. } | Basis::Unchanged { .. } => ListDiff::default(),
        Basis::Compare { before, after } => {
            // The baseline is read through `find_usable` too, not just the
            // newer capture.
            //
            // It takes a second process's `prune` landing in between, **and the
            // three reads are one moment for that reason**: inside one read
            // transaction (rolled back when it drops, having written nothing)
            // the capture found is the capture whose members are read.
            let moment = conn.unchecked_transaction()?;
            let Some(_) = snapshots::find_usable(&moment, before)? else {
                return Ok(None);
            };
            ListDiff::between(
                &snapshots::members(&moment, before)?,
                &snapshots::members(&moment, after)?,
            )
        }
    };

    Ok(Some(ListReport {
        kind,
        basis,
        since: mark.map(|m| m.compared_at),
        until: latest.taken_at.unwrap_or_default(),
        diff,
        total: latest.member_count as usize,
    }))
}

/// Which account this is about, from storage alone.
fn resolve(app: &App, typed: Option<&str>) -> Result<(Pk, Option<String>)> {
    let Some(typed) = typed else {
        let viewer = app.viewer();
        return Ok((viewer.pk, viewer.username.clone()));
    };

    let name = target::clean(typed);
    let Some(pk) = accounts::find_pk_by_username(app.db().conn(), name)? else {
        return Err(crate::sentences::refuse_never_walked(name));
    };

    Ok((pk, users::name(app.db().conn(), pk)?))
}
