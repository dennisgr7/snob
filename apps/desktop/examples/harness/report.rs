//! What a run found, said: a line per check on the terminal, and a page
//! with the pictures, the measures against their baseline, and for a check
//! that failed its frames and its log.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::drive::{Driven, Start};
use crate::scenarios::Lab;
use crate::suites::Outcome;
use crate::{Asked, app_dir};

/// One check, on the terminal.
pub fn line(outcome: &Outcome) {
    let mark = if outcome.passed { "ok  " } else { "FAIL" };
    println!(
        "{mark} {:<58} {:>6.1}s",
        outcome.name,
        outcome.took.as_secs_f32()
    );
    if !outcome.passed {
        for line in outcome.detail.lines() {
            println!("       {line}");
        }
    }
}

/// How far apart two channels may be and still be the same color: what one
/// machine's text rendering differs from another's by.
const SAME_WITHIN: i16 = 24;

/// The most of a picture that may differ, in parts of ten thousand.
const DIFFERENT_AT_MOST: usize = 20;

/// Compares the picture `taken` with the approved one of `name`, and marks
/// what differs on a copy beside it. With `approve`, takes it as approved.
pub fn compared(
    mut outcome: Outcome,
    taken: &Path,
    approved_dir: &Path,
    name: &str,
    approve: bool,
) -> Outcome {
    outcome
        .pictures
        .push(("now".to_string(), taken.to_path_buf()));
    let approved = approved_dir.join(format!("{name}.png"));
    if approve {
        let kept =
            std::fs::create_dir_all(approved_dir).and_then(|()| std::fs::copy(taken, &approved));
        outcome.passed = kept.is_ok();
        outcome.detail = match kept {
            Ok(_) => "approved".to_string(),
            Err(e) => format!("could not be approved: {e}"),
        };
        return outcome;
    }
    let (Ok(now), Ok(before)) = (image::open(taken), image::open(&approved)) else {
        outcome.detail =
            "new: no approved picture to compare with (--approve takes this one)".to_string();
        return outcome;
    };
    let (now, before) = (now.to_rgba8(), before.to_rgba8());
    outcome.pictures.push(("approved".to_string(), approved));
    if now.dimensions() != before.dimensions() {
        outcome.passed = false;
        outcome.detail = format!(
            "{:?} now, {:?} approved",
            now.dimensions(),
            before.dimensions()
        );
        return outcome;
    }
    let mut marked = now.clone();
    let mut different = 0usize;
    for ((x, y, new), old) in now.enumerate_pixels().zip(before.pixels()) {
        let same = new
            .0
            .iter()
            .zip(old.0)
            .all(|(new, old)| (i16::from(*new) - i16::from(old)).abs() <= SAME_WITHIN);
        if same {
            // Dimmed, so what differs stands out.
            let [r, g, b, _] = new.0;
            let dim = |c: u8| (u16::from(c) / 3 + 150) as u8;
            marked.put_pixel(x, y, image::Rgba([dim(r), dim(g), dim(b), 255]));
        } else {
            different += 1;
            marked.put_pixel(x, y, image::Rgba([255, 0, 200, 255]));
        }
    }
    let all = (now.width() * now.height()) as usize;
    if different * 10_000 > all * DIFFERENT_AT_MOST {
        let diff = taken.with_extension("diff.png");
        if marked.save(&diff).is_ok() {
            outcome.pictures.push(("what differs".to_string(), diff));
        }
        outcome.passed = false;
        outcome.detail = format!("{different} of {all} pixels differ from the approved picture");
    }
    outcome.ended()
}

/// What the app's process holds in memory now, in bytes.
pub fn memory_of(app: &Driven) -> Option<u64> {
    let mut system = System::new();
    let pid = Pid::from_u32(app.pid());
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_memory(),
    );
    system.process(pid).map(sysinfo::Process::memory)
}

/// How much of one processor the app used over `over`, in percent.
async fn cpu_of(app: &Driven, over: Duration) -> Option<f32> {
    let mut system = System::new();
    let pid = Pid::from_u32(app.pid());
    let what = ProcessRefreshKind::nothing().with_cpu();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, what);
    tokio::time::sleep(over).await;
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, what);
    system.process(pid).map(sysinfo::Process::cpu_usage)
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// The measures of a run, by name: milliseconds, percent, mebibytes.
type Measures = BTreeMap<String, f64>;

fn baseline_file() -> PathBuf {
    app_dir()
        .join("tests")
        .join("baseline")
        .join(format!("{}.json", std::env::consts::OS))
}

/// How much worse than its baseline a measure may be: a tenth, and never
/// less than what a busy machine adds by itself.
fn allowed(name: &str, baseline: f64) -> f64 {
    let slack = match name {
        name if name.ends_with("_ms") => 60.0,
        name if name.ends_with("_percent") => 1.5,
        _ => 8.0,
    };
    baseline * 1.10 + slack
}

async fn measure(lab: &Lab, asked: &Asked) -> anyhow::Result<Measures> {
    let start = Start {
        instagram: Some(lab.instagram.clone()),
        ..Start::default()
    };
    let (mut answers, mut usable, mut changes) = (Vec::new(), Vec::new(), Vec::new());
    let mut measures = Measures::new();
    for run in 0..5 {
        let root = asked.out.join("roots").join(format!("performance-{run}"));
        if root.exists() {
            std::fs::remove_dir_all(&root)?;
        }
        std::fs::create_dir_all(&root)?;
        let mut app = Driven::start(&lab.exe, &root, &start).await?;
        answers.push(app.answered_after.as_secs_f64() * 1000.0);
        app.until_screen("start").await?;
        usable.push(app.started.elapsed().as_secs_f64() * 1000.0);

        if run == 4 {
            // At rest, on the first screen: nothing moves, nobody asks, and
            // no field has a caret to blink.
            tokio::time::sleep(Duration::from_secs(2)).await;
            // Long enough that one tick of the system's clock, some 16 ms,
            // is a tenth of a percent and not a third.
            if let Some(cpu) = cpu_of(&app, Duration::from_secs(15)).await {
                measures.insert("idle_cpu_percent".to_string(), f64::from(cpu));
            }
            if let Some(memory) = memory_of(&app) {
                measures.insert(
                    "idle_memory_mib".to_string(),
                    memory as f64 / (1024.0 * 1024.0),
                );
            }
        }
        let since = Instant::now();
        app.click("start-paste").await?;
        app.until_screen("paste").await?;
        changes.push(since.elapsed().as_secs_f64() * 1000.0);

        app.quit().await?;
    }
    measures.insert("start_to_window_ms".to_string(), median(answers));
    measures.insert("start_to_usable_ms".to_string(), median(usable));
    measures.insert("screen_change_ms".to_string(), median(changes));
    if let Ok(binary) = std::fs::metadata(&lab.exe) {
        measures.insert(
            "test_binary_mib".to_string(),
            binary.len() as f64 / (1024.0 * 1024.0),
        );
    }
    Ok(measures)
}

/// Measures the app and holds each measure to its baseline.
pub async fn measured(lab: &Lab, asked: &Asked) -> Vec<Outcome> {
    let since = Instant::now();
    let measures = match measure(lab, asked).await {
        Ok(measures) => measures,
        Err(e) => {
            return vec![Outcome::of("performance", "measuring", since).failed(format!("{e:#}"))];
        }
    };
    let baseline: Measures = std::fs::read_to_string(baseline_file())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    // The history a report draws: one line a run.
    let history = asked.out.join("performance-history.jsonl");
    if let Ok(line) = serde_json::to_string(&measures) {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&history)
        {
            let _ = writeln!(file, "{line}");
        }
    }
    if asked.approve {
        let kept = baseline_file()
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .ok()
            .and_then(|_| serde_json::to_string_pretty(&measures).ok())
            .map(|text| std::fs::write(baseline_file(), text + "\n"));
        if !matches!(kept, Some(Ok(()))) {
            eprintln!("the baseline could not be written");
        }
    }

    measures
        .iter()
        .map(|(name, value)| {
            let (passed, detail) = match baseline.get(name) {
                _ if asked.approve => (true, format!("{value:.1}: taken as the baseline")),
                Some(before) => (
                    *value <= allowed(name, *before),
                    format!("{value:.1} now, {before:.1} the baseline"),
                ),
                None => (
                    true,
                    format!("{value:.1}: no baseline yet (--approve takes this one)"),
                ),
            };
            // A number read off one run of the app: it took no time of its
            // own.
            let mut outcome = Outcome::of("performance", name.clone(), Instant::now());
            outcome.passed = passed;
            outcome.detail = detail;
            outcome
        })
        .collect()
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// `path` as the page links to it: from the folder the page is in.
fn linked(out: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(out).unwrap_or(path);
    let said = relative.display().to_string().replace('\\', "/");
    if relative.is_absolute() {
        format!("file:///{}", said.trim_start_matches('/'))
    } else {
        said
    }
}

/// A bar for each measure beside its baseline.
fn bars(outcomes: &[&Outcome]) -> String {
    let mut drawn = String::new();
    for outcome in outcomes {
        let mut numbers = outcome
            .detail
            .split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter_map(|word| word.parse::<f64>().ok());
        let (Some(now), before) = (numbers.next(), numbers.next()) else {
            continue;
        };
        let most = before.unwrap_or(now).max(now).max(0.001);
        let bar = |value: f64, class: &str| {
            format!(
                "<rect class=\"{class}\" x=\"0\" width=\"{:.0}\" height=\"10\"/>",
                value / most * 300.0
            )
        };
        drawn.push_str(&format!(
            "<figure><figcaption>{}: {}</figcaption><svg viewBox=\"0 0 300 24\" width=\"300\" height=\"24\" role=\"img\" aria-label=\"{}\"><g>{}</g><g transform=\"translate(0 13)\">{}</g></svg></figure>",
            escaped(&outcome.name),
            escaped(&outcome.detail),
            escaped(&outcome.detail),
            bar(now, "now"),
            before.map(|before| bar(before, "before")).unwrap_or_default(),
        ));
    }
    drawn
}

const STYLE: &str = "
:root{color-scheme:light dark;--bg:#fff;--fg:#111;--dim:#52525b;--line:#e4e4e7;--bad:#b42318;--ok:#15803d;--pink:#f472b6}
@media(prefers-color-scheme:dark){:root{--bg:#000;--fg:#e6e8ec;--dim:#9aa1ab;--line:#23272f;--bad:#ff8a80;--ok:#86efac}}
body{background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif;margin:0 auto;max-width:1100px;padding:24px 16px}
h1,h2{font-family:ui-monospace,monospace}h1 i{color:var(--pink)}
table{border-collapse:collapse;width:100%}td,th{border-bottom:1px solid var(--line);padding:6px 8px;text-align:left;vertical-align:top}
.ok{color:var(--ok)}.bad{color:var(--bad);font-weight:600}.dim{color:var(--dim)}
pre{white-space:pre-wrap;margin:4px 0;font-size:13px}
img{max-width:320px;border:1px solid var(--line);border-radius:6px;display:block}
.shots{display:flex;flex-wrap:wrap;gap:10px}figure{margin:6px 0}figcaption{font-size:12px;color:var(--dim)}
rect.now{fill:var(--pink)}rect.before{fill:var(--dim)}
";

/// Writes the page of a run, and answers where it is.
pub fn write(out: &Path, outcomes: &[Outcome]) -> anyhow::Result<PathBuf> {
    let failed = outcomes.iter().filter(|outcome| !outcome.passed).count();
    let mut page = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>snob desktop: test report</title><style>{STYLE}</style><h1><i>snob.</i> desktop: test report</h1><p>{} checked on {} {}, <span class=\"{}\">{} failed</span>.</p>",
        outcomes.len(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        if failed == 0 { "ok" } else { "bad" },
        failed,
    );
    let mut suites: Vec<&'static str> = Vec::new();
    for outcome in outcomes {
        if !suites.contains(&outcome.suite) {
            suites.push(outcome.suite);
        }
    }
    for suite in suites {
        let of_suite: Vec<&Outcome> = outcomes.iter().filter(|o| o.suite == suite).collect();
        let bad = of_suite.iter().filter(|outcome| !outcome.passed).count();
        page.push_str(&format!(
            "<h2>{suite} <span class=\"dim\">{} checked, {bad} failed</span></h2>",
            of_suite.len()
        ));
        if suite == "performance" {
            page.push_str(&bars(&of_suite));
        }
        page.push_str(
            "<table><tr><th>check</th><th>result</th><th>time</th><th>what was found</th></tr>",
        );
        for outcome in of_suite {
            let (class, result) = if outcome.passed {
                ("ok", "passed")
            } else {
                ("bad", "FAILED")
            };
            page.push_str(&format!(
                "<tr><td>{}</td><td class=\"{class}\">{result}</td><td class=\"dim\">{:.1}s</td><td>",
                escaped(&outcome.name),
                outcome.took.as_secs_f32()
            ));
            if !outcome.detail.is_empty() {
                page.push_str(&format!("<pre>{}</pre>", escaped(&outcome.detail)));
            }
            if !outcome.pictures.is_empty() {
                page.push_str("<div class=\"shots\">");
                for (what, picture) in &outcome.pictures {
                    let link = linked(out, picture);
                    page.push_str(&format!(
                        "<figure><a href=\"{link}\"><img loading=\"lazy\" src=\"{link}\" alt=\"{}: {}\"></a><figcaption>{}</figcaption></figure>",
                        escaped(&outcome.name),
                        escaped(what),
                        escaped(what),
                    ));
                }
                page.push_str("</div>");
            }
            if let Some(log) = &outcome.log {
                page.push_str(&format!("<a href=\"{}\">its log</a>", linked(out, log)));
            }
            page.push_str("</td></tr>");
        }
        page.push_str("</table>");
    }
    page.push_str("</html>\n");
    let file = out.join("report.html");
    std::fs::write(&file, page)?;
    Ok(file)
}
