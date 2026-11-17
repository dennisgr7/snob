//! Expiring what nothing needs any more in an account's database: old
//! captures, reports too old to be news, and finished runs
//! (`snob_store::store::watch::prune`, AGENTS.md "Stored lists").
//!
//! **Every command takes part, at most once a day** ([`settle_daily`], from
//! `App::open`); the monitor settles every account's database once a run,
//! whatever its viewers did ([`settle_account`]).

use snob_core::Epoch;
use snob_store::paths::AccountPaths;
use snob_store::store::{Store, watch as store};

/// Expires what nothing needs any more: old captures, reports too old to be
/// news, and finished runs. Answers how many reports were given up on.
///
/// **Once per run, beside the queue drain**, not at the end of a comparison:
/// a report is committed only through the delivery step, so a run that ended
/// earlier (a session gone, a third party gone private) would leave a report
/// past `deliveries::MAX_AGE_SECS` sitting `pending` for ever.
pub fn settle(db: &Store, at: Epoch) -> usize {
    match store::prune(db.conn(), at) {
        Ok(swept) => {
            if swept.captures > 0 {
                tracing::debug!(
                    removed = swept.captures,
                    "expired what nothing needs any more"
                );
            }
            swept.given_up
        }
        Err(e) => {
            tracing::warn!(error = %e, "old captures could not be expired");
            0
        }
    }
}

/// Retention for every command that is not the monitor, at most once a day.
///
/// **The gate is a stored timestamp rather than a flag**, so an ordinary
/// invocation costs one `SELECT` on a table with one row and nothing else.
pub fn settle_daily(db: &Store) {
    const KEY: &str = "settled_at";
    const A_DAY: i64 = 24 * 3_600;

    let now = snob_core::clock::now();
    let last = db
        .remembered(KEY)
        .ok()
        .flatten()
        // The one row this is kept in is TEXT, so the moment is parsed back
        // out of it here, at the boundary, and nothing further in handles a
        // number.
        .and_then(|v| v.parse::<i64>().ok())
        .map_or(Epoch::default(), Epoch::new);
    if now - last < A_DAY {
        return;
    }

    let _ = settle(db, now);
    if let Err(e) = db.remember(KEY, &now.to_string()) {
        // Not worth failing an ordinary command over.
        tracing::debug!(error = %e, "when retention last ran could not be recorded");
    }
}

/// Best-effort retention for one account's database, opened here, so it needs
/// no session: a run settles every account's this way whatever its viewers did.
pub fn settle_account(paths: &AccountPaths, at: Epoch) -> usize {
    match Store::open_existing(paths) {
        Ok(Some(db)) => settle(&db, at),
        Ok(None) => 0,
        Err(e) => {
            tracing::warn!(error = %e, "the database could not be opened to settle it");
            0
        }
    }
}
