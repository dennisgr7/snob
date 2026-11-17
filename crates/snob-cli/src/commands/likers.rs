//! `snob likers LINK`: the accounts that liked a post, as many as Instagram
//! shows the web app (`snob_engine::posts::likers`, which says how many that
//! is); snob says it is a sample, beside the count when the owner did not
//! hide it.
//!
//! **They are other people's accounts**, so this keeps the rule a list walk
//! keeps (AGENTS.md, "Consent"): asked first, before any request, since the
//! question cannot name the owner before the post is read; `-y` answers in
//! advance, and no terminal without `-y` is exit 130 with nothing sent.

use anyhow::Result;
use snob_core::model::post::Likers;
use snob_engine::app::App;
use snob_engine::exit::{ExitCode, ExitError};
use snob_engine::posts::{self, LIKERS_AT_MOST};
use snob_ig::shortcode::Shortcode;
use snob_store::paths::AccountPaths;
use snob_store::secrets::SecretStore;

use crate::cli::{Format, LikersArgs};
use crate::commands::common;
use crate::output::Rendered;
use crate::ui;

pub async fn run(args: LikersArgs, store: SecretStore, paths: &AccountPaths) -> Result<ExitCode> {
    if args.check_ahead {
        Shortcode::parse(&args.link).map_err(|e| {
            ExitError::new(
                ExitCode::Error,
                format!("{e}: give a post's or a reel's link"),
            )
        })?;
        return check_ahead(&store, paths);
    }
    // Before anything is opened: a link that names no post costs nothing,
    // and a destination that cannot hold the format must not cost a request.
    let link = Shortcode::parse(&args.link).map_err(|e| {
        ExitError::new(
            ExitCode::Error,
            format!("{e}: give a post's or a reel's link"),
        )
    })?;
    let destination = common::destination(&args.output)?;
    // Refuses in a cooldown, before anything is asked.
    let mut app = common::reader(&store, paths, false, false)?;

    // Costs nothing to know, so it is known before the question.
    refuse_short_day(u64::from(app.client().pacer().accounts_left()?))?;

    ask(&app, args.consent.yes, ui::can_be_asked(), link.code()).await?;
    common::record_agent_act(&mut app, &args.agent, "likers", Some(link.code()), None)?;

    let post = posts::fetch_link(app.client(), &link).await?;
    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }
    let likers = posts::likers(app.client(), &post).await?;
    ui::info(&shown_line(&likers));

    let accounts: Vec<_> = likers
        .accounts
        .iter()
        .map(|liker| liker.account.clone())
        .collect();
    match destination.format() {
        Format::Json | Format::Ndjson => {
            let rows = likers
                .accounts
                .iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?;
            let mut text =
                crate::output::media::rows_or_envelope(rows, destination.format(), |_| {
                    serde_json::to_value(&likers).unwrap_or_default()
                })?;
            if !text.is_empty() {
                text.push('\n');
            }
            destination.write_rendered(&Rendered::Text(text))?;
        }
        _ => destination.write(&accounts)?,
    }
    Ok(ExitCode::Ok)
}

/// The refusal of a read the day's accounts cannot pay for, with `left` of
/// them: one home, for the run and for the check `snob mcp` makes first.
fn refuse_short_day(left: u64) -> Result<()> {
    if left >= u64::from(LIKERS_AT_MOST) {
        return Ok(());
    }
    Err(ExitError::new(
        ExitCode::Error,
        format!(
            "the accounts this account may read today are nearly spent ({left} left), \
             and a post's likers can be {LIKERS_AT_MOST}; nothing was sent"
        ),
    )
    .with_hint("they free up over 24 hours; \"snob status\" says how many are left")
    .into())
}

/// What `snob mcp` asks before it puts the question to the person
/// (`--check-ahead`): what would refuse the read now, **sending nothing and
/// writing nothing**.
fn check_ahead(store: &SecretStore, paths: &AccountPaths) -> Result<ExitCode> {
    let now = snob_core::clock::now_ms();
    common::alone_ahead(&store.session_of(paths), paths, now)?;
    let (_store, budget) = snob_engine::app::standing(paths, now)?;
    if let Some(held) = snob_engine::app::held_in(&budget, paths, now) {
        return Err(common::in_cooldown(&held, "nothing can be sent"));
    }
    refuse_short_day(budget.accounts_left())?;
    ui::say!("{}", serde_json::json!({ "would": "send" }));
    Ok(ExitCode::Ok)
}

/// Asks before the likers of `code` are read, as a walk of somebody else's list
/// asks.
async fn ask(app: &App, yes: bool, someone_is_there: bool, code: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !someone_is_there {
        return Err(snob_engine::sentences::refuse_unattended(
            format!("reading the likers of {code} needs confirmation"),
            "Pass -y to confirm in advance.".to_string(),
        ));
    }
    app.warn(crate::report::READING_A_POSTS_LIKERS);
    if !ui::confirm_off_thread(
        app.progress(),
        format!("Read the accounts that liked {code}?"),
        false,
    )
    .await?
    {
        return Err(ExitError::new(
            ExitCode::Interrupted,
            format!("nothing was read: the likers of {code} were not confirmed"),
        )
        // As a list's question answered no (`sentences::refuse_declined`),
        // so a script tells it from a Ctrl+C, which shares the exit code.
        .with_reason("declined")
        .into());
    }
    Ok(())
}

/// What standard error says about how much of the list this is.
fn shown_line(likers: &Likers) -> String {
    let shown = posts::grouped(likers.shown as u64);
    let accounts = if likers.shown == 1 {
        "account"
    } else {
        "accounts"
    };
    match likers.likes {
        _ if likers.complete => format!("all {shown} {accounts} that liked it"),
        Some(likes) => format!(
            "{shown} of the {} accounts that liked it: Instagram shows a sample, the accounts \
             you know first",
            posts::grouped(likes)
        ),
        None => format!(
            "{shown} {accounts} that liked it; the owner hid how many there are, and Instagram \
             shows a sample"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn likers(likes: Option<u64>, shown: usize) -> Likers {
        Likers {
            code: "DbGwql4IMei".into(),
            url: "https://www.instagram.com/p/DbGwql4IMei/".into(),
            likes,
            shown,
            complete: likes == Some(shown as u64),
            accounts: Vec::new(),
        }
    }

    #[test]
    fn the_likers_say_they_are_a_sample() {
        assert_eq!(
            shown_line(&likers(Some(603_328), 105)),
            "105 of the 603,328 accounts that liked it: Instagram shows a sample, the \
             accounts you know first"
        );
        assert_eq!(
            shown_line(&likers(Some(2), 2)),
            "all 2 accounts that liked it"
        );
    }

    #[test]
    fn hidden_counts_withhold_the_total() {
        let line = shown_line(&likers(None, 40));
        assert!(
            line.starts_with("40 accounts that liked it; the owner hid"),
            "{line}"
        );
        assert!(!line.contains(" of "), "{line}");
    }
}
