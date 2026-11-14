//! The sentences a person reads.
//!
//! The other half of a report from `super::wire`, and the split is by audience
//! rather than by layer: that module decides what a receiver is sent, this one
//! decides what is printed. Rewording anything here is free; rewording anything
//! there is an integration change.
//!
//! What actually moved is `crate::engine::watch`'s answer and nothing in here
//! recomputes any of it — the module rule that `engine` never decides how
//! something looks has a matching half, which is that this never decides what
//! is true.

use snob_core::Epoch;
use snob_core::model::{ListKind, User, printable};
use snob_core::watch::{Basis, ListDiff};

use crate::engine::Provenance;
use crate::engine::check::Problem;
use crate::engine::watch::{Skipped, TickReport, WatchReport};
use crate::engine::watch_posts::{PostsReport, PostsSkipped, SavedPost};
use crate::report;
use snob_core::watch::posts::Basis as PostsBasis;

/// Why a list was not compared, said in a sentence.
///
/// `engine` handed over what happened and nothing else; which words that
/// deserves is this module's question, which is why the tokens are matched
/// here rather than carried as strings.
pub(super) fn refusal_line(kind: ListKind, skipped: Skipped) -> String {
    match skipped {
        Skipped::NobodyLooked(provenance) => format!(
            "the {kind} list was served from storage and nothing checked whether it is still \
             true{}, so it was not compared and the monitor did not move on",
            match provenance {
                Provenance::Cooldown => " (the account is in cooldown)",
                Provenance::PollFailed => " (the check failed)",
                Provenance::WalkedRecently => {
                    " (it changed, but it was walked less than a day ago; a later run walks it)"
                }
                _ => "",
            }
        ),
        // The same half-sentence the summary uses for the same situation.
        // Writing a second one here is how two commands end up describing one
        // event in two ways.
        Skipped::Incomplete(reason, _) => format!(
            "the {kind} list could not be read in full ({}), so it was not compared: the \
             accounts missing from it would have been reported as people who left",
            report::why_incomplete(reason).unwrap_or("it stopped early")
        ),
    }
}

/// Why the posts of `account` were not looked at, said in a sentence; the
/// sibling of [`refusal_line`]. `None` for posts read less than an hour ago,
/// which is the rule working and not worth a warning.
pub(super) fn posts_refusal_line(account: Option<&str>, skipped: PostsSkipped) -> Option<String> {
    let whose = match account {
        Some(name) => format!("the posts of @{}", printable(name)),
        None => "the posts".to_string(),
    };
    Some(match skipped {
        PostsSkipped::ReadRecently => return None,
        PostsSkipped::Cooldown => {
            format!("{whose} were not read: the account is in cooldown, and nothing is sent")
        }
        PostsSkipped::Incomplete(reason, _) => format!(
            "{whose} could not be read in full ({}), so nothing was concluded about them; the \
             next run reads them again",
            report::why_incomplete(reason).unwrap_or("it stopped early")
        ),
        PostsSkipped::Hidden => {
            format!("{whose} are not shown to you: the account is private and you do not follow it")
        }
        PostsSkipped::NoBrowser => format!(
            "{whose} were not read: a grid is read from the browser, and SNOB_NO_BROWSER is set"
        ),
    })
}

/// What is wrong with something `snob watch check` looked at, said in a
/// sentence.
///
/// The same split as [`refusal_line`] above, for the other half of the monitor:
/// `engine::check` hands over what it found and this decides the words, for
/// the terminal report and `--json` alike.
///
/// Two of them are consts in `report` rather than literals here.
/// `NOTHING_CONFIGURED` because `watch::status` answers that same question
/// about the same machine and the two probes must not disagree, and
/// `NO_RECORDED_CONSENT` because two arms below are one condition with one
/// clause between them.
pub(super) fn problem_line(problem: &Problem) -> String {
    match problem {
        Problem::NothingConfigured => report::NOTHING_CONFIGURED.to_string(),
        Problem::NeverFires => "this schedule never fires: no moment it names exists".to_string(),
        Problem::Unbuildable(why) => format!("this schedule cannot be built: {why}"),
        Problem::NoSchedule => "the file names no schedule: \"snob watch\" runs only with one \
             on its command line, as --every 6h, and \"watch once --when-due\" refuses"
            .to_string(),
        Problem::InCooldown { held } => format!("not checked: {}", report::held_until(held)),
        // One condition, one wording, and a clause for the account a cooldown
        // stopped anything else being checked about.
        Problem::NoRecordedConsent { in_cooldown: false } => {
            report::NO_RECORDED_CONSENT.to_string()
        }
        Problem::NoRecordedConsent { in_cooldown: true } => format!(
            "{} (and it is in cooldown, so nothing else was checked)",
            report::NO_RECORDED_CONSENT
        ),
        Problem::SessionSilent => "not checked: the session is not responding".to_string(),
        Problem::NoSession => "no session is stored; run \"snob login\"".to_string(),
        Problem::SignedOutHere => format!("{}; {}", report::SIGNED_OUT_HERE, report::SIGN_IN_AGAIN),
        Problem::NoUsername => "not checked: the session carries no username, and Instagram \
             did not name the account either"
            .to_string(),
        Problem::CountersUnknowable => {
            "Instagram would not serve this account's profile, so its id came from \
             search, which carries no counters. A scheduled run will work, but it \
             cannot tell a truncated list from a complete one."
                .to_string()
        }
        Problem::FirstRunLaysTheBaseline => {
            "the first scheduled run lays the baseline down and reports no changes; \
             the second one onwards reports them"
                .to_string()
        }
        Problem::NotPosted => "--no-webhook, so nothing was posted; the address and the headers \
             were still checked"
            .to_string(),
        Problem::PostsNeedABrowser => {
            "its posts are read from the browser, and SNOB_NO_BROWSER is \
             set: runs will read its lists and skip its posts"
                .to_string()
        }
        Problem::PostsHidden => "its posts are not shown to you: the account is private and you \
             do not follow it"
            .to_string(),
        Problem::NoFfmpeg => "quality = \"best\" joins a video's tracks with ffmpeg, and none \
             was found, so every run refuses to start; install it, or say \"compat\""
            .to_string(),
        Problem::CannotSaveTo(dir) => format!(
            "{} cannot be made or written, so the new posts would be reported and not saved",
            printable(dir)
        ),
        Problem::FirstRunLaysThePostsBaseline => {
            "the first run takes the posts on its grid as seen and reports none; the \
             runs after it report what is new"
                .to_string()
        }
        // Not this module's sentence and not rewritten into one: it is what the
        // schedule parser, the store, Instagram or the user's own receiver
        // said, and that text is the only thing identifying the cause.
        Problem::Foreign(said) => said.clone(),
    }
}

/// Says that reports were abandoned, because nothing else will.
///
/// The sweep marks a report `expired` once it is too old to be news, and that
/// is the moment a set of arrivals and departures stops existing: `due` has
/// already been refusing to hand it back, so the retry ladder never reaches the
/// sentence in `send_one` that was written for exactly this. Without this line
/// the event would be a row changing state in silence: `status` counts only
/// `pending`, and the health verdict goes from `warning` to `ok` at the instant
/// the news is lost.
pub(super) fn say_what_was_given_up(given_up: usize) {
    if given_up == 0 {
        return;
    }
    crate::ui::warn(&given_up_sentence(given_up));
}

/// The abandoned-reports sentence, plural tuple and all, written once.
///
/// `status` prints the same event -- rows the sweep marked `expired` -- so both
/// read this one string rather than two copies that drift.
pub(super) fn given_up_sentence(given_up: usize) -> String {
    let (subject, what) = if given_up == 1 {
        ("report was", "what it said is")
    } else {
        ("reports were", "what they said is")
    };
    format!(
        "{given_up} {subject} given up on for being too old to be news; \
         {what} not reported a second time."
    )
}

/// What a tick found, as a person reads it: [`describe`], with what is owed
/// to the posts when they were watched and not looked at.
pub(super) fn describe_tick(tick: &TickReport) -> Vec<String> {
    let posts_skipped = tick.posts_skipped();
    let lists_refused = tick.lists.iter().any(|list| list.skipped.is_some());
    // Posts read within the hour are no refusal.
    let posts_refused = posts_skipped.is_some_and(|skipped| skipped != PostsSkipped::ReadRecently);
    if tick.lists.is_empty() && posts_skipped == Some(PostsSkipped::ReadRecently) {
        let who = crate::app::label(tick.report.account_pk, tick.report.username.as_deref());
        return vec![format!(
            "The posts of {who} were read less than an hour ago, so they were not read again \
             this time."
        )];
    }
    // An entry of posts alone that could not look at them looked at nothing.
    let mut lines = describe(
        &tick.report,
        lists_refused || (tick.lists.is_empty() && posts_refused),
    );
    // And beside the lists, said in the summary, which is all a chat format
    // is sent.
    if posts_refused && !tick.lists.is_empty() {
        lines.push(String::new());
        lines.push("Its posts could not be read this time.".to_string());
    }
    lines
}

/// The answer as a person reads it.
///
/// Returned as lines rather than printed, so a test can read them without
/// capturing standard output.
///
/// The lists first, then the posts. An account whose posts alone were
/// reported says nothing about its lists, which it does not watch.
pub(super) fn describe(report: &WatchReport, refused: bool) -> Vec<String> {
    let lists_reported = report.followers.is_some() || report.following.is_some();
    let mut lines = if lists_reported || report.posts.is_none() {
        describe_lists(report, refused)
    } else {
        Vec::new()
    };
    if let Some(posts) = &report.posts {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(posts_lines(report, posts));
    }
    lines
}

/// What the posts of `report`'s account came to.
fn posts_lines(report: &WatchReport, posts: &PostsReport) -> Vec<String> {
    let who = crate::app::label(report.account_pk, report.username.as_deref());
    if posts.basis == PostsBasis::Baseline {
        return vec![format!(
            "The posts of {who} have never been reported on, so there is nothing to compare \
             them against. The next run is the first that can say anything."
        )];
    }
    let mut lines = Vec::new();
    if posts.news.is_empty() {
        lines.push(format!("No new posts from {who} since the last report."));
    } else {
        let since = posts
            .since
            .map(|since| format!(" since {}", report::stored_on(since)))
            .unwrap_or_default();
        lines.push(format!("New posts from {who}{since}: {}", posts.news.len()));
        for post in &posts.news {
            lines.push(format!(
                "  {}  {}  {}",
                report::stored_on(post.taken_at),
                kind_words(post.kind),
                post.link()
            ));
            let headline = post.headline();
            if !headline.trim().is_empty() {
                lines.push(format!("    {}", one_line(headline)));
            }
            if let Some(saved) = posts.saved_of(post.pk) {
                lines.extend(saved_lines(saved));
            }
        }
    }
    if !posts.complete {
        let name = report
            .username
            .as_deref()
            .map(printable)
            .unwrap_or_default();
        lines.push(format!(
            "Only the newest {} posts were read, and none of them was one seen before: posts \
             from before them may have gone unreported. \"snob posts {name} --pages 10\" \
             reads further back.",
            posts.news.len()
        ));
    }
    lines
}

/// What a post is, in a few words.
fn kind_words(kind: crate::posts::PostKind) -> String {
    match kind {
        crate::posts::PostKind::Carousel(n) => format!("carousel of {n}"),
        other => other.label(),
    }
}

/// The most of a caption a line holds: about what a notification shows on
/// one line.
const CAPTION_CHARS: usize = 80;

/// The first line of a caption, cut to [`CAPTION_CHARS`].
fn one_line(caption: &str) -> String {
    let text = printable(caption);
    if text.chars().count() <= CAPTION_CHARS {
        return text;
    }
    let cut: String = text.chars().take(CAPTION_CHARS - 1).collect();
    format!("{}\u{2026}", cut.trim_end())
}

/// What was saved of one post, and what was not.
fn saved_lines(saved: &SavedPost) -> Vec<String> {
    let mut lines = Vec::new();
    let directory = printable(&saved.directory);
    if !saved.files.is_empty() {
        let files = saved.files.len();
        lines.push(format!(
            "    saved {files} {} to {directory}",
            if files == 1 { "file" } else { "files" }
        ));
    }
    if saved.failed > 0 {
        lines.push(format!(
            "    {} could not be saved; the next run tries again while the post is on the page",
            plural_files(saved.failed)
        ));
    }
    if saved.left > 0 {
        lines.push(format!(
            "    {} left for the next run, past what one run saves",
            plural_files(saved.left)
        ));
    }
    lines
}

fn plural_files(n: usize) -> String {
    if n == 1 {
        "1 file".to_string()
    } else {
        format!("{n} files")
    }
}

/// The lists' part of [`describe`].
fn describe_lists(report: &WatchReport, refused: bool) -> Vec<String> {
    let who = crate::app::label(report.account_pk, report.username.as_deref());

    if report.followers.is_none() && report.following.is_none() {
        // A run in which every list was refused concluded nothing, and that is
        // not the same as an account nothing has ever been walked for: two
        // complete captures may be on disk. The reason comes from the refusal
        // lines the caller prints next, so this only has to not claim the
        // opposite.
        if refused {
            return vec![format!("Nothing could be looked at for {who} this time.")];
        }
        // The command names the account: a bare "snob followers" walks the
        // viewer's own, which is not the one that has nothing stored.
        let command = match (report.is_self, report.username.as_deref()) {
            (false, Some(name)) => format!("snob followers {}", printable(name)),
            _ => "snob followers".to_string(),
        };
        return vec![format!(
            "Nothing has been walked for {who} yet, so there is nothing to compare.\n\
             Run \"{command}\" once and this will have something to say from then on."
        )];
    }

    // Said before anything else, because everything after it would otherwise
    // read as "nothing happened" when what it means is "this is the first look".
    let baselines: Vec<ListKind> = [report.followers.as_ref(), report.following.as_ref()]
        .into_iter()
        .flatten()
        .filter(|r| matches!(r.basis, Basis::Baseline { .. }))
        .map(|r| r.kind)
        .collect();

    let mut lines = Vec::new();
    if !baselines.is_empty() {
        let which = baselines
            .iter()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(" and ");
        // Both lists are the common case — a first look almost always finds
        // two — so the sentence has to read for two as well as for one.
        let (noun, verb) = if baselines.len() > 1 {
            ("lists have", "them")
        } else {
            ("list has", "it")
        };
        lines.push(format!(
            "The {which} {noun} never been reported on, so there is no earlier capture to \
             compare {verb} against. The next run is the first that can say anything."
        ));
    }

    let changes = report.changes();
    if changes.is_empty() {
        if baselines.is_empty() {
            // **Say what was done, rather than asserting the negative.** Three
            // different runs reach this point and only one has earned "nothing
            // has changed".
            //
            // A list whose counter had not moved is served from storage without
            // being read, which is where nearly all of this tool's savings come
            // from and is worth keeping — walking three hundred accounts costs
            // twenty-five requests and asking whether they changed costs one. But
            // a counter cannot see a swap: one departure and one arrival leave
            // it identical, and a rename does not move it at all. Nothing is
            // lost — the walk happens once the capture ages out and the
            // comparison is against the last *reported* capture — but until
            // then the tool must not state a fact it has not checked.
            //
            // And a run where one list was refused and the other had no news
            // gets here too, because the refusal branch above is gated on there
            // being nothing stored at all and one surviving list gets past it.
            //
            // The distinction is already in the domain type:
            // `Basis::Unchanged` is documented as "this one did not have to
            // look", against a `Compare` that read both and found nothing.
            let nobody_looked = [report.followers.as_ref(), report.following.as_ref()]
                .into_iter()
                .flatten()
                .all(|r| matches!(r.basis, Basis::Unchanged { .. }));

            // With a list not compared, the headline says so itself: read
            // on its own, as a notification's first line is, "nothing has
            // changed" was an all-clear for a list nobody compared.
            lines.push(if refused {
                format!(
                    "Nothing has changed for {who} in the list that was compared since the \
                     last report."
                )
            } else {
                format!("Nothing has changed for {who} since the last report.")
            });
            if refused {
                lines.push(
                    "One of the lists could not be read this time, so this speaks only for \
                     the other one."
                        .to_string(),
                );
            } else if nobody_looked {
                lines.push(
                    "Their counters had not moved, so the lists were not read again.".to_string(),
                );
            }
            lines.push(since_line(report));
        }
        return lines;
    }

    lines.push(format!("Changes for {who}{}", period(report)));
    lines.push(String::new());

    for (kind, diff) in [
        (ListKind::Followers, &changes.followers),
        (ListKind::Following, &changes.following),
    ] {
        lines.extend(list_lines(kind, diff));
    }

    if !changes.renamed.is_empty() {
        // The verb agrees, because one rename is the common case: it is what
        // `once`, `diff` and the loop print most often. The baseline sentence
        // a screen up branches this way for the same reason.
        let renamed = changes.renamed.len();
        lines.push(format!(
            "  {renamed} now {} by another name",
            if renamed == 1 { "goes" } else { "go" }
        ));
        for r in &changes.renamed {
            lines.push(format!(
                "    @{} is now @{}",
                printable(&r.from),
                printable(&r.to)
            ));
        }
    }

    // **Said in the summary itself**, the last line of it: the chat formats
    // send the summary alone, and the refusal's own warning goes to standard
    // error, which a Slack channel never sees. Without it a report of changes
    // in one list reads as the whole story of both.
    if refused {
        lines.push(String::new());
        lines.push(
            "One of the lists could not be read this time, so these are the changes of the \
             other one alone."
                .to_string(),
        );
    }

    lines
}

pub(super) fn list_lines(kind: ListKind, diff: &ListDiff) -> Vec<String> {
    let mut lines = Vec::new();
    for (verb, who) in [("gained", &diff.gained), ("lost", &diff.lost)] {
        if who.is_empty() {
            continue;
        }
        lines.push(format!("  {kind} {verb}: {}", who.len()));
        for user in who {
            lines.push(format!("    {}", name_of(user)));
        }
    }
    lines
}

/// A name as it is drawn, filtered because it came off Instagram.
pub(super) fn name_of(user: &User) -> String {
    match user.full_name.as_deref().filter(|n| !n.trim().is_empty()) {
        Some(full) => format!("@{} ({})", user.safe_username(), printable(full)),
        None => format!("@{}", user.safe_username()),
    }
}

/// " since Aug 14 at 09:12", when there is a moment to name.
pub(super) fn period(report: &WatchReport) -> String {
    match earliest_since(report) {
        Some(since) => format!(" since {}", report::stored_on(since)),
        None => String::new(),
    }
}

pub(super) fn since_line(report: &WatchReport) -> String {
    match earliest_since(report) {
        Some(since) => format!("The last report was on {}.", report::stored_on(since)),
        None => "Nothing has been reported yet.".to_string(),
    }
}

/// The older of the two receipts.
///
/// The two lists are marked apart and can be reported at different moments, so
/// the interval the user is being shown starts at whichever was reported first
/// — saying the later one would claim a window shorter than the one the numbers
/// actually cover.
pub(super) fn earliest_since(report: &WatchReport) -> Option<Epoch> {
    [report.followers.as_ref(), report.following.as_ref()]
        .into_iter()
        .flatten()
        .filter_map(|r| r.since)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::watch::fixtures::{list, post, posts_report, report_with, user};
    use crate::engine::watch::ListReport;
    use snob_core::model::ListKind;
    use snob_core::watch::{Basis, ListDiff, Rename};
    use snob_core::{EpochMs, Pk};

    /// Every reason a check can give says something.
    ///
    /// A variant added to [`Problem`] with no arm here is a compile error; one
    /// added with an empty arm is not, and an empty problem column is a line
    /// saying something is wrong without saying what. The foreign one is the
    /// exception on purpose: it is somebody else's text and this only carries
    /// it.
    #[test]
    fn every_reason_a_check_can_give_says_something() {
        for problem in [
            Problem::NothingConfigured,
            Problem::NeverFires,
            Problem::Unbuildable("5m is too often".to_string()),
            Problem::NoSchedule,
            Problem::InCooldown {
                held: crate::app::Held {
                    until_ms: EpochMs::new(1_722_700_000_000),
                    braked: Vec::new(),
                },
            },
            Problem::NoRecordedConsent { in_cooldown: false },
            Problem::NoRecordedConsent { in_cooldown: true },
            Problem::SessionSilent,
            Problem::NoSession,
            Problem::SignedOutHere,
            Problem::NoUsername,
            Problem::CountersUnknowable,
            Problem::FirstRunLaysTheBaseline,
            Problem::NotPosted,
            Problem::PostsNeedABrowser,
            Problem::PostsHidden,
            Problem::NoFfmpeg,
            Problem::CannotSaveTo("/srv/media".to_string()),
            Problem::FirstRunLaysThePostsBaseline,
        ] {
            assert!(!problem_line(&problem).is_empty(), "{problem:?}");
        }

        // What the scheduler, Instagram or the user's own receiver said is the
        // only thing identifying the cause, so it survives whole.
        assert_eq!(
            problem_line(&Problem::Foreign("404 Not Found".to_string())),
            "404 Not Found"
        );
        assert!(
            problem_line(&Problem::Unbuildable("5m is too often".into()))
                .contains("5m is too often")
        );
    }

    /// One condition, one wording.
    ///
    /// The account nothing else was asked about because a cooldown was standing
    /// gets the consent sentence with a clause after it, not a second
    /// description of one situation.
    #[test]
    fn a_missing_consent_reads_the_same_whether_or_not_a_cooldown_stands() {
        let plain = problem_line(&Problem::NoRecordedConsent { in_cooldown: false });
        let waiting = problem_line(&Problem::NoRecordedConsent { in_cooldown: true });

        assert!(waiting.starts_with(&plain), "{waiting}");
        assert!(waiting.contains("in cooldown"), "{waiting}");
    }

    /// Somebody who has never run the tool is told to run it, not told that
    /// nothing changed — which would be true and useless.
    #[test]
    fn an_account_with_nothing_stored_is_told_what_to_run() {
        let lines = describe(&report_with(None, vec![]), false);
        assert!(lines.join("\n").contains("snob followers"), "{lines:?}");
    }

    /// A run that could not look at either list is not an account nothing has
    /// been walked for, though both are "no report to show".
    #[test]
    fn a_run_that_could_not_look_does_not_claim_the_account_is_unknown() {
        let lines = describe(&report_with(None, vec![]), true).join("\n");
        assert!(!lines.contains("snob followers"), "{lines}");
        assert!(lines.contains("could be looked at"), "{lines}");
    }

    /// The worst thing this feature could print. A first look has no earlier
    /// capture, so it must say so rather than report an empty diff as calm.
    #[test]
    fn a_first_look_says_so_instead_of_saying_nothing_changed() {
        let lines = describe(
            &report_with(
                Some(list(
                    Basis::Baseline { snapshot_id: 1 },
                    ListDiff::default(),
                    None,
                )),
                vec![],
            ),
            false,
        );
        let text = lines.join("\n");
        assert!(text.contains("never been reported"), "{text}");
        assert!(
            !text.contains("Nothing has changed"),
            "a baseline is not a quiet account: {text}"
        );
    }

    /// A first look almost always finds both lists, so the common case is the
    /// plural one.
    #[test]
    fn a_first_look_at_both_lists_says_so_in_the_plural() {
        let baseline = |kind| ListReport {
            kind,
            basis: Basis::Baseline { snapshot_id: 1 },
            since: None,
            until: Epoch::new(2_000),
            diff: ListDiff::default(),
            total: 309,
        };
        let report = WatchReport {
            account_pk: Pk::new(42),
            username: Some("me".into()),
            is_self: true,
            followers: Some(baseline(ListKind::Followers)),
            following: Some(baseline(ListKind::Following)),
            renamed: vec![],
            posts: None,
        };

        let text = describe(&report, false).join("\n");
        assert!(text.contains("lists have never been reported"), "{text}");
        assert!(!text.contains("list has never"), "{text}");
    }

    /// A run that did not read the lists must not assert that nothing changed.
    ///
    /// `Basis::Unchanged` is documented as "this one did not have to look": the
    /// counter had not moved, so the stored capture was served without being
    /// re-read, and a counter cannot see a swap.
    #[test]
    fn a_run_that_did_not_look_says_so() {
        let unread = describe(
            &report_with(
                Some(list(
                    Basis::Unchanged { snapshot_id: 7 },
                    ListDiff::default(),
                    Some(Epoch::new(1_000)),
                )),
                vec![],
            ),
            false,
        )
        .join("\n");
        assert!(
            unread.contains("were not read again"),
            "a counter poll is not a look: {unread}"
        );

        // And a run that really did read both and found nothing keeps the
        // plain sentence, because there it is true.
        let read = describe(
            &report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff::default(),
                    Some(Epoch::new(1_000)),
                )),
                vec![],
            ),
            false,
        )
        .join("\n");
        assert!(read.contains("Nothing has changed"), "{read}");
        assert!(
            !read.contains("were not read again"),
            "this one did look: {read}"
        );

        // A refused list is a third case: the refusal branch is reached only
        // when *nothing* is stored, so one surviving list gets past it.
        let partial = describe(
            &report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff::default(),
                    Some(Epoch::new(1_000)),
                )),
                vec![],
            ),
            true,
        )
        .join("\n");
        assert!(
            partial.contains("could not be read this time"),
            "a refused list must not be reported as quiet: {partial}"
        );
    }

    #[test]
    fn an_arrival_and_a_departure_are_both_named() {
        let diff = ListDiff {
            gained: vec![user(Pk::new(1), "arrived")],
            lost: vec![user(Pk::new(2), "left")],
        };
        let lines = describe(
            &report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    diff,
                    Some(Epoch::new(1_000)),
                )),
                vec![],
            ),
            false,
        );

        let text = lines.join("\n");
        assert!(text.contains("@arrived"), "{text}");
        assert!(text.contains("@left"), "{text}");
        assert!(text.contains("followers gained: 1"), "{text}");
        assert!(text.contains("followers lost: 1"), "{text}");
    }

    /// A rename on its own is news: the empty-diff check must not swallow a run
    /// whose only change was somebody's name.
    #[test]
    fn a_rename_on_its_own_is_still_reported() {
        let lines = describe(
            &report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff::default(),
                    Some(Epoch::new(1_000)),
                )),
                vec![Rename {
                    pk: Pk::new(7),
                    history_id: 7,
                    from: "before".into(),
                    to: "after".into(),
                    at: Epoch::new(1_500),
                }],
            ),
            false,
        );

        let text = lines.join("\n");
        assert!(text.contains("@before is now @after"), "{text}");
        assert!(!text.contains("Nothing has changed"), "{text}");
    }

    /// And one rename is counted as one. The test above asserts only the line
    /// *below* the count. One is the common case here: it is what `once`,
    /// `diff` and the loop print most often.
    #[test]
    fn one_rename_is_counted_as_one() {
        let renamed = |names: &[(&str, &str)]| {
            describe(
                &report_with(
                    Some(list(
                        Basis::Compare {
                            before: 1,
                            after: 2,
                        },
                        ListDiff::default(),
                        Some(Epoch::new(1_000)),
                    )),
                    names
                        .iter()
                        .enumerate()
                        .map(|(n, (from, to))| Rename {
                            pk: Pk::new(n as u64),
                            history_id: n as i64,
                            from: (*from).into(),
                            to: (*to).into(),
                            at: Epoch::new(1_500),
                        })
                        .collect(),
                ),
                false,
            )
            .join("\n")
        };

        let one = renamed(&[("before", "after")]);
        assert!(one.contains("1 now goes by another name"), "{one}");

        let two = renamed(&[("before", "after"), ("other", "later")]);
        assert!(two.contains("2 now go by another name"), "{two}");
    }

    /// A control character in a name reaches a terminal through this command
    /// like any other, so it goes through the same filter.
    #[test]
    fn a_name_is_filtered_before_it_is_drawn() {
        let lines = describe(
            &report_with(
                Some(list(
                    Basis::Compare {
                        before: 1,
                        after: 2,
                    },
                    ListDiff {
                        gained: vec![user(Pk::new(1), "bad\u{202e}name")],
                        lost: vec![],
                    },
                    Some(Epoch::new(1_000)),
                )),
                vec![],
            ),
            false,
        );
        assert!(
            !lines.join("\n").contains('\u{202e}'),
            "a bidi override reached the terminal"
        );
    }

    /// An account somebody else owns, whose posts alone are reported.
    fn school(posts: crate::engine::watch_posts::PostsReport) -> WatchReport {
        WatchReport {
            account_pk: Pk::new(9001),
            username: Some("school".into()),
            is_self: false,
            posts: Some(posts),
            ..report_with(None, vec![])
        }
    }

    #[test]
    fn new_posts_are_listed_with_their_dates_and_links() {
        let mut posts = posts_report(vec![post(2, "Open day this Friday\nmore"), post(1, "")]);
        posts.saved = vec![SavedPost {
            pk: posts.news[0].pk,
            directory: "/srv/media/school".into(),
            files: vec!["someone-a.jpg".into(), "someone-b.jpg".into()],
            failed: 1,
            left: 0,
        }];
        let lines = describe(&school(posts), false);
        let text = lines.join("\n");
        assert!(text.starts_with("New posts from @school since "), "{text}");
        assert!(lines[0].ends_with(": 2"), "{text}");
        assert!(
            lines[1].contains("photo  https://www.instagram.com/p/"),
            "{text}"
        );
        assert_eq!(lines[2], "    Open day this Friday", "only the first line");
        assert_eq!(lines[3], "    saved 2 files to /srv/media/school");
        assert!(lines[4].contains("1 file could not be saved"), "{text}");
        assert!(
            lines[5].starts_with("  "),
            "the second post, with no caption line"
        );
        assert_eq!(lines.len(), 6, "{text}");
        assert!(
            !text.contains("followers"),
            "its lists are not watched: {text}"
        );
    }

    #[test]
    fn a_baseline_of_posts_says_the_next_run_is_the_first_that_can_speak() {
        let mut posts = posts_report(Vec::new());
        posts.basis = snob_core::watch::posts::Basis::Baseline;
        posts.since = None;
        let text = describe(&school(posts), false).join("\n");
        assert!(
            text.contains("The posts of @school have never been reported on"),
            "{text}"
        );
        assert!(
            text.contains("The next run is the first that can say anything"),
            "{text}"
        );

        let quiet = describe(&school(posts_report(Vec::new())), false).join("\n");
        assert_eq!(quiet, "No new posts from @school since the last report.");
    }

    #[test]
    fn an_incomplete_read_says_so_and_how_to_read_further() {
        let mut posts = posts_report(vec![post(1, "a")]);
        posts.complete = false;
        let text = describe(&school(posts), false).join("\n");
        assert!(text.contains("may have gone unreported"), "{text}");
        assert!(text.contains("\"snob posts school --pages 10\""), "{text}");
        assert!(
            !text.contains("posts @school"),
            "no at sign to be typed back: {text}"
        );
    }

    #[test]
    fn a_long_caption_is_cut_to_a_line() {
        let long = "word ".repeat(40);
        let lines = describe(&school(posts_report(vec![post(1, &long)])), false);
        let caption = lines[2].trim_start();
        assert_eq!(caption.chars().count(), CAPTION_CHARS);
        assert!(caption.ends_with('\u{2026}'), "{caption}");
    }

    /// A report of changes says, in the summary the chat formats send, that a
    /// list could not be compared, rather than only in a warning on standard
    /// error.
    #[test]
    fn changes_with_a_list_not_compared_say_so_in_the_summary() {
        let report = report_with(
            Some(list(
                Basis::Compare {
                    before: 1,
                    after: 2,
                },
                ListDiff {
                    gained: vec![user(Pk::new(7), "newcomer")],
                    lost: vec![],
                },
                Some(snob_core::Epoch::new(1_000)),
            )),
            vec![],
        );
        let refused = describe(&report, true).join("\n");
        assert!(refused.contains("could not be read this time"), "{refused}");
        let whole = describe(&report, false).join("\n");
        assert!(!whole.contains("could not be read"), "{whole}");
    }

    /// An account with nothing stored is told the command that walks it, not
    /// one that walks the viewer's own.
    #[test]
    fn nothing_stored_names_the_command_for_that_account() {
        let other = WatchReport {
            account_pk: Pk::new(9001),
            username: Some("someone".into()),
            is_self: false,
            ..report_with(None, vec![])
        };
        let text = describe(&other, false).join("\n");
        assert!(text.contains("\"snob followers someone\""), "{text}");
        let own = describe(&report_with(None, vec![]), false).join("\n");
        assert!(own.contains("\"snob followers\" once"), "{own}");
    }

    /// Posts read within the hour are said to be, rather than read as an
    /// account nothing was ever walked for.
    #[test]
    fn posts_read_within_the_hour_are_said_to_be() {
        use crate::engine::watch::TickPosts;
        let mut tick = crate::engine::watch::TickReport::for_test(
            WatchReport {
                account_pk: Pk::new(9001),
                username: Some("school".into()),
                is_self: false,
                ..report_with(None, vec![])
            },
            0,
            snob_core::Epoch::new(1_700_000_000),
        );
        tick.posts = Some(TickPosts {
            skipped: Some(PostsSkipped::ReadRecently),
        });
        let text = describe_tick(&tick).join("\n");
        assert!(text.contains("less than an hour ago"), "{text}");
        assert!(!text.contains("snob followers"), "{text}");
    }
}
