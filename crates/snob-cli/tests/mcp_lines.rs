//! Every command line `snob mcp` builds for a child is one the command line
//! takes: built by the server's own planner (`snob_mcp::tools::plan`), parsed
//! by clap's grammar here, where the dependency points down.

#![cfg(feature = "mcp")]

use clap::Parser as _;
use serde_json::{Value, json};
use snob_cli::cli::Cli;
use snob_mcp::args::ConfirmedVia;
use snob_mcp::tools::{Confirmed, Offer, Plan, plan};

const EVERYTHING: Offer<'static> = Offer {
    writes: true,
    downloads: Some("/srv/snob downloads"),
    accounts: true,
    pinned: None,
};

const BY_CHAT: Confirmed<'static> = Confirmed {
    via: ConfirmedVia::Chat,
    client: Some("client 1"),
    writes_a_day: 10,
};

fn planned(tool: &str, args: Value) -> Plan {
    plan(tool, args.as_object().unwrap(), EVERYTHING).unwrap()
}

#[track_caller]
fn taken(line: impl IntoIterator<Item = String>) {
    let parsed = Cli::try_parse_from(std::iter::once("snob".to_string()).chain(line));
    assert!(parsed.is_ok(), "{parsed:?}");
}

#[test]
fn a_call_that_names_its_account_is_a_line_the_command_line_takes() {
    let follow = planned(
        "snob_follow",
        json!({"account": "someone", "as_account": "43"}),
    );
    taken(
        follow
            .globals()
            .into_iter()
            .chain(follow.command_line(None)),
    );
}

#[test]
fn signing_one_account_out_is_a_line_the_command_line_takes() {
    let any = Offer {
        writes: false,
        downloads: None,
        ..EVERYTHING
    };
    let sign_out = plan(
        "snob_logout",
        json!({"account": "@other"}).as_object().unwrap(),
        any,
    )
    .unwrap();
    taken(
        sign_out
            .globals()
            .into_iter()
            .chain(sign_out.command_line(Some(&BY_CHAT))),
    );
}

#[test]
fn the_stored_captures_and_their_changes_are_lines_the_command_line_takes() {
    for (tool, args) in [
        (
            "snob_snapshots",
            json!({"account": "@someone", "list": "followers"}),
        ),
        (
            "snob_changes",
            json!({"account": "someone", "list": "following", "from": 12, "to": 31}),
        ),
        ("snob_changes", json!({"since": "7d"})),
        ("snob_changes", json!({"since": "2026-11-01"})),
    ] {
        taken(planned(tool, args).args);
    }
}

#[test]
fn stories_and_highlights_are_lines_the_command_line_takes() {
    for (tool, args) in [
        ("snob_stories", json!({"account": "someone"})),
        (
            "snob_stories",
            json!({"account": "someone", "pictures": true}),
        ),
        ("snob_highlights", json!({"account": "someone"})),
        (
            "snob_highlights",
            json!({"account": "someone", "highlight": 2, "pictures": true}),
        ),
    ] {
        taken(planned(tool, args).args);
    }
}

#[test]
fn a_download_is_a_line_the_command_line_takes() {
    taken(planned("snob_post_download", json!({"link": "DTYHCKvDNxy"})).args);
}

#[test]
fn a_trays_download_is_a_line_the_command_line_takes() {
    for (tool, args) in [
        ("snob_stories_download", json!({})),
        (
            "snob_stories_download",
            json!({"account": "someone", "items": [1, 3]}),
        ),
        (
            "snob_highlights_download",
            json!({"account": "someone", "highlight": 2}),
        ),
        (
            "snob_highlights_download",
            json!({"account": "someone", "highlight": 2, "items": [2, 1]}),
        ),
    ] {
        taken(planned(tool, args).args);
    }
}

#[test]
fn a_write_checked_ahead_is_a_line_the_command_line_takes() {
    let follow = planned("snob_follow", json!({"account": "someone"}));
    taken(follow.ahead_line(3).unwrap());
}

#[test]
fn what_a_job_is_started_with_is_a_command_a_job_may_run() {
    let arguments = json!({"list": "fans", "account": "x", "wait_seconds": 45, "offset": 0});
    let walk = planned("snob_list", arguments.clone());
    let line = walk.start_line(
        "snob_list",
        arguments.as_object().unwrap(),
        Some("client 1"),
        Some(&BY_CHAT),
    );
    let after = line.iter().position(|arg| arg == "--").unwrap();
    assert!(snob_cli::jobs::argv::check(&line[after + 1..]).is_ok());
}

#[test]
fn a_grid_read_told_where_to_stop_is_a_line_the_command_line_takes() {
    let grid = planned(
        "snob_posts",
        json!({
            "account": "someone",
            "until": [
                "https://www.instagram.com/reel/DTYHCKvDNxy/?igsh=secret",
                "DTYHCKvDNxy",
                "BTYHCKvDNxy",
            ],
        }),
    );
    taken(grid.args);
}
