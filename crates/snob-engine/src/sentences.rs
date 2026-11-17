//! The sentences the engine puts in its refusals and warnings: plain text,
//! with no styling and no terminal width.

use snob_core::model::{ListKind, printable};
use snob_core::{Epoch, EpochMs, Pk};

use crate::app::{ConsentInAdvance, Held, Viewer};
use crate::engine::Provenance;
use crate::exit::{ExitCode, ExitError};
use crate::system::sync::lock;

/// What a session stored here that this computer no longer sends as is
/// (`owner::signed_out`): its account was signed out of here, by a `logout`
/// whose removal of the session failed, or a `purge`, or the account's data
/// directory went.
pub const SIGNED_OUT_HERE: &str = "the session stored was signed out on this computer (a \
     logout or a purge, or its data was removed), so nothing is sent as it";

/// The advice for a session this computer no longer sends as, which only a
/// login lets in again.
pub const SIGN_IN_AGAIN: &str = "run \"snob login\"";

/// The account this run acts as, for a failure told in JSON to name.
static ACTING_AS: std::sync::Mutex<Option<Viewer>> = std::sync::Mutex::new(None);

/// Records the account this run acts as.
pub fn act_as(viewer: Viewer) {
    set_acting_as(Some(viewer));
}

/// Records the account this run acts as, or that it acts as none.
pub fn set_acting_as(viewer: Option<Viewer>) {
    *lock(&ACTING_AS) = viewer;
}

/// The account this run acts as, once one has been resolved.
pub fn acting_as() -> Option<Viewer> {
    lock(&ACTING_AS).clone()
}

/// How to name any account to a person: `@someone`, or the id when the name has
/// not been learned yet.
pub fn label(pk: Pk, username: Option<&str>) -> String {
    match username {
        Some(name) => format!("@{}", printable(name)),
        None => format!("account {pk}"),
    }
}

/// The same rule where the absent case means the viewer rather than an account
/// whose name is not known yet.
pub fn target_label(target: Option<&str>) -> String {
    match target {
        Some(name) => format!("@{}", printable(name)),
        None => "your account".to_string(),
    }
}

/// "Aug 3 at 14:12", in the local zone like every other moment the tool prints.
pub fn stored_on(taken_at: Epoch) -> String {
    format_epoch(taken_at, "earlier")
}

/// "Aug 3, 2024", for a moment that may be years old.
pub fn dated(at: Epoch) -> String {
    chrono::DateTime::from_timestamp(at.get(), 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%b %-d, %Y")
                .to_string()
        })
        .unwrap_or_else(|| "sometime".to_string())
}

/// "Aug 4 at 16:30", from a cooldown end.
pub fn cooldown_ends_at(until_ms: EpochMs) -> String {
    format_epoch(until_ms.to_epoch(), "later")
}

/// Until when nothing may be sent, as a clause: the account's own cooldown,
/// or the brake on every account.
pub fn held_until(held: &Held) -> String {
    if held.braked.is_empty() {
        format!(
            "the account is in cooldown until {}",
            cooldown_ends_at(held.until_ms)
        )
    } else {
        brake_sentence(held.until_ms, &held.braked)
    }
}

/// The refusal of anything that would send during a cooldown: `doing` says
/// what was not done ("nothing can be sent").
pub fn refuse_held(held: &Held, doing: &str) -> anyhow::Error {
    ExitError::new(
        ExitCode::RateLimited,
        format!("{}, so {doing}", held_until(held)),
    )
    .with_cooldown_until(held.until_ms)
    .into()
}

/// The brake on every account, naming the accounts whose push-backs set it.
fn brake_sentence(until_ms: EpochMs, accounts: &[String]) -> String {
    let named = and_list(accounts).unwrap_or_else(|| "more than one account".to_string());
    format!(
        "every account is paused until {} because Instagram pushed back on {named} \
         within an hour",
        cooldown_ends_at(until_ms)
    )
}

/// A moment, for a person to read.
///
/// **The month is named rather than numbered**, because `03/08` is the eighth
/// of March to one reader and the third of August to another; naming it removes
/// the ambiguity for everybody.
///
/// **In the local zone**, because everything else the person reads is.
fn format_epoch(at: Epoch, unknown: &str) -> String {
    format_epoch_in(at, unknown, &chrono::Local)
}

pub fn format_epoch_in<Z: chrono::TimeZone>(at: Epoch, unknown: &str, zone: &Z) -> String
where
    Z::Offset: std::fmt::Display,
{
    chrono::DateTime::from_timestamp(at.get(), 0)
        .map(|t| t.with_timezone(zone).format("%b %-d at %H:%M").to_string())
        .unwrap_or_else(|| unknown.to_string())
}

/// Two stored lists too far apart to be crossed.
///
/// - A **cooldown** is waited out, so "run it again later" is true and the
///   throttling code is right.
/// - **`--offline`** is the user's own doing, and dropping it is the fix.
/// - A **failed poll** is neither.
pub fn refuse_different_moments(
    a: Provenance,
    b: Provenance,
    a_at: Epoch,
    b_at: Epoch,
) -> anyhow::Error {
    // Each arm asks the same question of the same pair, so each one asks it the
    // same way.
    let either = |wanted: Provenance| a == wanted || b == wanted;
    // `stored_moments` names the one an agent cannot act on as written: its
    // server passed the `--offline` (a stored list, a later page no longer
    // held), so it is told in its own tools' words (`mcp::answer`).
    let (code, hint, reason) = if either(Provenance::Cooldown) {
        (
            ExitCode::RateLimited,
            "Run it again once the cooldown lifts.",
            None,
        )
    } else if either(Provenance::CacheFlag) {
        (
            ExitCode::Error,
            "Run it again without --offline, so both lists are checked against the account.",
            Some("stored_moments"),
        )
    } else {
        (
            ExitCode::Error,
            "Instagram could not be reached to check either list. Run it again in a while.",
            None,
        )
    };

    let refusal = ExitError::new(
        code,
        format!(
            "the two stored lists are from different moments ({} and {}), so crossing \
             them would invent results.",
            stored_on(a_at),
            stored_on(b_at)
        ),
    )
    .with_hint(hint);
    match reason {
        Some(reason) => refusal.with_reason(reason).into(),
        None => refusal.into(),
    }
}

/// Why a cooldown could not be served around.
#[derive(Debug, Clone, Copy)]
pub enum Blocked<'a> {
    /// `--refresh` was asked for, and walking is exactly what cannot happen.
    RefreshWanted,
    /// The named account has never been tracked, so there is nothing stored.
    AccountUnknown(&'a str),
    /// The account is known but this list has never been walked to the end.
    NothingStored(ListKind),
}

/// The account is in cooldown and storage cannot answer either.
pub fn refuse_in_cooldown(held: &Held, blocked: Blocked<'_>) -> anyhow::Error {
    let lifts = held.until_ms;
    let held = held_until(held);
    let detail = match blocked {
        Blocked::RefreshWanted => format!("{held}; --refresh cannot walk until it lifts"),
        // Filtered here rather than at the call site, so every caller of the
        // variant gets it.
        Blocked::AccountUnknown(name) => format!(
            "{held}, and no list of @{} is stored to serve in the meantime",
            printable(name)
        ),
        Blocked::NothingStored(kind) => format!(
            "{held}, and no complete snapshot of the {kind} list is stored, so there \
             is nothing to serve"
        ),
    };
    ExitError::new(ExitCode::RateLimited, detail)
        .with_cooldown_until(lifts)
        .into()
}

/// A cooldown, or the brake, that landed between the check and the walk.
pub fn refuse_cooldown_mid_walk(held: &Held) -> anyhow::Error {
    ExitError::new(
        ExitCode::RateLimited,
        format!("{}; nothing can be walked until it lifts", held_until(held)),
    )
    .with_cooldown_until(held.until_ms)
    .into()
}

/// A list another snob is walking right now, awake or asleep on the budget.
pub fn walked_elsewhere(target: Option<&str>, kind: ListKind) -> anyhow::Error {
    let who = target_label(target);
    let minutes = snob_store::store::snapshots::CLAIM_TTL_SECS / 60;
    ExitError::new(
        ExitCode::Error,
        format!(
            "another snob is walking {who}'s {kind} list right now; run this again \
             once it has finished, and it will use what that walk stored.\n\
             A walk that stopped without letting go is taken over after {minutes} minutes."
        ),
    )
    .with_reason("walked_elsewhere")
    .into()
}

/// What somebody is agreeing to when they let a run read a stranger's lists.
pub const READING_SOMEBODY_ELSES_LIST: &str = "this reads a list that belongs to somebody else, and lands their followers \
     in your local database. It is also a heavier request than reading your own, \
     and Instagram is readier to refuse it";

/// The rest of a list is more than the day's accounts can cover.
pub fn over_the_day(needed: u64, left: u64) -> String {
    format!(
        "the rest of this list is about {needed} accounts and today's budget has {left} left. \
         Reading more than that in a day is the pattern Instagram answers with an \
         automated-activity warning"
    )
}

/// What happens instead, and the flag that changes it.
pub const PAUSING_BY_DEFAULT: &str = "it will pause when the budget runs out and continue on \
     its own; --same-day finishes it today instead";

/// What the monitor says when it stops a walk for the battery.
pub const BATTERY_CRITICAL_STOP: &str = "the battery is critical; the walk stops here, \
     and the next run picks it up from this page";

/// What the monitor says when the battery turned critical before a list it
/// had not started.
pub const BATTERY_CRITICAL_SKIP: &str =
    "the battery is critical; this run reads no more lists, and the next run reads them";

/// What is left of a rest between sittings, as a person counts it: whole
/// minutes, rounded up, since the walk does not go before it ends.
pub fn rest_left(left: std::time::Duration) -> String {
    match left.as_secs().div_ceil(60) {
        1 => "1 minute".to_string(),
        minutes => format!("{minutes} minutes"),
    }
}

/// How long a walk has left, from the fewest and the most seconds it may take,
/// as a person says it: whole minutes rounded up, as [`rest_left`] counts them,
/// **one figure when both round to the same**, and "under a minute" when not
/// one is left.
pub fn time_left(least: u64, most: u64) -> String {
    let (least, most) = (least.min(most), least.max(most));
    let minutes = |n: u64| {
        if n == 1 {
            "1 minute".to_string()
        } else {
            format!("{n} minutes")
        }
    };
    match (least.div_ceil(60), most.div_ceil(60)) {
        (_, 0) => "under a minute".to_string(),
        (low, high) if low == high => format!("about {}", minutes(high)),
        (0, high) => format!("up to {}", minutes(high)),
        (low, high) => format!("about {low} to {high} minutes"),
    }
}

/// The consent question, with the account named the way the warning above
/// named it.
pub fn ask_to_continue(shown: &str) -> String {
    format!("Continue with {shown}?")
}

/// Being unable to ask and being told no are two different events, and this is
/// the first one, shaped the same way everywhere a question finds no terminal:
/// what was not done, why nothing could be asked, and how to answer in advance.
pub fn refuse_unattended(refused: String, in_advance: String) -> anyhow::Error {
    // A reason of its own, so a script can tell "nobody could be asked" from
    // a Ctrl+C, which shares the exit code.
    ExitError::new(
        ExitCode::Interrupted,
        format!("{refused}, and there is no terminal to ask at. {in_advance}"),
    )
    .with_reason("consent_required")
    .into()
}

/// Nobody is there to be asked, so nothing is enumerated.
///
/// Which way to answer in advance is the **caller's** fact rather than this
/// sentence's, and it arrives as [`ConsentInAdvance`]: the list commands take
/// `-y`, and `watch once` deliberately has no `-y`, so advice naming it there
/// would fail to parse.
pub fn refuse_unconsented(shown: &str, in_advance: ConsentInAdvance) -> anyhow::Error {
    let in_advance = match in_advance {
        ConsentInAdvance::Flag => "Pass -y to confirm in advance.".to_string(),
        ConsentInAdvance::WatchConfig => format!(
            "Run \"snob watch setup\" to answer it once, or ask about {shown} \
             while you are here."
        ),
    };
    refuse_unattended(
        format!("reading {shown}'s lists needs confirmation"),
        in_advance,
    )
}

/// They were asked, and they said no.
pub fn refuse_declined(shown: &str) -> anyhow::Error {
    ExitError::new(
        ExitCode::Interrupted,
        format!("nothing was done: {shown} was not confirmed"),
    )
    .with_reason("declined")
    .into()
}

/// A stored following list handed out with the account's own writes since
/// its walk applied (`engine::serve_stored`): what was applied, by name, and
/// that the list was not read again. Never "updated": nothing was walked.
pub fn own_writes_applied(
    taken_at: Epoch,
    applied: &[snob_store::store::own_writes::Applied],
) -> String {
    use snob_store::store::own_writes::Verb;
    let named = |verb: Verb| -> Vec<String> {
        applied
            .iter()
            .filter(|change| change.verb == verb)
            .map(|change| match &change.username {
                Some(name) => format!("@{}", printable(name)),
                None => format!("account {}", change.pk),
            })
            .collect()
    };
    let mut done = Vec::new();
    if let Some(names) = and_list(&named(Verb::Unfollow)) {
        done.push(format!("unfollowed {names}"));
    }
    if let Some(names) = and_list(&named(Verb::Follow)) {
        done.push(format!("followed {names}"));
    }
    let changes = if applied.len() == 1 {
        "1 change".to_string()
    } else {
        format!("{} changes", applied.len())
    };
    format!(
        "the following list stored on {} is shown with {changes} you made through snob \
         since ({}); it was not read again",
        stored_on(taken_at),
        done.join(" and ")
    )
}

/// Serving a stored list because nothing may be spent.
pub fn serving_stored_in_cooldown(held: &Held, kind: ListKind, taken_at: Epoch) -> String {
    format!(
        "{}; serving the {kind} list stored on {}",
        held_until(held),
        stored_on(taken_at)
    )
}

/// The counter poll failed and there is a stored list to fall back on.
pub fn poll_failed_serving_stored(error: &anyhow::Error) -> String {
    format!(
        "could not check for changes ({}); using the stored list",
        what_went_wrong(error)
    )
}

/// The counter poll failed and nothing is stored, so the walk goes ahead
/// without a number to check it against.
pub fn poll_failed(error: &anyhow::Error) -> String {
    format!("could not read the profile ({})", what_went_wrong(error))
}

/// A failure as one clause inside a sentence.
fn what_went_wrong(error: &anyhow::Error) -> String {
    match error
        .chain()
        .next()
        .and_then(|head| head.downcast_ref::<snob_ig::error::IgError>())
    {
        Some(instagram) => what_instagram_said(instagram),
        None => error.to_string(),
    }
}

/// A private account nobody here can read the lists of.
pub fn refuse_private(username: &str, requested: bool) -> anyhow::Error {
    if requested {
        return anyhow::anyhow!(
            "@{} is private and your follow request has not been accepted yet, \
             so its lists cannot be read",
            printable(username)
        );
    }
    anyhow::anyhow!(
        "@{} is a private account you do not follow, so its lists cannot be read",
        printable(username)
    )
}

/// The account whose profile Instagram will not serve, and what that costs the
/// run.
pub fn counters_unknowable(username: &str) -> String {
    format!(
        "Instagram would not serve the profile of @{}, so its id came from search \
         instead. That route carries no follower or following counts, so this run \
         cannot tell a truncated list from a complete one, and cannot judge whether \
         a cached list is still current.",
        printable(username)
    )
}

/// A run that concluded nothing about an account nothing is stored for.
pub fn refuse_nothing_looked_at(name: &str, code: ExitCode) -> anyhow::Error {
    ExitError::new(
        code,
        format!(
            "nothing could be looked at for @{} this time, and nothing is stored \
             about that account yet to report against",
            printable(name)
        ),
    )
    .into()
}

/// A name the monitor was pointed at and has never walked.
pub fn refuse_never_walked(name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "nothing is stored about @{}. Run \"snob followers {}\" once and the monitor \
         will have something to compare against from then on.",
        printable(name),
        printable(name),
    )
}

/// What to do about something Instagram's client reported, when there is
/// anything to do about it.
pub fn advice_for(error: &snob_ig::error::IgError) -> Option<&'static str> {
    use snob_ig::error::IgError;
    match error {
        IgError::SessionExpired => Some("run \"snob login\" again"),
        IgError::SignedOutHere => Some(SIGN_IN_AGAIN),
        IgError::WaitTooLong { .. } => Some(
            "run it again later, or with a longer --max-wait; \"snob status --budget\" says \
             when the budget has room",
        ),
        IgError::NoCsrfToken => Some(
            "run \"snob login --browser\", or, with SNOB_NO_BROWSER, pass the token with \
             \"snob login --paste --csrftoken\"",
        ),
        _ => None,
    }
}

/// One line: what Instagram's client said, and what to do about it.
///
/// The `hint:` line is what a **reported failure** gets, and the two places
/// that print an `IgError` are not that: `engine::walk` warns about what
/// stopped a walk while the walk's own refusal is still to come, and
/// `watch::check` puts the answer in a column of its own.
pub fn what_instagram_said(error: &snob_ig::error::IgError) -> String {
    match advice_for(error) {
        Some(advice) => format!("{error}; {advice}"),
        None => error.to_string(),
    }
}

/// Nothing stored to answer with, and `--offline` said not to look.
pub fn refuse_nothing_stored(kind: ListKind, target: Option<&str>) -> anyhow::Error {
    let target = target.map(|name| printable(name.strip_prefix('@').unwrap_or(name)));
    let (of, command) = match &target {
        Some(name) => (format!(" of @{name}"), format!("snob {kind} {name}")),
        None => (String::new(), format!("snob {kind}")),
    };
    ExitError::new(ExitCode::Error, format!("no {kind} list{of} is stored"))
        .with_reason("nothing_stored")
        .with_hint(format!(
            "run \"{command}\" once, or drop --offline, which says not to look for one"
        ))
        .into()
}

/// "a", "a and b", "a, b and c": names as a sentence says them, or `None` for
/// none, whose wording is the caller's.
pub fn and_list(names: &[String]) -> Option<String> {
    match names.split_last()? {
        (last, []) => Some(last.clone()),
        (last, rest) => Some(format!("{} and {last}", rest.join(", "))),
    }
}

/// The refusal every command gives when there is no session.
pub fn no_session() -> anyhow::Error {
    ExitError::new(ExitCode::NoSession, "no session is stored")
        .with_hint("run \"snob login\"")
        .into()
}

/// Refuses a session stored here that this computer no longer sends as
/// (`owner::signed_out`), with the code and hint of having none, and the
/// sentence that says why ([`SIGNED_OUT_HERE`]).
pub fn signed_out_here() -> anyhow::Error {
    ExitError::new(ExitCode::NoSession, SIGNED_OUT_HERE)
        .with_hint(SIGN_IN_AGAIN)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two of the targets this names come from `watch.toml`, which validates a
    /// username not at all, and one of those is the banner a scheduled service
    /// opens with — the first thing a monitor ever prints.
    #[test]
    fn naming_a_target_takes_out_what_a_terminal_would_obey() {
        let shown = target_label(Some("friend\u{1b}[2K"));
        assert!(!shown.contains('\x1b'), "{shown:?}");
        assert_eq!(shown, "@friend[2K");

        assert_eq!(target_label(None), "your account");
    }

    /// The time a walk has left says one figure when both ends round to the
    /// same minute, and never "0 minutes".
    #[test]
    fn the_time_left_is_one_figure_when_both_ends_are_the_same_minute() {
        assert_eq!(time_left(0, 0), "under a minute");
        assert_eq!(time_left(3, 10), "about 1 minute");
        assert_eq!(time_left(61, 90), "about 2 minutes");
        assert_eq!(time_left(30, 200), "about 1 to 4 minutes");
        assert_eq!(time_left(0, 200), "up to 4 minutes");
        assert_eq!(time_left(0, 45), "up to 1 minute");
        assert_eq!(
            time_left(400, 130),
            "about 3 to 7 minutes",
            "in either order"
        );
    }

    /// What is left of a rest is said in whole minutes, never fewer than it
    /// is: a second over a minute is two.
    #[test]
    fn the_rest_left_is_said_in_minutes_rounded_up() {
        let secs = std::time::Duration::from_secs;
        assert_eq!(rest_left(secs(1)), "1 minute");
        assert_eq!(rest_left(secs(60)), "1 minute");
        assert_eq!(rest_left(secs(61)), "2 minutes");
        assert_eq!(rest_left(secs(483)), "9 minutes");
    }

    /// A timestamp that makes no sense still has to read as something, because
    /// the alternative is a message with a hole in the middle of it.
    #[test]
    fn a_date_out_of_range_still_reads_as_something() {
        assert_eq!(stored_on(Epoch::new(i64::MAX)), "earlier");
        assert_eq!(cooldown_ends_at(EpochMs::new(i64::MAX)), "later");
        assert_eq!(
            format_epoch_in(Epoch::new(1_722_700_000), "earlier", &chrono::Utc),
            "Aug 3 at 15:46"
        );
        // The same moment in milliseconds reads as the same second, which is
        // what `EpochMs::to_epoch` is for.
        assert_eq!(
            format_epoch_in(
                EpochMs::new(1_722_700_000_000).to_epoch(),
                "later",
                &chrono::Utc
            ),
            "Aug 3 at 15:46"
        );
    }

    /// What is printed is the wall clock, not UTC with no label.
    #[test]
    fn a_moment_is_printed_in_the_zone_the_reader_is_in() {
        let madrid_in_august = chrono::FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(
            format_epoch_in(Epoch::new(1_722_700_000), "earlier", &madrid_in_august),
            "Aug 3 at 17:46"
        );
    }

    #[test]
    fn names_are_joined_as_a_sentence_says_them() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(and_list(&[]), None);
        assert_eq!(and_list(&names(&["@a"])).as_deref(), Some("@a"));
        assert_eq!(
            and_list(&names(&["@a", "@b"])).as_deref(),
            Some("@a and @b")
        );
        assert_eq!(
            and_list(&names(&["@a", "@b", "@c"])).as_deref(),
            Some("@a, @b and @c")
        );
    }

    /// The brake says it is every account, and names the ones pushed back on,
    /// with the throttling code.
    #[test]
    fn the_brake_names_the_accounts_behind_it() {
        let held = Held {
            until_ms: EpochMs::new(1_722_700_000_000),
            braked: vec!["@one".to_string(), "@two".to_string()],
        };
        let error = refuse_in_cooldown(&held, Blocked::NothingStored(ListKind::Followers));
        let text = error.to_string();
        assert!(text.starts_with("every account is paused until"), "{text}");
        assert!(
            text.contains("pushed back on @one and @two within an hour"),
            "{text}"
        );
        assert_eq!(crate::exit::from_chain(&error), Some(ExitCode::RateLimited));

        let three = ["@a", "@b", "@c"].map(String::from);
        assert!(
            brake_sentence(held.until_ms, &three).contains("@a, @b and @c"),
            "three accounts are listed as a sentence would"
        );
    }
}
