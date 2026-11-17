//! What is logged, and for a program that outlives its runs, where it is
//! kept: files in the data directory, one event a line, bounded in size and
//! age.
//!
//! **One system for every program**: the levels and the filter are the
//! command line's ([`filter`], `SNOB_LOG`). A command writes its lines to
//! standard error and is gone; an app has nobody reading that, so it writes
//! them to [`Files`] through [`ToFiles`].
//!
//! **No secret reaches a line.** A credential is a `Secret`, which prints as
//! `<hidden>`; [`scrub`] is the second net, over every string a line carries.

use std::borrow::Cow;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use snob_store::paths::{self, AppPaths};
use tracing::Level;
use tracing_subscriber::filter::Targets;

use crate::system::sync::lock;

/// What is logged, and at which level, and why `spec` (`SNOB_LOG`) was not
/// taken when it does not parse: the quiet default then, so a typo does not
/// also silence warnings.
pub fn filter(verbose: bool, spec: Option<&str>, owner: bool) -> (Targets, Option<String>) {
    if verbose {
        return (detailed(), None);
    }
    let quiet = || {
        let quiet = Targets::new().with_default(Level::WARN);
        if owner {
            quiet.with_target("snob_engine::browser::owner::server", Level::INFO)
        } else {
            quiet
        }
    };
    match spec {
        Some(spec) => match with_moved_targets(spec).parse::<Targets>() {
            Ok(chosen) => (chosen, None),
            Err(e) => (quiet(), Some(e.to_string())),
        },
        None => (quiet(), None),
    }
}

/// Every crate of snob's at debug, and nothing of anybody else's.
fn detailed() -> Targets {
    OURS.iter().fold(Targets::new(), |targets, name| {
        targets.with_target(*name, Level::DEBUG)
    })
}

/// The crates whose lines are snob's own.
const OURS: [&str; 7] = [
    "snob",
    "snob_ig",
    "snob_core",
    "snob_store",
    "snob_cli",
    "snob_engine",
    "snob_desktop",
];

/// Where a module of the engine was, and where its lines are since the
/// engine's were grouped: the browser's under `browser`, the operating
/// system's under `system`, the monitor's under `watch`, and a command that
/// moved into the engine under its own name. The longer name first where one
/// begins another; `::engine` last, which held the monitor.
///
/// **More, never less**: the monitor's own module holds what was
/// `engine::watch`, and a target selects its children, so a spec for the old
/// name selects the webhook's lines too.
const MOVED_IN_THE_ENGINE: [(&str, &[&str]); 17] = [
    ("::cdp", &["::browser::cdp"]),
    ("::headless", &["::browser::headless"]),
    ("::owner", &["::browser::owner"]),
    ("::detach", &["::system::detach"]),
    ("::exe", &["::system::exe"]),
    ("::interrupt", &["::system::interrupt"]),
    ("::local_socket", &["::system::local_socket"]),
    ("::parent", &["::system::parent"]),
    ("::pipe", &["::system::pipe"]),
    ("::power", &["::system::power"]),
    ("::random", &["::system::random"]),
    ("::seat", &["::system::seat"]),
    ("::engine::watch_posts", &["::watch::posts"]),
    ("::engine::watch", &["::watch", "::engine::retention"]),
    ("::engine::check", &["::watch::check"]),
    ("::engine", &["::watch"]),
    // `snob_cli`'s, which the engine performs now.
    ("::commands::follow", &["::follow"]),
];

/// A `SNOB_LOG` spec with, ahead of every pair that names a module by where
/// it was, the same level for where that code lives now, so a spec a person
/// saved still selects what it did:
///
/// - most of what was `snob_cli::<module>` is `snob_engine::<module>`, and
///   `snob_cli::commands::mcp` is `snob_mcp`;
/// - a module of the engine's that was grouped since is under its group
///   ([`MOVED_IN_THE_ENGINE`]), whichever of the two crates the spec names it
///   by.
///
/// **Ahead, so a pair the person wrote for the new name wins**: of two pairs
/// for one target the later is kept. A module that stayed gets a pair that
/// matches nothing, which costs nothing.
fn with_moved_targets(spec: &str) -> String {
    /// `prefix` taken off `path` where it ends a path segment.
    fn under<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
        path.strip_prefix(prefix)
            .filter(|rest| rest.is_empty() || rest.starts_with("::") || rest.starts_with('['))
    }
    /// Where the engine's `module` is now, when it was moved.
    fn grouped(module: &str) -> Vec<String> {
        MOVED_IN_THE_ENGINE
            .iter()
            .find_map(|(was, now)| {
                under(module, was).map(|rest| now.iter().map(|is| format!("{is}{rest}")).collect())
            })
            .unwrap_or_default()
    }

    let mut pairs = Vec::new();
    for pair in spec.split(',') {
        let pair = pair.trim();
        let (target, level) = pair.split_at(pair.find('=').unwrap_or(pair.len()));
        if let Some(module) = under(target, "snob_engine") {
            pairs.extend(
                grouped(module)
                    .into_iter()
                    .map(|now| format!("snob_engine{now}{level}")),
            );
            continue;
        }
        let Some(module) = under(target, "snob_cli") else {
            continue;
        };
        match under(module, "::commands::mcp") {
            Some(rest) => pairs.push(format!("snob_mcp{rest}{level}")),
            None => {
                pairs.push(format!("snob_engine{module}{level}"));
                pairs.extend(
                    grouped(module)
                        .into_iter()
                        .map(|now| format!("snob_engine{now}{level}")),
                );
                if module.is_empty() || module.starts_with('[') {
                    pairs.push(format!("snob_mcp{module}{level}"));
                }
            }
        }
    }
    pairs.push(spec.to_string());
    pairs.join(",")
}

/// The most one log file holds before the next is started.
pub const FILE_BYTES: u64 = 1024 * 1024;

/// How many log files are kept, the one being written among them.
pub const KEPT_FILES: usize = 5;

/// How long a log file or a crash report is kept.
pub const KEPT_FOR: Duration = Duration::from_secs(14 * 24 * 60 * 60);

/// How long the detailed level stays on once asked for: a switch left on by
/// mistake turns itself off.
pub const DETAILED_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// Where an app's log is kept: `logs/` in the data directory.
pub fn dir(paths: &AppPaths) -> PathBuf {
    paths.data_dir().join("logs")
}

/// The log files of one directory: `snob.log` being written, and
/// `snob.1.log` (the newest before it) to `snob.4.log`.
pub struct Files {
    dir: PathBuf,
    open: Mutex<Option<(std::fs::File, u64)>>,
}

impl Files {
    /// The log in `dir`, made private when it is not there, with what is past
    /// its keeping removed.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        paths::create_private_dir(dir).map_err(std::io::Error::other)?;
        sweep(dir, SystemTime::now());
        Ok(Self {
            dir: dir.to_path_buf(),
            open: Mutex::new(None),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, n: usize) -> PathBuf {
        match n {
            0 => self.dir.join("snob.log"),
            n => self.dir.join(format!("snob.{n}.log")),
        }
    }

    /// Appends `line`, whole, starting the next file first when this one is
    /// full. **A line that cannot be written is dropped**: a full disk must
    /// not stop the program that says so.
    pub fn write(&self, line: &str) {
        let mut open = lock(&self.open);
        let needed = line.len() as u64 + 1;
        if open
            .as_ref()
            .is_some_and(|(_, written)| written + needed > FILE_BYTES)
        {
            *open = None;
            self.rotate();
        }
        if open.is_none() {
            let Ok(file) = private_append(&self.file(0)) else {
                return;
            };
            let written = file.metadata().map(|m| m.len()).unwrap_or(0);
            if written + needed > FILE_BYTES && written > 0 {
                drop(file);
                self.rotate();
                let Ok(file) = private_append(&self.file(0)) else {
                    return;
                };
                *open = Some((file, 0));
            } else {
                *open = Some((file, written));
            }
        }
        if let Some((file, written)) = open.as_mut() {
            // One write, so two lines never interleave.
            let mut whole = String::with_capacity(line.len() + 1);
            whole.push_str(line);
            whole.push('\n');
            if file.write_all(whole.as_bytes()).is_ok() {
                *written += needed;
            }
        }
    }

    /// Moves every file one place older; the oldest goes.
    fn rotate(&self) {
        let _ = std::fs::remove_file(self.file(KEPT_FILES - 1));
        for n in (0..KEPT_FILES - 1).rev() {
            let _ = std::fs::rename(self.file(n), self.file(n + 1));
        }
        sweep(&self.dir, SystemTime::now());
    }

    /// The newest `most` lines, oldest first, for a report.
    pub fn recent(&self, most: usize) -> Vec<String> {
        let mut lines = Vec::new();
        for n in 0..KEPT_FILES {
            let Ok(text) = std::fs::read_to_string(self.file(n)) else {
                continue;
            };
            let mut older: Vec<String> = text.lines().map(str::to_string).collect();
            older.append(&mut lines);
            lines = older;
            if lines.len() >= most {
                break;
            }
        }
        let from = lines.len().saturating_sub(most);
        lines.split_off(from)
    }
}

/// A file only this user reads, appended to.
fn private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Removes the logs and crash reports of `dir` last written more than
/// [`KEPT_FOR`] before `now`.
fn sweep(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ours = (name.starts_with("snob.") && name.ends_with(".log"))
            || (name.starts_with(CRASH_PREFIX) && name.ends_with(".txt"));
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|written| now.duration_since(written).ok())
            .is_some_and(|age| age > KEPT_FOR);
        if ours && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A running log: the layer that writes it, and what turns its detail on.
pub struct Log {
    pub layer: ToFiles,
    pub detail: Detail,
}

/// Starts the log of an app in `paths`: info for snob's own crates and
/// warnings for the rest, or what `spec` (`SNOB_LOG`) says, and the detailed
/// level while it is on.
pub fn start(paths: &AppPaths, spec: Option<&str>) -> std::io::Result<Log> {
    let files = Arc::new(Files::open(&dir(paths))?);
    let usual = OURS
        .iter()
        .fold(Targets::new().with_default(Level::WARN), |targets, name| {
            targets.with_target(*name, Level::INFO)
        });
    let usual = spec
        .and_then(|spec| with_moved_targets(spec).parse::<Targets>().ok())
        .unwrap_or(usual);
    let detail = Detail {
        until: Arc::new(AtomicI64::new(0)),
        file: files.dir().join("detailed-until"),
    };
    detail.until.store(detail.kept(), Ordering::Relaxed);
    Ok(Log {
        layer: ToFiles {
            files,
            usual,
            detailed: detailed(),
            detailed_until: Arc::clone(&detail.until),
        },
        detail,
    })
}

/// The switch for the detailed level: on for [`DETAILED_FOR`] and then off
/// by itself, across restarts.
#[derive(Clone)]
pub struct Detail {
    /// Epoch seconds; 0 when off.
    until: Arc<AtomicI64>,
    file: PathBuf,
}

impl Detail {
    fn kept(&self) -> i64 {
        std::fs::read_to_string(&self.file)
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Until when the detailed level is on, when it is.
    pub fn on_until(&self) -> Option<snob_core::Epoch> {
        let until = self.until.load(Ordering::Relaxed);
        (until > snob_core::clock::now().get()).then(|| snob_core::Epoch::new(until))
    }

    /// Turns the detailed level on, for [`DETAILED_FOR`] from now, or off.
    pub fn set(&self, on: bool) {
        let until = if on {
            snob_core::clock::now().get() + DETAILED_FOR.as_secs() as i64
        } else {
            0
        };
        self.until.store(until, Ordering::Relaxed);
        if on {
            let _ = std::fs::write(&self.file, until.to_string());
        } else {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

/// Writes every event the filter lets through to [`Files`], as one JSON
/// object a line: `ts` (UTC), `level`, `target`, `msg`, then the event's own
/// fields. Every string is [`scrub`]bed.
pub struct ToFiles {
    files: Arc<Files>,
    usual: Targets,
    detailed: Targets,
    detailed_until: Arc<AtomicI64>,
}

impl ToFiles {
    /// The files written to, for a report.
    pub fn files(&self) -> Arc<Files> {
        Arc::clone(&self.files)
    }

    fn lets_through(&self, target: &str, level: &Level) -> bool {
        self.usual.would_enable(target, level)
            || (self.detailed_until.load(Ordering::Relaxed) > snob_core::clock::now().get()
                && self.detailed.would_enable(target, level))
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for ToFiles {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let metadata = event.metadata();
        if !self.lets_through(metadata.target(), metadata.level()) {
            return;
        }
        let mut line = serde_json::Map::new();
        let now = chrono::DateTime::from_timestamp_millis(snob_core::clock::now_ms().get())
            .unwrap_or_default();
        line.insert(
            "ts".into(),
            now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                .into(),
        );
        line.insert("level".into(), metadata.level().as_str().into());
        line.insert("target".into(), metadata.target().into());
        event.record(&mut Fields(&mut line));
        self.files
            .write(&serde_json::Value::Object(line).to_string());
    }
}

/// An event's fields, into a line.
struct Fields<'a>(&'a mut serde_json::Map<String, serde_json::Value>);

impl Fields<'_> {
    fn put(&mut self, field: &tracing::field::Field, value: serde_json::Value) {
        let name = match field.name() {
            "message" => "msg",
            // The line's own names are not an event's to replace.
            name @ ("ts" | "level" | "target" | "msg") => {
                self.0.insert(format!("field.{name}"), value);
                return;
            }
            name => name,
        };
        self.0.insert(name.to_string(), value);
    }
}

impl tracing::field::Visit for Fields<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.put(field, scrub(value).into_owned().into());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.put(field, scrub(&format!("{value:?}")).into_owned().into());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.put(field, value.into());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.put(field, value.into());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.put(field, value.into());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.put(field, value.into());
    }
}

/// What stands in a line where a secret was.
const HIDDEN: &str = "<hidden>";

/// The names whose value is a credential, wherever a line gives one a value
/// (`name=value`, `name: value`, `"name":"value"`).
const SECRET_NAMES: [&str; 16] = [
    "sessionid",
    "csrftoken",
    "x-csrftoken",
    "fb_dtsg",
    "lsd",
    "x-fb-lsd",
    "datr",
    "ig_did",
    "mid",
    "rur",
    "password",
    "token",
    "secret",
    "signature",
    "snob_mcp_key",
    "snob_mcp_token",
];

/// The headers whose whole value is one: to the end of the line or the quote.
const SECRET_HEADERS: [&str; 3] = ["cookie", "set-cookie", "authorization"];

/// `text` with every credential it may carry replaced by `<hidden>`: the
/// value of a name that holds one, a header that carries one, anything shaped
/// like a `sessionid`, and an MCP key.
pub fn scrub(text: &str) -> Cow<'_, str> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    // The ranges to hide, found on the lowercased copy: ASCII lowercasing
    // moves no byte.
    let mut hide: Vec<(usize, usize)> = Vec::new();

    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut named = |name: &str, to_the_end: bool| {
        let mut from = 0;
        while let Some(found) = lower[from..].find(name) {
            let start = from + found;
            let after = start + name.len();
            from = after;
            if start > 0 && word(bytes[start - 1]) {
                continue;
            }
            // `"name"`, then `=` or `:`, then the value, quoted or bare.
            let mut at = after;
            if bytes.get(at) == Some(&b'"') || bytes.get(at) == Some(&b'\\') {
                at += 1;
                if bytes.get(at) == Some(&b'"') {
                    at += 1;
                }
            }
            while bytes.get(at) == Some(&b' ') {
                at += 1;
            }
            if !matches!(bytes.get(at), Some(b'=' | b':')) {
                continue;
            }
            at += 1;
            while matches!(bytes.get(at), Some(b' ' | b'"' | b'\'' | b'\\')) {
                at += 1;
            }
            let ends = |b: u8| {
                if to_the_end {
                    matches!(b, b'"' | b'\n' | b'\r')
                } else {
                    matches!(
                        b,
                        b';' | b'&'
                            | b','
                            | b'"'
                            | b'\''
                            | b' '
                            | b'\n'
                            | b'\r'
                            | b')'
                            | b'}'
                            | b']'
                            | b'\\'
                    )
                }
            };
            let end = bytes[at..]
                .iter()
                .position(|b| ends(*b))
                .map_or(bytes.len(), |n| at + n);
            if end > at {
                hide.push((at, end));
            }
        }
    };
    for name in SECRET_NAMES {
        named(name, false);
    }
    for name in SECRET_HEADERS {
        named(name, true);
    }

    // A `sessionid` by its shape, whatever names it: the account's id, a
    // separator, and a long token.
    let token = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'%' | b':' | b'-' | b'_');
    let mut at = 0;
    while at < bytes.len() {
        if !bytes[at].is_ascii_digit() || (at > 0 && token(bytes[at - 1])) {
            at += 1;
            continue;
        }
        let digits = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let rest = &lower[at + digits..];
        let separator = if rest.starts_with("%3a") {
            3
        } else if rest.starts_with(':') {
            1
        } else {
            0
        };
        let long = rest.as_bytes()[separator.min(rest.len())..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        let whole = bytes[at..].iter().take_while(|b| token(**b)).count();
        if separator > 0 && long >= 8 && rest[separator + long..].starts_with(['%', ':']) {
            hide.push((at, at + whole));
        }
        at += whole.max(1);
    }

    // An MCP key: its prefix stays, to say what was there.
    const KEY: &str = "snob_mcp_";
    let mut from = 0;
    while let Some(found) = lower[from..].find(KEY) {
        let start = from + found + KEY.len();
        let long = bytes[start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            .count();
        if long >= 20 {
            hide.push((start, start + long));
        }
        from = start + long;
    }

    if hide.is_empty() {
        return Cow::Borrowed(text);
    }
    hide.sort_unstable();
    let mut clean = String::with_capacity(text.len());
    let mut done = 0;
    for (start, end) in hide {
        if end <= done {
            continue;
        }
        let start = start.max(done);
        // The copy was lowercased byte for byte, so these are boundaries of
        // `text` wherever they are boundaries at all.
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        clean.push_str(&text[done..start]);
        clean.push_str(HIDDEN);
        done = end;
    }
    clean.push_str(&text[done..]);
    Cow::Owned(clean)
}

/// Logs `error` under a short code and returns the code: what a view shows
/// under "details", so the line is found at once.
pub fn failed(error: &anyhow::Error) -> String {
    let id = error_id();
    tracing::error!(id = %id, error = %format!("{error:#}"), "failed");
    id
}

/// A short code to name one failure by: `7K2Q-9FMX`.
fn error_id() -> String {
    // Without the letters read as digits.
    const LETTERS: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bytes = [0u8; 8];
    if crate::system::random::fill_random(&mut bytes).is_err() {
        bytes = snob_core::clock::now_ms().get().to_le_bytes();
    }
    let mut id = String::with_capacity(9);
    for (n, byte) in bytes.iter().enumerate() {
        if n == 4 {
            id.push('-');
        }
        id.push(char::from(LETTERS[usize::from(*byte) % LETTERS.len()]));
    }
    id
}

/// What a crash report's file is named after.
const CRASH_PREFIX: &str = "crash-";

/// What marks a crash report as shown already.
const SEEN: &str = ".seen";

/// The most of a crash report that is read back.
const MAX_CRASH_BYTES: u64 = 256 * 1024;

/// From now on a panic leaves a report in `dir` before the process goes:
/// `about` (the program, its version, the system), where it panicked, what
/// it said and the stack. **Kept on this computer, and sent nowhere.**
pub fn keep_crashes(dir: &Path, about: String) {
    let dir = dir.to_path_buf();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let said = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with no message".to_string());
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let report = format!(
            "{about}\npanicked at {at}\n{}\n\n{}\n",
            scrub(&said),
            std::backtrace::Backtrace::force_capture()
        );
        let file = dir.join(format!(
            "{CRASH_PREFIX}{}.txt",
            snob_core::clock::now_ms().get()
        ));
        if let Ok(mut file) = private_append(&file) {
            let _ = file.write_all(report.as_bytes());
            let _ = file.sync_all();
        }
        previous(info);
    }));
}

/// A crash report nobody has been shown yet.
#[derive(Debug)]
pub struct Crash {
    file: PathBuf,
    pub report: String,
}

impl Crash {
    /// The newest report in `dir` not shown yet.
    pub fn waiting(dir: &Path) -> Option<Self> {
        let newest = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(CRASH_PREFIX)
                            && name.ends_with(".txt")
                            && !name.contains(SEEN)
                    })
            })
            .max()?;
        let mut report = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(&newest).ok()?, MAX_CRASH_BYTES),
            &mut report,
        )
        .ok()?;
        Some(Self {
            file: newest,
            report,
        })
    }

    /// The newest report in `dir`, shown already or not, for a diagnostic
    /// report.
    pub fn newest(dir: &Path) -> Option<String> {
        let newest = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(CRASH_PREFIX) && name.ends_with(".txt"))
            })
            .max()?;
        let mut report = String::new();
        std::io::Read::read_to_string(
            &mut std::io::Read::take(std::fs::File::open(&newest).ok()?, MAX_CRASH_BYTES),
            &mut report,
        )
        .ok()?;
        Some(report)
    }

    /// Marks every report in the directory as shown: they stay, for a
    /// diagnostic report, until they are past their keeping.
    pub fn shown(self) {
        let Some(dir) = self.file.parent() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with(CRASH_PREFIX) && name.ends_with(".txt") && !name.contains(SEEN) {
                let seen = name.replacen(".txt", &format!("{SEEN}.txt"), 1);
                let _ = std::fs::rename(&path, path.with_file_name(seen));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt as _;

    /// The two settings a person reaches for, and the one they mistype.
    #[test]
    fn the_filter_reads_what_was_asked_for() {
        let (verbose, _) = filter(true, None, false);
        assert!(verbose.would_enable("snob_ig::client", &Level::DEBUG));
        assert!(!verbose.would_enable("hyper_util::client", &Level::DEBUG));
        assert!(!verbose.would_enable("snob_ig::client", &Level::TRACE));

        let (quiet, _) = filter(false, None, false);
        assert!(quiet.would_enable("snob_ig::client", &Level::WARN));
        assert!(!quiet.would_enable("snob_ig::client", &Level::INFO));
        assert!(!quiet.would_enable("snob_engine::browser::owner::server", &Level::INFO));

        // The owner says when it starts, and otherwise no more than a command.
        let (owner, _) = filter(false, None, true);
        assert!(owner.would_enable("snob_engine::browser::owner::server", &Level::INFO));
        assert!(!owner.would_enable("snob_engine::browser::owner::server", &Level::DEBUG));
        assert!(!owner.would_enable("snob_engine::browser::headless", &Level::INFO));

        let (chosen, ignored) = filter(false, Some("snob_store=debug,warn"), false);
        assert!(ignored.is_none());
        assert!(chosen.would_enable("snob_store::secrets", &Level::DEBUG));
        assert!(!chosen.would_enable("snob_ig::client", &Level::DEBUG));
        assert!(chosen.would_enable("snob_ig::client", &Level::WARN));

        // A spec that does not parse falls back to the quiet default rather
        // than to nothing at all, so a typo does not also silence warnings.
        let (typo, ignored) = filter(false, Some("snob_ig=loud"), false);
        assert!(ignored.is_some());
        assert!(typo.would_enable("snob_ig::client", &Level::WARN));
        assert!(!typo.would_enable("snob_ig::client", &Level::INFO));
    }

    /// A spec saved when every module was `snob_cli`'s still selects them
    /// where they are now, in the engine and in the server.
    #[test]
    fn a_spec_that_names_a_module_by_its_old_crate_still_selects_it() {
        let chosen = |spec: &str| filter(false, Some(spec), false).0;
        let old = chosen("snob_cli::headless=debug,snob_cli::commands::mcp=trace,warn");
        assert!(old.would_enable("snob_engine::browser::headless::guard", &Level::DEBUG));
        assert!(!old.would_enable("snob_engine::browser::headless::guard", &Level::TRACE));
        assert!(!old.would_enable("snob_engine::browser::owner::server", &Level::DEBUG));
        assert!(old.would_enable("snob_mcp::child", &Level::TRACE));
        assert!(old.would_enable("snob_mcp", &Level::TRACE));
        assert!(!old.would_enable("snob_engine::browser::cdp", &Level::INFO));
        assert!(old.would_enable("snob_ig::client", &Level::WARN));

        // The whole crate, as it was: the commands, the engine and the server.
        let whole = chosen("snob_cli=debug");
        for target in [
            "snob_cli::commands::lists",
            "snob_engine::engine::list",
            "snob_engine::jobs::supervise",
            "snob_mcp::http",
        ] {
            assert!(whole.would_enable(target, &Level::DEBUG), "{target}");
        }
        assert!(!whole.would_enable("snob_ig::client", &Level::DEBUG));

        // A module that stayed is still selected under its own name, and a
        // name that only begins like the crate's is no module of it.
        let stayed = chosen("snob_cli::jobs=debug,snob_client=debug");
        assert!(stayed.would_enable("snob_cli::jobs::start", &Level::DEBUG));
        assert!(stayed.would_enable("snob_engine::jobs::supervise", &Level::DEBUG));
        assert!(!stayed.would_enable("snob_engine::browser::owner", &Level::DEBUG));

        // What the person wrote for the new name is not overridden by what
        // the old one implies, in either order.
        for spec in [
            "snob_cli=trace,snob_engine=warn",
            "snob_engine=warn,snob_cli=trace",
        ] {
            let both = chosen(spec);
            assert!(both.would_enable("snob_cli::commands::lists", &Level::TRACE));
            assert!(
                !both.would_enable("snob_engine::browser::owner", &Level::INFO),
                "{spec}"
            );
            assert!(both.would_enable("snob_mcp::http", &Level::TRACE), "{spec}");
        }

        // A spec that does not parse is still said and still quiet.
        assert_eq!(
            with_moved_targets("snob_cli::owner=loud"),
            "snob_engine::owner=loud,snob_engine::browser::owner=loud,snob_cli::owner=loud"
        );
    }

    /// A spec saved before the engine's modules were grouped still selects
    /// them where they are now, and one written for where they are wins.
    #[test]
    fn a_spec_that_names_a_module_by_where_it_was_in_the_engine_still_selects_it() {
        let chosen = |spec: &str| filter(false, Some(spec), false).0;
        let old = chosen(
            "snob_engine::owner=debug,snob_engine::interrupt=info,\
             snob_engine::engine::watch_posts=trace,snob_engine::engine::check=debug,warn",
        );
        assert!(old.would_enable("snob_engine::browser::owner::server", &Level::DEBUG));
        assert!(!old.would_enable("snob_engine::browser::headless", &Level::INFO));
        assert!(old.would_enable("snob_engine::system::interrupt", &Level::INFO));
        assert!(!old.would_enable("snob_engine::system::interrupt", &Level::DEBUG));
        assert!(old.would_enable("snob_engine::watch::posts", &Level::TRACE));
        assert!(old.would_enable("snob_engine::watch::check", &Level::DEBUG));
        assert!(!old.would_enable("snob_engine::watch::webhook", &Level::INFO));

        // The monitor's run went to its own module and its retention stayed
        // with the lists; a follow went from the command to the engine.
        let split =
            chosen("snob_engine::engine::watch=debug,snob_cli::commands::follow=debug,warn");
        assert!(split.would_enable("snob_engine::watch", &Level::DEBUG));
        assert!(split.would_enable("snob_engine::engine::retention", &Level::DEBUG));
        assert!(!split.would_enable("snob_engine::engine::walk", &Level::DEBUG));
        assert!(split.would_enable("snob_engine::follow", &Level::DEBUG));

        // The lists' module held the monitor, and a module that only begins
        // like one that moved did not move.
        let lists = chosen("snob_engine::engine=debug,snob_engine::pipeline=debug,warn");
        assert!(lists.would_enable("snob_engine::engine::walk", &Level::DEBUG));
        assert!(lists.would_enable("snob_engine::watch", &Level::DEBUG));
        assert!(!lists.would_enable("snob_engine::system::pipe", &Level::INFO));

        for spec in [
            "snob_engine::owner=trace,snob_engine::browser::owner=warn",
            "snob_engine::browser::owner=warn,snob_engine::owner=trace",
        ] {
            assert!(
                !chosen(spec).would_enable("snob_engine::browser::owner", &Level::INFO),
                "{spec}"
            );
        }
    }

    #[test]
    fn a_credential_never_reaches_a_line_whatever_names_it() {
        let session = "42%3AAbCdEfGhIjKl%3A20%3AAYxyz";
        for line in [
            format!("Cookie: sessionid={session}; csrftoken=abcDEF123; ds_user_id=42"),
            format!("sent with sessionid={session}&next=1"),
            format!(r#"{{"sessionid":"{session}","username":"me"}}"#),
            format!("the browser handed over {session} for account 42"),
            "X-CSRFToken: abcDEF123".to_string(),
            "authorization: Bearer abcDEF123.more".to_string(),
            "the key is snob_mcp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopq".to_string(),
        ] {
            let clean = scrub(&line);
            for secret in [
                session,
                "AbCdEfGhIjKl",
                "abcDEF123",
                "ABCDEFGHIJKLMNOPQRSTUV",
            ] {
                assert!(!clean.contains(secret), "{line} became {clean}");
            }
            assert!(clean.contains(HIDDEN), "{clean}");
        }
    }

    #[test]
    fn a_line_with_no_credential_is_left_as_it_is() {
        for line in [
            "walked 12 accounts of @someone in 3.5 s",
            "the cooldown lasts until 2026-11-15T10:20:30Z",
            "account 42 signed in; 2 accounts",
            "mid-walk the token bucket was empty",
            "a line that is not ASCII: 日本語, über",
        ] {
            assert!(matches!(scrub(line), Cow::Borrowed(_)), "{line}");
        }
    }

    fn logged(lines: impl FnOnce()) -> (tempfile::TempDir, Vec<serde_json::Value>) {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        let log = start(&paths, None).unwrap();
        let files = log.layer.files();
        tracing::subscriber::with_default(tracing_subscriber::registry().with(log.layer), lines);
        let read = files
            .recent(100)
            .iter()
            .map(|line| serde_json::from_str(line).expect("one JSON object a line"))
            .collect();
        (root, read)
    }

    #[test]
    fn an_event_is_one_json_line_with_its_fields_and_no_secret() {
        let (_root, lines) = logged(|| {
            tracing::info!(target: "snob_engine::login", screen = "welcome", took_ms = 12, "shown");
            tracing::warn!(
                target: "snob_ig::client",
                header = "sessionid=42%3AAbCdEfGhIjKl%3A20",
                "sent"
            );
            tracing::debug!(target: "snob_engine::login", "not at the usual level");
            tracing::info!(target: "hyper_util::client", "somebody else's");
        });

        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["level"], "INFO");
        assert_eq!(lines[0]["target"], "snob_engine::login");
        assert_eq!(lines[0]["msg"], "shown");
        assert_eq!(lines[0]["screen"], "welcome");
        assert_eq!(lines[0]["took_ms"], 12);
        assert!(lines[0]["ts"].as_str().unwrap().ends_with('Z'));
        assert_eq!(lines[1]["header"], format!("sessionid={HIDDEN}"));
    }

    #[test]
    fn the_detailed_level_is_on_for_a_day_across_restarts_and_off_when_told() {
        let root = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(root.path());
        let log = start(&paths, None).unwrap();
        assert_eq!(log.detail.on_until(), None);
        assert!(!log.layer.lets_through("snob_engine::login", &Level::DEBUG));

        log.detail.set(true);
        let until = log.detail.on_until().expect("on").get();
        let day = DETAILED_FOR.as_secs() as i64;
        assert!((day - 5..=day).contains(&(until - snob_core::clock::now().get())));
        assert!(log.layer.lets_through("snob_engine::login", &Level::DEBUG));
        assert!(!log.layer.lets_through("hyper_util::client", &Level::DEBUG));

        let again = start(&paths, None).unwrap();
        assert_eq!(again.detail.on_until().map(|e| e.get()), Some(until));

        again.detail.set(false);
        assert_eq!(start(&paths, None).unwrap().detail.on_until(), None);
    }

    #[test]
    fn the_log_starts_another_file_when_one_is_full_and_keeps_five() {
        let root = tempfile::tempdir().unwrap();
        let files = Files::open(root.path()).unwrap();
        // A little under a tenth of a file a line.
        let line = "x".repeat(100 * 1024);
        for n in 0..70 {
            files.write(&format!("{n} {line}"));
        }

        let mut kept: Vec<String> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        kept.sort();
        assert_eq!(
            kept,
            [
                "snob.1.log",
                "snob.2.log",
                "snob.3.log",
                "snob.4.log",
                "snob.log"
            ]
        );
        for name in &kept {
            let size = std::fs::metadata(root.path().join(name)).unwrap().len();
            assert!(size <= FILE_BYTES, "{name} holds {size}");
        }
        let recent = files.recent(3);
        let numbers: Vec<&str> = recent
            .iter()
            .map(|line| line.split(' ').next().unwrap())
            .collect();
        assert_eq!(
            numbers,
            ["67", "68", "69"],
            "the newest lines, oldest first"
        );
    }

    #[test]
    fn what_is_past_its_keeping_is_removed_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        for name in ["snob.3.log", "crash-1.txt", "snob.log", "notes.txt"] {
            std::fs::write(root.path().join(name), b"x").unwrap();
        }

        sweep(root.path(), SystemTime::now());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 4);

        sweep(
            root.path(),
            SystemTime::now() + KEPT_FOR + Duration::from_secs(60),
        );
        let left: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["notes.txt"], "only snob's own files are swept");
    }

    #[test]
    fn a_failure_is_logged_under_the_code_it_is_shown_with() {
        let mut id = String::new();
        let (_root, lines) = logged(|| {
            id = failed(&anyhow::anyhow!("the inner reason").context("the outer one"));
        });

        assert_eq!(id.len(), 9, "{id}");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["level"], "ERROR");
        assert_eq!(lines[0]["id"], id);
        assert_eq!(lines[0]["error"], "the outer one: the inner reason");
        assert_ne!(error_id(), error_id());
    }

    #[test]
    fn a_crash_report_waits_until_it_has_been_shown() {
        let root = tempfile::tempdir().unwrap();
        assert!(Crash::waiting(root.path()).is_none());
        std::fs::write(root.path().join("crash-100.txt"), "the older one").unwrap();
        std::fs::write(root.path().join("crash-200.txt"), "the newer one").unwrap();

        let crash = Crash::waiting(root.path()).expect("one is waiting");
        assert_eq!(crash.report, "the newer one");
        crash.shown();

        assert!(Crash::waiting(root.path()).is_none());
        assert_eq!(
            Crash::newest(root.path()).as_deref(),
            Some("the newer one"),
            "still there for a diagnostic report"
        );
        assert!(root.path().join("crash-200.seen.txt").is_file());
        assert!(root.path().join("crash-100.seen.txt").is_file());
    }
}
