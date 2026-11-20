//! `snob changes`: who joined, left or was renamed between two stored captures
//! of a list.
//!
//! **Sends nothing and writes nothing** (`snob_engine::history`): no request,
//! so it answers in a cooldown too, and no mark moved, so it is not the
//! monitor and asking twice gets one answer.

use anyhow::Result;
use serde_json::{Value as Json, json};
use snob_core::model::{ListKind, User};
use snob_engine::exit::ExitCode;
use snob_engine::history::{self, Compared, Missing, Pick, Snapshot};
use snob_store::paths::AccountPaths;

use crate::cli::{ChangesArgs, Format};
use crate::output::grid::{Grid, Value};
use crate::output::{self, Presentation, Rendered};

pub fn run(args: ChangesArgs, account: &AccountPaths) -> Result<ExitCode> {
    let destination = args.output.path.as_deref();
    let format = output::effective_format(args.output.format, destination);
    output::check_destination(format, destination)?;

    let stored = history::open_or_refuse(account, args.target.as_deref())?;
    let whose = crate::commands::snapshots::whose(&stored);
    let pick = pick(&args, snob_core::clock::now());
    let kinds = stored.kinds(args.list.map(ListKind::from), &pick)?;
    let compared = stored.compare_all(&kinds, &pick)?;

    let rendered = match format {
        Format::Json => {
            let mut text = serde_json::to_string_pretty(&document(&whose, &compared))?;
            text.push('\n');
            Rendered::Text(text)
        }
        other => grid(&compared).render(other, Presentation::detect(destination))?,
    };
    output::write_rendered(&rendered, destination)?;
    for one in &compared {
        crate::ui::info(&said(&whose, one));
        if let Some(oldest) = &one.oldest_instead {
            crate::ui::warn(&format!(
                "no {} capture is that old; compared from the oldest one stored, of {}",
                one.kind,
                snob_engine::sentences::stored_on(oldest.taken_at.unwrap_or_default())
            ));
        }
    }
    Ok(ExitCode::Ok)
}

/// The two captures `--from`, `--to` and `--since` ask for.
fn pick(args: &ChangesArgs, now: snob_core::Epoch) -> Pick {
    match (args.from, args.to) {
        (Some(from), to) => Pick::Between { from, to },
        (None, Some(to)) => Pick::UpTo(to),
        (None, None) => args
            .since
            .as_ref()
            .map_or(Pick::Newest, |since| Pick::Since(since.at(now))),
    }
}

/// The sentence on standard error for one list.
fn said(whose: &str, one: &Compared) -> String {
    let Some((before, after)) = &one.between else {
        return match &one.alone {
            Some((capture, Missing::Later)) => format!(
                "{whose}'s {} list has no whole capture newer than the one of {} ({}), so \
                 there is nothing to compare it with yet; a later walk gives one.",
                one.kind,
                snob_engine::sentences::stored_on(capture.taken_at.unwrap_or_default()),
                capture.id,
            ),
            Some((capture, Missing::Earlier)) => format!(
                "{whose}'s {} list has no whole capture older than the one of {} ({}), so \
                 there is nothing to compare it with.",
                one.kind,
                snob_engine::sentences::stored_on(capture.taken_at.unwrap_or_default()),
                capture.id,
            ),
            None => format!(
                "{whose}'s {} list has fewer than two whole captures stored, so there is \
                 nothing to compare yet; a later walk gives one.",
                one.kind
            ),
        };
    };
    format!(
        "{whose}'s {}: {} gained, {} lost, {} renamed, between the capture of {} ({}) and the \
         one of {} ({}).",
        one.kind,
        one.diff.gained.len(),
        one.diff.lost.len(),
        one.renamed.len(),
        snob_engine::sentences::stored_on(before.taken_at.unwrap_or_default()),
        before.id,
        snob_engine::sentences::stored_on(after.taken_at.unwrap_or_default()),
        after.id,
    )
}

/// `--format json`: one entry per list, with the two captures it compares.
fn document(whose: &str, compared: &[Compared]) -> Json {
    let capture = |snapshot: &Snapshot| json!({ "id": snapshot.id, "taken_at": snapshot.taken_at });
    json!({
        "account": snob_core::model::printable(whose),
        "lists": compared.iter().map(|one| json!({
            "list": one.kind.as_str(),
            "from": one.between.as_ref().map(|(before, _)| capture(before)),
            "to": one.between.as_ref().map(|(_, after)| capture(after)),
            "gained": output::readable(&one.diff.gained),
            "lost": output::readable(&one.diff.lost),
            "renamed": one.renamed.iter().map(|rename| json!({
                "pk": rename.pk,
                "from": snob_core::model::printable(&rename.from),
                "to": snob_core::model::printable(&rename.to),
                "at": rename.at,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

/// Every other format: one row per change.
fn grid(compared: &[Compared]) -> Grid {
    let mut rows = Vec::new();
    for one in compared {
        let Some((before, after)) = &one.between else {
            continue;
        };
        let ids = [
            Value::Number(before.id.max(0) as u64),
            Value::Number(after.id.max(0) as u64),
        ];
        let account = |change: &str, user: &User| {
            let mut row = vec![
                Value::Text(one.kind.as_str().into()),
                Value::Text(change.into()),
                Value::Number(user.pk.get()),
                Value::text(Some(&user.username)),
                Value::text(user.full_name.as_deref()),
                Value::Empty,
            ];
            row.extend(ids.clone());
            row
        };
        rows.extend(one.diff.gained.iter().map(|user| account("gained", user)));
        rows.extend(one.diff.lost.iter().map(|user| account("lost", user)));
        for rename in &one.renamed {
            let mut row = vec![
                Value::Text(one.kind.as_str().into()),
                Value::Text("renamed".into()),
                Value::Number(rename.pk.get()),
                Value::text(Some(&rename.to)),
                Value::Empty,
                Value::text(Some(&rename.from)),
            ];
            row.extend(ids.clone());
            rows.push(row);
        }
    }
    Grid {
        columns: vec![
            ("list", "List"),
            ("change", "Change"),
            ("pk", "Id"),
            ("username", "Username"),
            ("full_name", "Full name"),
            ("previous_username", "Was"),
            ("from_snapshot", "From"),
            ("to_snapshot", "To"),
        ],
        rows,
    }
}
