//! `cargo harness`: every test of the desktop app from one place, the same
//! on a developer's machine, in Docker and on a CI runner.
//!
//! With no argument it asks which suites to run; with arguments it runs
//! them without asking, and a word that is no suite's name filters the
//! scenarios by name. `cargo harness demo` opens the test build by hand on
//! the fake Instagram of readable names.
//!
//! **Nothing here reaches Instagram or the real keyring**: the app under
//! test is the `testing` build, in a directory of the run's own, pointed at
//! the fake Instagram this process serves.

mod drive;
mod report;
mod scenarios;
mod suites;

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

/// The suites, in the order they run: its name, what it is, and whether a
/// pull request runs it.
pub const SUITES: [(&str, &str, bool); 7] = [
    ("logic", "the unit and in-process tests of the app", true),
    (
        "behavior",
        "the real app, driven end to end against the fake Instagram",
        true,
    ),
    (
        "visual",
        "screenshots of every screen against the approved ones",
        true,
    ),
    (
        "accessibility",
        "names, roles, focus and target sizes on every screen",
        true,
    ),
    (
        "privacy",
        "no secret written, no test mode in a release build",
        true,
    ),
    (
        "performance",
        "start, screen changes and idle cost against the baseline",
        false,
    ),
    (
        "endurance",
        "many sleeps and many account changes, watching memory",
        false,
    ),
];

/// What a run was asked for.
pub struct Asked {
    pub suites: Vec<&'static str>,
    /// Only the scenarios whose name holds one of these.
    pub filters: Vec<String>,
    /// Takes the screenshots of this run as the approved ones.
    pub approve: bool,
    pub demo: bool,
    /// Where the run writes: its report, its screenshots, its logs.
    pub out: PathBuf,
}

/// Where the app's own files are: the directory of its manifest.
pub fn app_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Where Cargo builds the app: `CARGO_TARGET_DIR` when it is set, as in the
/// container, and the app's own `target` otherwise.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| app_dir().join("target"), PathBuf::from)
}

fn usage() -> String {
    let mut text = String::from(
        "cargo harness [SUITE ...] [FILTER ...] [--approve] [--out DIR]\ncargo harness demo\n\n\
         With no suite, asks which to run. Suites:\n",
    );
    for (name, what, quick) in SUITES {
        let when = if quick { "" } else { " (nightly)" };
        text.push_str(&format!("  {name:<14} {what}{when}\n"));
    }
    for (name, what) in [
        ("all", "every suite"),
        ("quick", "the suites a pull request runs"),
    ] {
        text.push_str(&format!("  {name:<14} {what}\n"));
    }
    text
}

/// Asks which suites to run, at a terminal.
fn choose() -> Vec<&'static str> {
    println!("Which tests of the desktop app?\n");
    println!("  0  all of them");
    for (at, (name, what, _)) in SUITES.iter().enumerate() {
        println!("  {}  {name}: {what}", at + 1);
    }
    print!("\nNumbers or names, separated by spaces [0]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    let mut chosen = Vec::new();
    for word in line.split_whitespace() {
        match word.parse::<usize>() {
            Ok(0) => return SUITES.iter().map(|suite| suite.0).collect(),
            Ok(n) if n <= SUITES.len() => chosen.push(SUITES[n - 1].0),
            _ => {
                if let Some(suite) = SUITES.iter().find(|suite| suite.0 == word) {
                    chosen.push(suite.0);
                }
            }
        }
    }
    if chosen.is_empty() {
        SUITES.iter().map(|suite| suite.0).collect()
    } else {
        chosen
    }
}

fn asked(args: &[String]) -> Result<Asked, String> {
    let mut asked = Asked {
        suites: Vec::new(),
        filters: Vec::new(),
        approve: false,
        demo: false,
        out: target_dir().join("harness"),
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(usage()),
            "--approve" => asked.approve = true,
            "--out" => {
                asked.out = args.next().map(PathBuf::from).ok_or_else(usage)?;
            }
            "demo" => asked.demo = true,
            "all" => asked.suites = SUITES.iter().map(|suite| suite.0).collect(),
            "quick" => {
                asked.suites = SUITES
                    .iter()
                    .filter(|suite| suite.2)
                    .map(|suite| suite.0)
                    .collect();
            }
            word => match SUITES.iter().find(|suite| suite.0 == word) {
                Some(suite) => asked.suites.push(suite.0),
                None if word.starts_with('-') => return Err(usage()),
                None => asked.filters.push(word.to_lowercase()),
            },
        }
    }
    if asked.suites.is_empty() && !asked.demo {
        asked.suites = if asked.filters.is_empty() {
            choose()
        } else {
            // A name with no suite: the scenarios it names, driven.
            vec!["behavior"]
        };
    }
    Ok(asked)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let asked = match asked(&args) {
        Ok(asked) => asked,
        Err(usage) => {
            eprint!("{usage}");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("no runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(suites::run(asked)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("the harness could not run: {e:#}");
            ExitCode::FAILURE
        }
    }
}
