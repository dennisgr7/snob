//! `snob snapshots`: the captures of an account's lists stored here.
//!
//! **Sends nothing and writes nothing** (`snob_engine::history`).

use anyhow::Result;
use snob_core::model::ListKind;
use snob_engine::exit::ExitCode;
use snob_engine::history::{self, Listed};
use snob_store::paths::AccountPaths;

use crate::cli::SnapshotsArgs;
use crate::output::grid::{Grid, Value};
use crate::output::{self, Presentation};

/// How the account a stored report is about is named: the viewer's label,
/// or the name it was asked by.
pub(crate) fn whose(stored: &history::Stored) -> String {
    match &stored.named {
        Some(name) => format!("@{}", snob_core::model::printable(name)),
        None => {
            snob_engine::sentences::acting_as().map_or_else(|| "your account".into(), |v| v.label())
        }
    }
}

pub fn run(args: SnapshotsArgs, account: &AccountPaths) -> Result<ExitCode> {
    let destination = args.output.path.as_deref();
    let format = output::effective_format(args.output.format, destination);
    output::check_destination(format, destination)?;

    let stored = history::open_or_refuse(account, args.target.as_deref())?;
    let whose = whose(&stored);
    let listed = stored.captures(args.list.map(ListKind::from))?;

    let grid = grid(&listed);
    let rendered = grid.render(format, Presentation::detect(destination))?;
    output::write_rendered(&rendered, destination)?;
    crate::ui::info(&match listed.len() {
        0 => format!("No capture of {whose}'s lists is stored."),
        1 => format!("1 capture of {whose}'s lists is stored."),
        n => format!("{n} captures of {whose}'s lists are stored."),
    });
    Ok(ExitCode::Ok)
}

/// One row per capture, newest first.
fn grid(listed: &[Listed]) -> Grid {
    Grid {
        columns: vec![
            ("id", "Id"),
            ("list", "List"),
            ("taken_at", "Ended"),
            ("started_at", "Began"),
            ("source", "Source"),
            ("accounts", "Accounts"),
            ("declared", "Declared"),
            ("complete", "Whole"),
            ("stopped", "Stopped"),
        ],
        rows: listed
            .iter()
            .map(|listed| {
                let snapshot = &listed.snapshot;
                vec![
                    Value::Number(snapshot.id.max(0) as u64),
                    Value::Text(snapshot.kind.as_str().to_string()),
                    snapshot.taken_at.map_or(Value::Empty, Value::Moment),
                    Value::Moment(snapshot.started_at),
                    Value::Text(listed.source.clone()),
                    Value::Number(snapshot.member_count),
                    snapshot.declared_count.map_or(Value::Empty, Value::Number),
                    Value::Flag(listed.complete),
                    Value::text(listed.stopped_by.as_deref()),
                ]
            })
            .collect(),
    }
}
