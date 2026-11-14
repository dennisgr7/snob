//! Whether a stored list still answers the question, and the one request that
//! finds out.

use anyhow::Result;
use snob_core::clock::now;
use snob_core::model::{ListKind, User};
use snob_ig::error::IgError;
use snob_store::store::{accounts, snapshots};

use crate::app::App;
use crate::engine::target::Target;
use crate::engine::{ListOutcome, ListQuery, Provenance, walk};

/// Polls, compares, and either serves what is stored or walks.
pub async fn decide_and_fetch(
    app: &mut App,
    args: &ListQuery,
    kind: ListKind,
    target: &Target,
    stored: Option<snapshots::Snapshot>,
) -> Result<(Vec<User>, ListOutcome)> {
    let declared = match poll(app, target, kind).await {
        Ok(counter) => counter,
        // A security check or a dead session is the reader's to clear, and
        // nothing stored answers it: said as itself, with its own exit code
        // and, for a challenge, its link.
        Err(e)
            if e.downcast_ref::<IgError>().is_some_and(|e| {
                e.invalidates_session() || matches!(e, IgError::PageNotReady(_))
            }) =>
        {
            return Err(e);
        }
        Err(e) => {
            // Walking the whole list right when Instagram is already having
            // trouble is the worst possible reaction, so anything stored wins.
            if let Some(snapshot) = &stored
                && !args.refresh
            {
                app.warn(&crate::sentences::poll_failed_serving_stored(&e));
                // Served, but with nothing said about whether it is still true.
                return super::serve_stored(app, snapshot, Provenance::PollFailed);
            }
            app.warn(&crate::sentences::poll_failed(&e));
            None
        }
    };

    if !args.refresh
        && let Some(snapshot) = &stored
        && is_still_good(snapshot, declared, args.max_age)
    {
        // The counter was polled just now and had not moved, so this describes
        // the account as it is however old the snapshot is.
        return super::serve_stored(app, snapshot, Provenance::CounterVerified);
    }

    // The counter moved, or the capture aged out; either way the list has
    // stopped answering.
    if !args.refresh
        && let Some(gap) = args.walk_at_most_every
        && let Some(snapshot) = &stored
        && snapshot.taken_at.unwrap_or_default() + gap > now()
    {
        return super::serve_stored(app, snapshot, Provenance::WalkedRecently);
    }

    walk::fetch(app, args, kind, target, declared).await
}

/// The two conditions a stored list has to meet, both of them necessary.
fn is_still_good(
    snapshot: &snapshots::Snapshot,
    declared: Option<u64>,
    max_age: std::time::Duration,
) -> bool {
    // Saturating, so the longest `--max-age` anyone can write reuses rather
    // than walks.
    let fresh = snapshot.taken_at.unwrap_or_default() + max_age >= now();
    // `None` never counts as unchanged: not knowing the counter is not the
    // same as knowing it stayed put.
    let unchanged = declared.is_some() && declared == snapshot.declared_count;
    fresh && unchanged
}

/// A stored list that still answers, if there is one — for a caller that
/// already holds today's counter and wants to know whether opening the list
/// will cost anything before asking anybody about spending.
pub fn fresh_members(
    app: &App,
    account: snob_core::Pk,
    kind: ListKind,
    declared: Option<u64>,
    max_age: std::time::Duration,
) -> Result<Option<Vec<User>>> {
    let Some(snapshot) = snapshots::latest_complete(app.db().conn(), account, kind)? else {
        return Ok(None);
    };
    if !is_still_good(&snapshot, declared, max_age) {
        return Ok(None);
    }
    Ok(Some(snapshots::members(app.db().conn(), snapshot.id)?))
}

/// Reads the counters.
async fn poll(app: &mut App, target: &Target, kind: ListKind) -> Result<Option<u64>> {
    let counters = match target.counters {
        Some(counters) => counters,
        None => {
            let Some(counters) = app
                .client()
                .counters(target.pk, target.username.as_deref())
                .await?
            else {
                return Ok(None);
            };
            // One answer carries both counters, so the other list of a crossing
            // does not have to ask again.
            app.remember_counters(counters);
            counters
        }
    };

    accounts::record_poll(
        app.db().conn(),
        target.pk,
        counters.followers,
        counters.following,
    )?;
    Ok(counters.of(kind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Epoch;

    fn snapshot(taken_at: Epoch, declared: Option<u64>) -> snapshots::Snapshot {
        crate::engine::stored_snapshot(taken_at, taken_at, declared)
    }

    const SIX_HOURS: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

    #[test]
    fn fresh_and_unmoved_is_the_only_case_that_is_reused() {
        let recent = snapshot(now(), Some(300));
        assert!(is_still_good(&recent, Some(300), SIX_HOURS));
    }

    #[test]
    fn a_moved_counter_is_walked_however_fresh_the_list_is() {
        let recent = snapshot(now(), Some(300));
        assert!(!is_still_good(&recent, Some(301), SIX_HOURS));
    }

    #[test]
    fn an_old_list_is_walked_however_still_the_counter_is() {
        let old = snapshot(
            now() - SIX_HOURS - std::time::Duration::from_secs(1),
            Some(300),
        );
        assert!(!is_still_good(&old, Some(300), SIX_HOURS));
    }

    /// The longest maximum age anyone can write must not mean the shortest.
    ///
    /// `duration::parse` refuses what will not fit in an `i64`, and a
    /// `Duration` built any other way still saturates instead of wrapping: a
    /// saturated bound reuses everything rather than walking everything.
    #[test]
    fn an_absurd_maximum_age_reuses_rather_than_walks() {
        let ancient = snapshot(Epoch::default(), Some(300));
        let forever = std::time::Duration::from_secs(u64::MAX);

        assert!(
            is_still_good(&ancient, Some(300), forever),
            "the longest age anyone can write is the one that expires nothing"
        );
    }

    /// Not knowing the counter is not the same as knowing it stayed put. If an
    /// unknown counted as unchanged, a failed poll would freeze the cache.
    #[test]
    fn an_unknown_counter_is_never_taken_as_unchanged() {
        let recent = snapshot(now(), Some(300));
        assert!(!is_still_good(&recent, None, SIX_HOURS));

        let never_counted = snapshot(now(), None);
        assert!(!is_still_good(&never_counted, None, SIX_HOURS));
    }
}
