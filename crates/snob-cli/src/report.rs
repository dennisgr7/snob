//! The sentences the commands share, and how a failure is printed. The
//! sentences the engine returns are its own (`sentences`), named here too.

use snob_core::model::{ListKind, StopReason, User, printable};
use snob_engine::app::Viewer;
use snob_engine::exit::{ExitCode, ExitError};
use snob_engine::sentences::{acting_as, advice_for, and_list, cooldown_ends_at, stored_on};
use snob_ig::pager::Warning;

use crate::ui::theme::Role;

/// What a machine with no `watch.toml` is told, by both things that look: `snob
/// watch check`, through `commands::watch::say::problem_line`, and
/// `watch::status::health`.
pub const NOTHING_CONFIGURED: &str =
    "nothing is configured, so a bare \"snob watch\" has no schedule to run on";

/// What a missing consent means for a run with nobody at the keyboard.
pub const NO_RECORDED_CONSENT: &str =
    "no recorded consent, so an unattended run will refuse to read it";

/// Prints a failed run's error, as one message rather than as several.
pub fn print_error(error: &anyhow::Error, wording: Wording) {
    print_error_as(error, wording, acting_as().as_ref());
}

/// [`print_error`] for a failure met while acting as `viewer`, which a run
/// over several accounts knows better than the account acting last.
pub(crate) fn print_error_as(error: &anyhow::Error, wording: Wording, viewer: Option<&Viewer>) {
    match wording {
        Wording::Prose => eprint!("{}", rendered(error)),
        Wording::Json => eprintln!("{}", error_json_for(error, viewer)),
    }
}

/// How a failure is told: to a person, or to a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wording {
    Prose,
    Json,
}

/// The failure as one JSON object, for a caller that asked for JSON.
fn error_json_for(error: &anyhow::Error, viewer: Option<&Viewer>) -> serde_json::Value {
    let code = snob_engine::exit::exit_code_for(error);
    let mut chain = error
        .chain()
        .map(|cause| filtered(&cause.to_string(), "\n"));
    let message = chain.next().unwrap_or_default();
    let causes: Vec<String> = chain.collect();
    let instagram = instagram_in(error);
    let hint = advice_in(error).map(|hint| filtered(hint, "\n"));
    let url = instagram.and_then(|e| e.challenge_url());
    let ours = error.chain().find_map(|c| c.downcast_ref::<ExitError>());
    let lifts = ours
        .and_then(ExitError::cooldown_until)
        .or_else(|| {
            instagram.and_then(|e| match e {
                snob_ig::error::IgError::InCooldown { until_ms } => Some(*until_ms),
                _ => None,
            })
        })
        .map(|until_ms| until_ms.to_epoch());
    let retry_at = ours
        .and_then(ExitError::retry_at)
        .map(|at| at.to_epoch_not_before());
    // A wait past `--max-wait` exits as a stop does (130), and is told apart
    // from one by this: it is the one worth trying again later.
    let reason = ours.and_then(ExitError::reason).or(match instagram {
        Some(snob_ig::error::IgError::WaitTooLong { .. }) => Some("budget_wait"),
        Some(snob_ig::error::IgError::NotFound { what: Some(_) }) => Some("no_such_account"),
        _ => None,
    });

    serde_json::json!({
        "viewer": viewer.map(Viewer::json),
        "error": {
            "code": code.as_str(),
            "exit": snob_engine::exit::code(code),
            "message": message,
            "causes": causes,
            "hint": hint,
            "url": url,
            "cooldown_until": lifts,
            "retry_at": retry_at,
            "reason": reason,
        }
    })
}

/// `hint:` as every hint is labeled: bold, in the color of an aside.
fn hint_label() -> console::StyledObject<&'static str> {
    console::style("hint:")
        .fg(Role::Info.console())
        .bold()
        .for_stderr()
}

/// The block `print_error` writes, built rather than printed, so a test can
/// assert on every branch of it, the filter included, from inside the process.
fn rendered(error: &anyhow::Error) -> String {
    let code = snob_engine::exit::from_chain(error);
    if code == Some(ExitCode::Interrupted) {
        // No label, so nothing to indent under — but the filter is not the
        // label's business.
        return format!("{}\n", filtered(&error.to_string(), "\n"));
    }

    // The same roles the views draw in (`ui::theme`), so an error is one red
    // and a hint one cyan wherever this program says them.
    let label = console::style("error:")
        .fg(Role::Error.console())
        .bold()
        .for_stderr();
    let mut out = format!("{label} {}\n", indented(&error.to_string()));

    for cause in error.chain().skip(1) {
        let caused = console::style("caused by:").dim().for_stderr();
        out.push_str(&format!("  {caused} {}\n", indented(&cause.to_string())));
    }

    if let Some(hint) = advice_in(error) {
        let label = hint_label();
        out.push_str(&format!("{label}  {}\n", indented(hint)));
    }

    // The backstop in `Pacer::clear` answers with the epoch and no wording,
    // because when a cooldown lifts is a date and `snob-ig` has no business
    // formatting one.
    if let Some(snob_ig::error::IgError::InCooldown { until_ms }) = instagram_in(error) {
        let label = hint_label();
        out.push_str(&format!(
            "{label}  {}\n",
            indented(&format!("it lifts {}", cooldown_ends_at(*until_ms)))
        ));
    }

    out
}

/// The first thing Instagram's client said, anywhere in the chain.
fn instagram_in(error: &anyhow::Error) -> Option<&snob_ig::error::IgError> {
    error.chain().find_map(|c| c.downcast_ref())
}

/// The one piece of advice a failure carries, read the same way by the prose
/// and by the JSON so the two shapes cannot disagree.
fn advice_in(error: &anyhow::Error) -> Option<&str> {
    error
        .chain()
        .find_map(|c| c.downcast_ref::<ExitError>())
        .and_then(ExitError::hint)
        .or_else(|| instagram_in(error).and_then(advice_for))
}

/// Lines after the first start under the label rather than at column zero.
fn indented(text: &str) -> String {
    filtered(text, "\n       ")
}

/// The filter every string this module prints goes through, which makes it the
/// boundary the rule asks for: a name is filtered before anything draws it,
/// whoever it came from.
///
/// **Line by line, not over the whole string.**
fn filtered(text: &str, join: &str) -> String {
    text.split('\n')
        .map(printable)
        .collect::<Vec<_>>()
        .join(join)
}

/// `sentence` with its first letter raised, to open a line with. By
/// character: cut by bytes, a sentence opening on anything past ASCII is a
/// panic.
pub(crate) fn opening(sentence: &str) -> String {
    let mut rest = sentence.chars();
    match rest.next() {
        Some(first) => std::iter::once(first.to_ascii_uppercase())
            .chain(rest)
            .collect(),
        None => String::new(),
    }
}

/// The requests half of a summary line: what was spent, or the promise that
/// nothing was.
pub fn spent(n: u32) -> String {
    if n > 0 {
        format!(" - {}", requests(n))
    } else {
        " - without touching the network".to_string()
    }
}

/// The date a crossing's summary names: the older of the two captures, because
/// a crossing is only as recent as its staler half.
pub fn stored_on_the_older_of(a: snob_core::Epoch, b: snob_core::Epoch) -> String {
    stored_on(a.min(b))
}

/// The refusal shared by the crossings and the summary.
pub fn refuse_incomplete(
    list: ListKind,
    outcome: &snob_engine::engine::ListOutcome,
    misreading: &str,
) -> anyhow::Error {
    ExitError::new(
        outcome.exit_code(),
        format!(
            "the {list} list could not be read in full, so the answer would be wrong: \
             the accounts missing from it would appear as if {misreading}."
        ),
    )
    .with_hint(try_again_advice(outcome.reason, outcome.resumable))
    .into()
}

/// Said before `snob likers` asks: what it reads, and that it is a list of
/// other people's accounts as a followers list is.
pub const READING_A_POSTS_LIKERS: &str = "this reads the accounts that liked a post, which belong \
     to other people, and Instagram shows about a hundred of them, the ones you know first";

/// The walk is asleep on the day's accounts and will go on by itself.
pub fn paused_for_the_day(wait: std::time::Duration) -> String {
    let minutes = wait.as_secs().div_ceil(60);
    let when = if minutes < 120 {
        format!("{minutes} minutes")
    } else {
        format!("about {} hours", minutes.div_ceil(60))
    };
    format!(
        "today's account budget is spent; waiting {when} to continue from where it stopped. \
         Ctrl+C stops here, and the next run picks up from the same page"
    )
}

/// What `snob watch once <name> --webhook ...` says before it asks about
/// `name`'s lists: that what changes in them goes to `destination`, shown as
/// every address is (`webhook::shown_str`).
pub fn sent_to(name: &str, destination: &str) -> String {
    format!(
        "What changes in @{}'s lists is sent to {}.",
        printable(snob_engine::engine::target::clean(name)),
        snob_engine::watch::webhook::shown_str(destination)
    )
}

/// What the scheduled monitor says when the run that is due waits for power.
pub const BATTERY_CRITICAL_HOLD: &str = "the battery is critical; the run that is due waits \
     until the machine is on power or charged again";

/// What a walk says when it finds the machine slept in the middle of it.
pub fn slept_during_the_walk(slept: std::time::Duration) -> String {
    let minutes = slept.as_secs().div_ceil(60);
    let how_long = if minutes < 120 {
        format!("{minutes} minutes")
    } else {
        format!("about {} hours", minutes.div_ceil(60))
    };
    format!(
        "the computer slept for {how_long}; picking the walk up again where it stopped \
         once the network is back"
    )
}

/// "@someone followers" — what a run is walking, said the same way by every
/// command that walks something.
pub fn walking(kind: ListKind, subject: &str) -> String {
    format!("{subject} {kind}")
}

/// "3 unfollowers", plus what took the others away when anything did.
pub fn counted(shown: usize, kept: usize, total: usize, one: &str, many: &str) -> String {
    let what = if shown == 1 { one } else { many };
    let mut line = format!("{shown} {what}");
    match (kept < total, shown < kept) {
        (false, false) => {}
        (true, false) => line.push_str(&format!(" (of {total}, the rest filtered out)")),
        (false, true) => line.push_str(&format!(" (of {kept}, trimmed by --limit)")),
        (true, true) => line.push_str(&format!(
            " (of {total}: {} filtered out, the rest trimmed by --limit)",
            total - kept
        )),
    }
    line
}

/// "1 answer", "12 answers": how many answers of the API `snob status`
/// counted.
pub fn answers(n: u32) -> String {
    match n {
        1 => "1 answer".to_string(),
        n => format!("{n} answers"),
    }
}

/// "1 account" / "12 accounts", for the headings and notes of the views.
pub fn accounts(n: usize) -> String {
    match n {
        1 => "1 account".to_string(),
        n => format!("{n} accounts"),
    }
}

/// "1 request" / "7 requests".
pub fn requests(n: u32) -> String {
    if n == 1 {
        "1 request".to_string()
    } else {
        format!("{n} requests")
    }
}

/// The closing advice for a message about an incomplete walk.
///
/// The other four — the hard page cap, a cursor that came back unchanged, two
/// empty pages, and several pages with nothing new — stop in the **middle** of
/// the pagination with a cursor stored.
pub fn try_again_advice(reason: StopReason, resumable: bool) -> &'static str {
    match reason {
        StopReason::RateLimit if !resumable => {
            "Run it again once the cooldown lifts; there is nothing stored to continue from, so \
             it starts over."
        }
        StopReason::RateLimit => {
            "Run it again once the cooldown lifts to continue where it left off, within a day \
             and a half of when this walk began; after that it starts over."
        }
        StopReason::SessionInvalid => {
            "Deal with what Instagram asked for first. Running it again before that cannot get \
             any further."
        }
        StopReason::Truncated if !resumable => {
            "Instagram stopped serving this account's list; there is nothing to continue from, \
             so running it again starts over. Try later."
        }
        StopReason::Truncated => {
            "Instagram stopped serving pages. Run it again within a day and a half of when this \
             walk began and it continues from where it stopped; after that it starts over."
        }
        _ if !resumable => {
            "Run it again; there is nothing stored to continue from, so it starts over."
        }
        _ => "Run it again to continue where it left off.",
    }
}

/// Why a walk did not finish, in the words the summary uses.
pub fn why_incomplete(reason: StopReason) -> Option<&'static str> {
    match reason {
        StopReason::Completed => None,
        StopReason::PageLimit => Some("cut short by the cap you asked for"),
        StopReason::Canceled => Some("interrupted"),
        StopReason::Truncated => Some("Instagram stopped serving pages"),
        StopReason::RateLimit => Some("Instagram is throttling requests"),
        StopReason::Network => Some("network failure"),
        StopReason::SessionInvalid => Some("the session stopped working"),
    }
}

/// What the walker noticed, in the words the person watching it reads.
pub fn pager_warning(warning: Warning) -> String {
    match warning {
        Warning::SameCursorTwice => {
            "Instagram returned the same cursor twice; stopping so the request is not repeated"
                .to_string()
        }
        Warning::TwoEmptyPages => "Instagram returned two empty pages in a row".to_string(),
        Warning::GoingInCircles => {
            "several pages in a row with no new accounts; the list is going in circles".to_string()
        }
        Warning::EmptyAndNoCounter => {
            "the list came back empty and the profile counter could not be read, so there is \
             no way to tell an empty list from one Instagram did not serve; treating it as \
             incomplete rather than risking the comparison"
                .to_string()
        }
        Warning::StoppedShort { walked, declared } => format!(
            "Instagram stopped serving pages at {walked} of the {declared} accounts it declared; \
             the list is incomplete and cannot be compared against"
        ),
        Warning::ShortOfDeclared { walked, declared } => format!(
            "walked {walked} accounts while Instagram declared {declared}; \
             the difference is usually deleted accounts"
        ),
        Warning::Repeated { repeated, received } => format!(
            "{repeated} of the {received} accounts Instagram served were ones it had already \
             served, more than one in twenty; the list may be missing accounts and cannot be \
             compared against"
        ),
    }
}

/// "pepito, carlos and 4 others", or `None` when there is nobody to name.
pub fn name_a_few(people: &[User], cap: usize) -> Option<String> {
    // A cap of zero would name nobody and count everybody, which is not a
    // sentence anyone wants to read. At least one name, always.
    let shown = cap.max(1).min(people.len());
    if shown == 0 {
        return None;
    }

    // Filtered here rather than at each consumer: this line is the first thing
    // `snob scan` prints, with no flag needed, and it also goes into the
    // markdown summary, whose escaping is about table cells rather than about
    // what a terminal obeys.
    let mut parts: Vec<String> = people[..shown]
        .iter()
        .map(|u| format!("@{}", u.safe_username()))
        .collect();
    parts.extend(match people.len() - shown {
        0 => None,
        1 => Some("1 other".to_string()),
        n => Some(format!("{n} others")),
    });

    and_list(&parts)
}

/// A schedule as clauses, ready to be joined into a sentence.
pub fn schedule_clauses(
    every: Option<std::time::Duration>,
    on: &[String],
    at: &[String],
    cron: Option<&str>,
) -> Vec<String> {
    let mut clauses = Vec::new();
    if let Some(every) = every {
        clauses.push(format!("every {}", snob_core::duration::format(every)));
    }
    if !on.is_empty() {
        clauses.push(format!("on {}", printable(&on.join(", "))));
    }
    if !at.is_empty() {
        clauses.push(format!("at {}", printable(&at.join(", "))));
    }
    if let Some(cron) = cron {
        clauses.push(format!("on the schedule \"{}\"", printable(cron)));
    }
    clauses
}

/// What the jitter does, or nothing at all when there is none.
///
/// **The wording is shared and the number is not.**
pub fn jitter_sentence(jitter: std::time::Duration) -> Option<String> {
    (!jitter.is_zero()).then(|| {
        format!(
            "Each run is pushed up to {} later, so it does not land on the same second every \
             time.",
            snob_core::duration::format(jitter)
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::Pk;
    use snob_core::{Epoch, EpochMs};
    use snob_engine::app::Held;
    use snob_engine::engine::{ListOutcome, Provenance};
    use snob_engine::sentences::{
        Blocked, refuse_cooldown_mid_walk, refuse_different_moments, refuse_in_cooldown,
        refuse_nothing_stored, what_instagram_said,
    };

    /// What `watch once <name> --webhook` says before its question names the
    /// account and where its changes go, the secret path of a Slack webhook
    /// left out.
    #[test]
    fn the_destination_is_said_without_its_secret() {
        let said = sent_to("@friend", "https://hooks.slack.com/services/T0/B0/secret");
        assert!(said.contains("@friend's lists"), "{said}");
        assert!(said.contains("hooks.slack.com"), "{said}");
        assert!(!said.contains("secret"), "{said}");
    }

    /// Nothing stored names the account the list is of, in the command to
    /// type back, without an at sign, and carries its reason.
    #[test]
    fn nothing_stored_names_whose_list_it_is() {
        let theirs = refuse_nothing_stored(ListKind::Followers, Some("@user3"));
        let json = error_json_for(&theirs, None);
        assert_eq!(json["error"]["reason"], "nothing_stored");
        assert_eq!(
            json["error"]["message"],
            "no followers list of @user3 is stored"
        );
        let hint = json["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("run \"snob followers user3\" once"), "{hint}");
        let own = refuse_nothing_stored(ListKind::Following, None);
        let json = error_json_for(&own, None);
        assert!(
            json["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("run \"snob following\" once"),
            "{json}"
        );
    }

    /// A refusal by a cooldown says when it lifts as a moment, not only in
    /// the sentence.
    #[test]
    fn a_cooldown_refusal_says_when_it_lifts_in_its_json() {
        let held = Held {
            until_ms: EpochMs::new(1_700_000_000_000),
            braked: Vec::new(),
        };
        for refused in [
            refuse_in_cooldown(&held, Blocked::RefreshWanted),
            refuse_in_cooldown(&held, Blocked::NothingStored(ListKind::Followers)),
            refuse_cooldown_mid_walk(&held),
        ] {
            let json = error_json_for(&refused, None);
            assert_eq!(
                json["error"]["cooldown_until"],
                serde_json::json!(held.until_ms.to_epoch()),
                "{json}"
            );
            assert_eq!(json["error"]["code"], "rate_limited");
        }
    }

    /// The cap on an agent's writes says when the next is free as a moment,
    /// which is no cooldown's end.
    #[test]
    fn a_refusal_by_the_write_cap_says_when_it_frees_in_its_json() {
        let free_at = EpochMs::new(1_700_000_000_500);
        let refused = crate::commands::common::write_cap_reached(10, free_at);
        let json = error_json_for(&refused, None);
        assert_eq!(
            json["error"]["retry_at"],
            serde_json::json!(free_at.to_epoch_not_before()),
            "{json}"
        );
        assert_eq!(json["error"]["reason"], "mcp_write_cap");
        assert!(json["error"]["cooldown_until"].is_null(), "{json}");
    }

    /// A wait for the budget past `--max-wait` exits as a stop does, and is
    /// told apart by its reason.
    #[test]
    fn a_wait_past_the_limit_has_a_reason_of_its_own() {
        let error = anyhow::Error::new(snob_ig::error::IgError::WaitTooLong {
            wait: std::time::Duration::from_secs(900),
            limit: std::time::Duration::from_secs(300),
        });
        let json = error_json_for(&error, None);
        assert_eq!(json["error"]["code"], "interrupted");
        assert_eq!(json["error"]["reason"], "budget_wait");
    }

    /// Every string this module draws goes through the name filter, and the
    /// paragraph breaks it lays out survive it.
    ///
    /// `printable` turns any whitespace into a space, newlines included, so
    /// filtering a refusal whole would collapse the very structure `indented`
    /// exists to produce. Line by line, both hold.
    #[test]
    fn the_layout_survives_the_filter_and_an_escape_sequence_does_not() {
        let hostile = "first line\u{1b}[2K\u{1b}[A\nsecond line";
        let out = indented(hostile);

        assert!(
            !out.chars().any(|c| c.is_control() && c != '\n'),
            "{out:?} reaches a terminal"
        );
        assert_eq!(
            out.lines().count(),
            2,
            "the deliberate break between sentences is not the filter's business: {out:?}"
        );
        assert!(out.starts_with("first line"));
        assert!(out.trim_end().ends_with("second line"));
    }

    fn people(names: &[&str]) -> Vec<User> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| User {
                pk: Pk::new(i as u64 + 1),
                username: (*name).into(),
                full_name: None,
                is_private: None,
                is_verified: None,
                pfp_url: None,
            })
            .collect()
    }

    #[test]
    fn nobody_is_not_a_sentence() {
        assert_eq!(name_a_few(&[], 3), None);
    }

    /// This line opens `snob scan` with no flag asked for, so a username is
    /// the shortest route from somebody else's profile to the terminal.
    #[test]
    fn a_hostile_name_cannot_drive_the_terminal() {
        let hostile = people(&["ana\u{1b}[2K", "lu\u{202e}is"]);
        let line = name_a_few(&hostile, 3).unwrap();
        assert!(!line.contains('\u{1b}'), "{line:?}");
        assert!(!line.contains('\u{202e}'), "{line:?}");
        assert_eq!(line, "@ana[2K and @luis");
    }

    #[test]
    fn one_name_stands_alone() {
        assert_eq!(name_a_few(&people(&["ana"]), 3).unwrap(), "@ana");
    }

    #[test]
    fn the_last_one_joins_with_and() {
        assert_eq!(
            name_a_few(&people(&["ana", "luis"]), 3).unwrap(),
            "@ana and @luis"
        );
        assert_eq!(
            name_a_few(&people(&["ana", "luis", "eva"]), 3).unwrap(),
            "@ana, @luis and @eva"
        );
    }

    /// The count is the last item of the one list, not a second list after it.
    #[test]
    fn past_the_cap_the_rest_are_counted() {
        let five = people(&["ana", "luis", "eva", "juan", "sara"]);
        assert_eq!(
            name_a_few(&five, 3).unwrap(),
            "@ana, @luis, @eva and 2 others"
        );
        assert_eq!(
            name_a_few(&five, 4).unwrap(),
            "@ana, @luis, @eva, @juan and 1 other"
        );
        // Exactly at the cap nothing is left over to count.
        assert_eq!(
            name_a_few(&five, 5).unwrap(),
            "@ana, @luis, @eva, @juan and @sara"
        );
    }

    /// A run somebody stopped gets no label, and is filtered all the same. The
    /// refusal below is the one a scheduled run raises, and the name in it
    /// comes from `watch.toml`, where nothing validates a username.
    #[test]
    fn a_stopped_run_is_filtered_even_though_it_carries_no_label() {
        let hostile = "gh\u{1b}[2K\u{1b}[A";
        let error: anyhow::Error = ExitError::new(
            ExitCode::Interrupted,
            format!(
                "reading @{hostile}'s lists needs confirmation, and a scheduled run has \
                 nobody to ask.\nRun \"snob watch setup\" to answer it once."
            ),
        )
        .into();

        let out = rendered(&error);

        assert!(
            !out.chars().any(|c| c.is_control() && c != '\n'),
            "{out:?} reaches a terminal"
        );
        assert!(out.contains("@gh"), "the name is still shown: {out}");
        assert!(
            !out.contains("error:"),
            "stopping a run is not a failure to report: {out}"
        );
        // No label above it, so the second sentence stays at column zero.
        let mut lines = out.lines();
        assert!(lines.next().is_some_and(|l| l.starts_with("reading @gh")));
        assert_eq!(
            lines.next(),
            Some("Run \"snob watch setup\" to answer it once.")
        );
        assert_eq!(lines.next(), None);
    }

    /// An account name reaches the cooldown refusal without anybody typing it:
    /// `Watched::list_args` puts the one from `watch.toml` straight into
    /// `ListArgs.target`, and nothing validates a username there.
    #[test]
    fn a_cooldown_refusal_cannot_be_made_to_erase_the_line_above_it() {
        let name = "gh\u{1b}[2K\u{1b}[A";
        let error = refuse_in_cooldown(&own_cooldown(1_000), Blocked::AccountUnknown(name));
        let message = error.to_string();

        assert!(
            !message.chars().any(|c| c.is_control()),
            "{message:?} is printed to a terminal"
        );
        assert!(
            message.contains("@gh"),
            "the name is still shown: {message}"
        );
    }

    /// A walk that stopped for `reason`, and what Instagram said about it when
    /// that is more specific than the reason.
    fn stopped(reason: StopReason, stopped_by: Option<ExitCode>) -> ListOutcome {
        ListOutcome {
            stopped_by,
            ..ListOutcome::for_test(Provenance::Walked, reason)
        }
    }

    /// A failure told to a program carries what the prose carries: the code
    /// the exit status names, the message, the advice set apart, and the
    /// address a challenge is cleared at -- filtered, since the consumer may
    /// print it.
    #[test]
    fn a_failure_in_json_carries_the_code_the_hint_and_the_address() {
        let hostile = "gh\u{1b}[2K";
        let error: anyhow::Error = anyhow::Error::new(snob_ig::error::IgError::Challenge {
            url: Some("https://www.instagram.com/challenge/".into()),
        })
        .context(format!("could not read @{hostile}'s followers list"));
        let json = error_json_for(&error, None);
        let error = &json["error"];

        assert_eq!(error["code"], "challenge");
        assert_eq!(error["exit"], 4);
        assert_eq!(error["message"], "could not read @gh[2K's followers list");
        assert_eq!(error["url"], "https://www.instagram.com/challenge/");
        assert_eq!(error["causes"].as_array().map(Vec::len), Some(1));

        let advised: anyhow::Error = ExitError::new(ExitCode::NoSession, "no session")
            .with_hint("run \"snob login\"")
            .into();
        let json = error_json_for(&advised, None);
        assert_eq!(json["error"]["code"], "no_session");
        assert_eq!(json["error"]["hint"], "run \"snob login\"");
        assert!(json["error"]["url"].is_null());
        assert!(json["error"]["reason"].is_null());

        // A finer token than the exit's, beside it, never in its place.
        let busy: anyhow::Error = ExitError::new(ExitCode::Error, "held elsewhere")
            .with_reason("busy")
            .into();
        let json = error_json_for(&busy, None);
        assert_eq!(json["error"]["code"], "error");
        assert_eq!(json["error"]["reason"], "busy");
    }

    /// A failure names the account the run acted as, beside the error, so a
    /// script running several accounts can tell whose it was.
    #[test]
    fn a_failure_in_json_names_the_account_it_acted_as() {
        let error = anyhow::anyhow!("it failed");
        assert!(error_json_for(&error, None)["viewer"].is_null());

        let viewer = Viewer {
            pk: snob_core::Pk::new(42),
            username: Some("me\u{1b}[2K".into()),
        };
        let json = error_json_for(&error, Some(&viewer));
        assert_eq!(json["viewer"]["pk"], 42);
        assert_eq!(json["viewer"]["username"], "me[2K");
        assert_eq!(json["error"]["message"], "it failed");
    }

    /// The refusal has to name the mistake the caller would otherwise have
    /// made, and carry the code that says what to do about it.
    #[test]
    fn refusing_names_the_list_the_misreading_and_the_code() {
        let error = refuse_incomplete(
            ListKind::Followers,
            &stopped(StopReason::RateLimit, None),
            "they did not follow you",
        );
        let text = error.to_string();
        assert!(text.contains("followers list"), "{text}");
        assert!(text.contains("they did not follow you"), "{text}");
        // The advice lives on the hint, not in the sentence: see
        // `a_refusal_keeps_its_advice_apart_from_what_happened`.
        assert!(hint_of(&error).unwrap().contains("starts over"));

        assert_eq!(
            snob_engine::exit::from_chain(&error),
            Some(ExitCode::RateLimited)
        );
    }

    #[test]
    fn a_refusal_carries_the_code_of_whatever_stopped_the_walk() {
        for (reason, expected) in [
            (StopReason::Canceled, ExitCode::Interrupted),
            (StopReason::RateLimit, ExitCode::RateLimited),
            (StopReason::SessionInvalid, ExitCode::NoSession),
            (StopReason::Truncated, ExitCode::Error),
        ] {
            // Nothing more specific than the stop reason, so the code comes off
            // the reason — which is `ListOutcome::exit_code`'s fallback.
            let error = refuse_incomplete(ListKind::Following, &stopped(reason, None), "whatever");
            assert_eq!(
                snob_engine::exit::from_chain(&error),
                Some(expected),
                "{reason:?}"
            );
        }
    }

    /// `SessionInvalid` is what the store records for both "log in again" and
    /// "Instagram wants the account verified", and those are different codes.
    /// The caller is allowed to know better than the stop reason does.
    #[test]
    fn a_challenge_keeps_its_own_code_under_a_session_invalid_stop() {
        let error = refuse_incomplete(
            ListKind::Followers,
            &stopped(StopReason::SessionInvalid, Some(ExitCode::Challenge)),
            "whatever",
        );
        assert_eq!(
            snob_engine::exit::from_chain(&error),
            Some(ExitCode::Challenge)
        );
    }

    /// Against an account Instagram has just flagged, "run it again" is the
    /// one piece of advice that cannot help and can make it worse.
    #[test]
    fn a_dead_session_is_not_told_to_try_again() {
        // Both ways: a checkpoint can land mid-pagination with a cursor stored,
        // and continuing is still the wrong thing to offer.
        for resumable in [false, true] {
            let advice = try_again_advice(StopReason::SessionInvalid, resumable);
            assert!(!advice.contains("continue where it left off"), "{advice}");
            assert!(advice.contains("Instagram asked for"), "{advice}");
        }
    }

    fn hint_of(error: &anyhow::Error) -> Option<String> {
        error
            .chain()
            .find_map(|c| c.downcast_ref::<ExitError>())
            .and_then(ExitError::hint)
            .map(str::to_string)
    }

    /// The advice is separate from the failure, so the printer can label them
    /// differently and the advice does not read as more of the complaint.
    #[test]
    fn a_refusal_keeps_its_advice_apart_from_what_happened() {
        let error = refuse_incomplete(
            ListKind::Followers,
            &stopped(StopReason::RateLimit, None),
            "they did not follow you",
        );
        assert!(error.to_string().contains("would be wrong"), "{error}");
        assert!(
            !error.to_string().contains("Run it again"),
            "the advice is not part of what happened: {error}"
        );
        assert!(hint_of(&error).unwrap().contains("starts over"));
    }

    /// A cooldown is waited out and the other two causes are not, so the same
    /// refusal owes them different advice. Telling somebody to sit out a
    /// cooldown they are not in is worse than saying nothing.
    #[test]
    fn different_moments_are_explained_by_why_nobody_checked() {
        let throttled = refuse_different_moments(
            Provenance::Cooldown,
            Provenance::Cooldown,
            Epoch::new(0),
            Epoch::new(1),
        );
        assert!(hint_of(&throttled).unwrap().contains("cooldown lifts"));
        assert_eq!(
            snob_engine::exit::from_chain(&throttled),
            Some(ExitCode::RateLimited)
        );

        let asked_for = refuse_different_moments(
            Provenance::CacheFlag,
            Provenance::CacheFlag,
            Epoch::new(0),
            Epoch::new(1),
        );
        let hint = hint_of(&asked_for).unwrap();
        assert!(hint.contains("--offline"), "{hint}");
        assert!(!hint.contains("cooldown"), "{hint}");
        assert_eq!(
            snob_engine::exit::from_chain(&asked_for),
            Some(ExitCode::Error)
        );
        // A reason of its own, so an agent, whose server passed the flag, is
        // told something it can do instead.
        let reason = |error: &anyhow::Error| {
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<ExitError>())
                .and_then(ExitError::reason)
        };
        assert_eq!(reason(&asked_for), Some("stored_moments"));
        assert_eq!(reason(&throttled), None);

        // A failed poll is neither of the two. Nobody asked for storage — the
        // request to check went out and did not come back — so it is not told
        // to drop a flag it never passed.
        let nobody_could_check = refuse_different_moments(
            Provenance::PollFailed,
            Provenance::PollFailed,
            Epoch::new(0),
            Epoch::new(1),
        );
        let hint = hint_of(&nobody_could_check).unwrap();
        assert!(!hint.contains("--offline"), "{hint}");
        assert!(!hint.contains("cooldown"), "{hint}");
        assert_eq!(
            snob_engine::exit::from_chain(&nobody_could_check),
            Some(ExitCode::Error)
        );

        // All three say the same thing about what happened; only the advice
        // differs.
        for error in [&throttled, &asked_for, &nobody_could_check] {
            assert!(error.to_string().contains("different moments"), "{error}");
        }
    }

    /// Every way a cooldown can leave the user with nothing says which one it
    /// was, and all of them carry the throttling code.
    #[test]
    fn a_cooldown_refusal_names_what_is_missing() {
        let cases = [
            (Blocked::RefreshWanted, "--refresh"),
            (Blocked::AccountUnknown("someone"), "@someone"),
            (Blocked::NothingStored(ListKind::Followers), "followers"),
        ];
        for (blocked, expected) in cases {
            let error = refuse_in_cooldown(&own_cooldown(1_722_700_000_000), blocked);
            let text = error.to_string();
            assert!(text.contains(expected), "{text}");
            assert!(text.contains("in cooldown until"), "{text}");
            assert_eq!(
                snob_engine::exit::from_chain(&error),
                Some(ExitCode::RateLimited)
            );
        }
    }

    fn own_cooldown(until_ms: i64) -> Held {
        Held {
            until_ms: EpochMs::new(until_ms),
            braked: Vec::new(),
        }
    }

    /// A refusal is several sentences, and the ones after the first sit under
    /// the label rather than at column zero, where they would read as separate
    /// unattributed text.
    #[test]
    fn continuation_lines_sit_under_the_label() {
        let indented = indented(
            "first
second
third",
        );
        assert_eq!(
            indented,
            "first
       second
       third"
        );
        // A single line is left exactly as it was.
        assert_eq!(super::indented("only one"), "only one");
    }

    /// Filters that took nothing out must not leave a clause saying they did.
    #[test]
    fn the_count_only_mentions_filtering_when_something_was_filtered() {
        assert_eq!(
            counted(3, 3, 3, "unfollower", "unfollowers"),
            "3 unfollowers"
        );
        assert_eq!(
            counted(3, 3, 10, "unfollower", "unfollowers"),
            "3 unfollowers (of 10, the rest filtered out)"
        );
    }

    /// A cap is not a filter: `--limit 3` on ten unfollowers with no filters
    /// at all does not report the other seven as filtered out.
    #[test]
    fn a_cap_is_reported_as_a_cap() {
        assert_eq!(
            counted(3, 10, 10, "unfollower", "unfollowers"),
            "3 unfollowers (of 10, trimmed by --limit)"
        );
        // And when both happened, both are named.
        assert_eq!(
            counted(2, 4, 10, "unfollower", "unfollowers"),
            "2 unfollowers (of 10: 6 filtered out, the rest trimmed by --limit)"
        );
    }

    /// A filtered list lands on one result often enough that the singular is
    /// the summary users see most.
    #[test]
    fn a_count_of_one_reads_as_one() {
        assert_eq!(
            counted(1, 1, 1, "unfollower", "unfollowers"),
            "1 unfollower"
        );
        assert_eq!(
            counted(1, 1, 9, "unfollower", "unfollowers"),
            "1 unfollower (of 9, the rest filtered out)"
        );
        assert_eq!(
            counted(0, 0, 0, "unfollower", "unfollowers"),
            "0 unfollowers"
        );
    }

    #[test]
    fn the_request_count_is_pluralized() {
        assert_eq!(requests(0), "0 requests");
        assert_eq!(requests(1), "1 request");
        assert_eq!(requests(2), "2 requests");
    }

    #[test]
    fn the_advice_depends_on_the_stop_reason() {
        assert!(try_again_advice(StopReason::RateLimit, false).contains("starts over"));
        // These stop mid-pagination, so the store has a cursor.
        for reason in [
            StopReason::Canceled,
            StopReason::PageLimit,
            StopReason::Network,
            StopReason::RateLimit,
        ] {
            assert!(try_again_advice(reason, true).contains("continue where it left off"));
        }
    }

    /// The two ways this module offers a continuation. Both are matched,
    /// because saying "nothing to continue from" also contains the word.
    fn offers_a_continuation(advice: &str) -> bool {
        advice.contains("continue where it left off") || advice.contains("continues from")
    }

    /// The offer to continue follows what the store kept, not what the reason
    /// suggests.
    ///
    /// Every reason here can arrive either way. A `Canceled` walk normally has
    /// a cursor, but one interrupted more than a day and a half after it began
    /// has already aged out of the resume window — and the advice must not
    /// offer a continuation the next run cannot make.
    #[test]
    fn nothing_is_offered_to_continue_when_there_is_nothing_stored() {
        for reason in [
            StopReason::Canceled,
            StopReason::PageLimit,
            StopReason::Network,
            StopReason::Truncated,
            StopReason::RateLimit,
        ] {
            let advice = try_again_advice(reason, false);
            assert!(
                !offers_a_continuation(advice),
                "{reason:?} offered a continuation: {advice}"
            );
            assert!(advice.contains("starts over"), "{reason:?}: {advice}");
        }
    }

    /// The reclassified truncation is the one that really has nothing left.
    ///
    /// `verify_completion` reaches it **after** the pagination has ended, so the
    /// snapshot closes with no cursor and `snapshots::resumable` will not
    /// return a row without one. The four guards that stop in the middle of the
    /// pagination do leave one, which is why the reason alone cannot answer
    /// this and `resumable` is asked separately.
    #[test]
    fn a_truncated_walk_says_which_of_the_two_it_was() {
        let ended = try_again_advice(StopReason::Truncated, false);
        assert!(!offers_a_continuation(ended), "{ended}");
        assert!(ended.contains("starts over"), "{ended}");

        let stopped_short = try_again_advice(StopReason::Truncated, true);
        assert!(stopped_short.contains("continues from where it stopped"));
    }

    /// Advice that names a `snob` subcommand belongs to the binary, not to its
    /// HTTP client — and it still has to reach the person reading.
    ///
    /// Every path an `IgError` takes carries its advice: the labeled failure,
    /// the JSON shape, and the two places that print an `IgError` as one line.
    #[test]
    fn the_advice_an_ig_error_carried_still_reaches_the_reader() {
        use snob_ig::error::IgError;

        for (error, advice) in [
            (IgError::SessionExpired, "run \"snob login\" again"),
            (IgError::SignedOutHere, "run \"snob login\""),
            (
                IgError::NoCsrfToken,
                "run \"snob login --browser\", or, with SNOB_NO_BROWSER, pass the token with \
                 \"snob login --paste --csrftoken\"",
            ),
        ] {
            // One line: the diagnosis, then the advice.
            assert_eq!(
                what_instagram_said(&error),
                format!("{error}; {advice}"),
                "the two halves have to rejoin where they were joined before"
            );

            let said = error.to_string();
            let error: anyhow::Error = anyhow::Error::new(error);
            let out = rendered(&error);
            assert!(out.contains(&said), "{out}");
            assert!(out.contains("hint:"), "{out}");
            assert!(out.contains(advice), "{out}");

            let json = error_json_for(&error, None);
            assert_eq!(json["error"]["hint"], advice);
            assert_eq!(json["error"]["message"], said);
        }

        // Everything else has nothing to advise, and an invented hint would be
        // worse than none.
        assert_eq!(advice_for(&IgError::TooManyRedirects), None);
        assert_eq!(
            what_instagram_said(&IgError::TooManyRedirects),
            IgError::TooManyRedirects.to_string()
        );
    }

    /// A cooldown the backstop met says when it lifts, in prose and in JSON.
    #[test]
    fn a_cooldown_from_the_client_says_when_it_lifts() {
        let until_ms = EpochMs::new(1_700_000_000_000);
        let error = anyhow::Error::new(snob_ig::error::IgError::InCooldown { until_ms });

        let out = rendered(&error);
        assert!(out.contains("hint:"), "{out}");
        assert!(out.contains("it lifts "), "{out}");

        let json = error_json_for(&error, None);
        assert_eq!(
            json["error"]["cooldown_until"],
            serde_json::json!(until_ms.to_epoch())
        );
    }

    /// One hint, whoever wrote it. A reader is being told what to do, and two
    /// answers to that is worse than either of them alone.
    #[test]
    fn a_failure_carries_one_piece_of_advice() {
        let from_the_client: anyhow::Error =
            anyhow::Error::new(snob_ig::error::IgError::SessionExpired);
        assert_eq!(rendered(&from_the_client).matches("hint:").count(), 1);

        // A command's own advice is the one that shows, and the client's is
        // not consulted. They cannot in fact meet — an `ExitError` carries no
        // source, so nothing of Instagram's is ever underneath one — but the
        // order is written down rather than left to that.
        let from_the_command: anyhow::Error =
            ExitError::new(ExitCode::NoSession, "no session is stored")
                .with_hint("run \"snob login\"")
                .into();
        let out = rendered(&from_the_command);
        assert_eq!(out.matches("hint:").count(), 1, "{out}");
        assert!(out.contains("run \"snob login\"\n"), "{out}");
    }

    /// The pager reports a condition and this is where it becomes a sentence,
    /// so this is where the sentence is asserted on.
    ///
    /// `pager.rs`'s own tests match on the variant, which is the right test
    /// over there and leaves the words untested unless something checks them
    /// here.
    #[test]
    fn every_warning_the_walk_raises_says_something() {
        assert!(
            pager_warning(Warning::EmptyAndNoCounter).contains("came back empty"),
            "{}",
            pager_warning(Warning::EmptyAndNoCounter)
        );
        for warning in [
            Warning::SameCursorTwice,
            Warning::TwoEmptyPages,
            Warning::GoingInCircles,
            Warning::EmptyAndNoCounter,
        ] {
            assert!(!pager_warning(warning).is_empty(), "{warning:?}");
        }

        // The three that carry numbers name both of them: a shortfall with
        // only one of its halves shown says nothing about how big it is.
        let short = pager_warning(Warning::StoppedShort {
            walked: 39,
            declared: 21_631,
        });
        assert!(short.contains("39") && short.contains("21631"), "{short}");

        let deleted = pager_warning(Warning::ShortOfDeclared {
            walked: 80,
            declared: 100,
        });
        assert!(
            deleted.contains("80") && deleted.contains("100"),
            "{deleted}"
        );

        let repeated = pager_warning(Warning::Repeated {
            repeated: 9,
            received: 150,
        });
        assert!(
            repeated.contains("9 of the 150") && repeated.contains("cannot be compared"),
            "{repeated}"
        );
    }

    /// Only a full walk has nothing to explain. Every other ending owes the
    /// user a reason, and a missing one would print an empty clause.
    #[test]
    fn every_incomplete_ending_has_something_to_say() {
        assert_eq!(why_incomplete(StopReason::Completed), None);
        for reason in StopReason::ALL {
            if reason != StopReason::Completed {
                assert!(why_incomplete(reason).is_some(), "{reason:?}");
            }
        }
    }
}
