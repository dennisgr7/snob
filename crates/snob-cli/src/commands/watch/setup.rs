//! `snob watch setup`: the questions, and the file they are written to.

use anyhow::{Context, Result, bail};
use snob_core::model::printable;
use snob_core::secret::Secret;
use snob_core::{Epoch, Pk};
use snob_core::{duration, watch::schedule};
use snob_engine::engine::check::Verdict;
use snob_engine::exit::{ExitCode, ExitError};
use snob_store::config::{self, WatchConfig};
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::registry::Registry;
use snob_store::secrets::{Kind, SecretStore};

use crate::cli::WatchSetupArgs;
use crate::ui;

use super::preflight::{baseline_now, describe_check, preflight};
use super::schedule::{calendar_from, schedule_from};
use super::status::{describe_config, plural};

/// Walks somebody through configuring the monitor.
pub async fn setup(
    args: WatchSetupArgs,
    secrets: SecretStore,
    paths: &AppPaths,
    account: Option<&AccountPaths>,
) -> Result<ExitCode> {
    // Every question here needs an answer, and the failure mode of asking with
    // nobody there is a service configured by whatever the defaults happened to
    // be. Said once, up front.
    if !ui::can_show_a_menu() {
        return Err(ExitError::new(
            ExitCode::Interrupted,
            "\"snob watch setup\" asks questions and there is no terminal to ask at.\n\
             Write the file by hand, or run this where you can answer."
                .to_string(),
        )
        .into());
    }

    let existing = existing_to_replace(paths, args.dry_run)?;
    if let Some(existing) = &existing {
        ui::info(&format!(
            "There is already a configuration at {}.",
            config::path(paths).display()
        ));
        describe_config(existing)
            .iter()
            .for_each(|l| eprintln!("  {l}"));
        if !ui::confirm("Replace it?", false)? {
            return Err(
                ExitError::new(ExitCode::Interrupted, "nothing was changed".to_string()).into(),
            );
        }
    }

    // The configuration is built up rather than rendered piece by piece: each
    // question puts back the fields it validated, and the file is written from
    // the same type a run reads. `config::template` says why.
    let mut config = ask_schedule()?;
    let answers = ask_webhook()?;
    // The address goes down with it.
    let own = account.and_then(|a| {
        snob_engine::account::username(&Registry::load(paths).unwrap_or_default(), a.pk())
            .map(str::to_string)
    });
    let accounts = ask_accounts(answers.url.as_deref(), own.as_deref())?;

    // A dry run did not read the file to replace it; what it prints still
    // keeps the entries and the format a real run would.
    let replaced = existing.or_else(|| config::load(paths).ok().flatten());
    config.webhook = answers.url.as_ref().map(|url| config::WebhookConfig {
        url: url.clone(),
        headers: answers.headers.iter().cloned().collect(),
        heartbeat: answers.heartbeat,
        // The format the address was given, kept when the same address is
        // set up again; else the one its host is known to want.
        format: replaced
            .as_ref()
            .and_then(|e| e.webhook.as_ref())
            .filter(|w| &w.url == url)
            .map_or_else(|| format_for(url), |w| w.format),
    });
    config.accounts = entries_to_write(accounts, account.map(|a| a.pk()), replaced.as_ref());

    let registry = Registry::load(paths).unwrap_or_default();
    let text = config::template(&config, answers.signing_key.is_some(), |pk| {
        snob_engine::account::username(&registry, pk).map(str::to_string)
    });

    // Parsed before it is written, and before any secret is stored.
    config::parse(&text, &config::path(paths))
        .context("the configuration this produced could not be read back; this is a bug")?;

    if args.dry_run {
        crate::ui::say!("{text}");
        ui::info("Nothing was written.");
        return Ok(ExitCode::Ok);
    }

    // The file first, then the secrets: `config::write` can fail on a read-only
    // or full disk, and a credential belonging to no configured address is the
    // worse half to be left holding.
    let written = config::write(paths, &text)?;
    answers.store_secrets(&secrets)?;

    ui::info(&format!("Written to {}.", written.display()));

    // Everything above is a claim about a machine, a session and somebody
    // else's server.
    crate::ui::say!();
    let report = preflight(&Default::default(), &secrets, paths, account).await?;
    for line in describe_check(&report) {
        crate::ui::say!("{line}");
    }

    offer_the_baseline(&report, &secrets, paths, account).await?;

    ui::info("Start it with \"snob watch\", or print a timer for it with \"snob watch unit\".");
    Ok(ExitCode::Ok)
}

/// The configuration this run would replace, if it would replace one.
fn existing_to_replace(paths: &AppPaths, dry_run: bool) -> Result<Option<WatchConfig>> {
    if dry_run {
        return Ok(None);
    }
    Ok(config::load(paths)?)
}

/// The `[[account]]` entries to write: those just answered for, read as the
/// account in use, after every entry read as another account, kept as it was.
fn entries_to_write(
    answered: Vec<Answered>,
    in_use: Option<Pk>,
    replaced: Option<&WatchConfig>,
) -> Vec<config::AccountConfig> {
    let others = replaced
        .into_iter()
        .flat_map(|config| &config.accounts)
        .filter(|entry| {
            in_use.is_some() && entry.viewer.is_some_and(|viewer| Some(viewer) != in_use)
        })
        .cloned();
    let answered = answered.into_iter().map(|answer| config::AccountConfig {
        lists: answer.lists,
        posts: answer.posts,
        ..config::AccountConfig::lists_of(
            answer.target,
            in_use,
            answer
                .consent
                .map(|agreed_at| config::ConsentConfig { agreed_at }),
        )
    });
    others.chain(answered).collect()
}

/// One account as the wizard was told to watch it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answered {
    /// The username, or `self`.
    target: String,
    /// When somebody agreed to its lists being read; `None` for the own
    /// account and for one whose lists are not watched.
    consent: Option<Epoch>,
    lists: bool,
    posts: Option<config::PostsConfig>,
}

impl Answered {
    /// An account whose lists alone are watched.
    fn lists(target: &str, consent: Option<Epoch>) -> Self {
        Self {
            target: target.to_string(),
            consent,
            lists: true,
            posts: None,
        }
    }
}

/// What taking the baseline now would walk, over every account it covers.
struct BaselineCost {
    /// How many accounts the walk covers.
    accounts: usize,
    followers: u64,
    following: u64,
    /// Roughly how many pages they come to, accumulated per account rather
    /// than by dividing the totals: each account pages separately, so the
    /// rounding belongs to each of them. What the sittings count.
    pages: u64,
    /// The requests those pages take, with what goes out beside them
    /// (`engine::walk::requests_to_walk`).
    requests: u64,
    /// How many accounts the preflight could not read counters for.
    uncounted: usize,
    /// How many accounts' posts the first capture reads, a profile and a page
    /// each.
    posts: usize,
}

/// What laying an account's posts baseline sends, from the browser: its
/// profile as the app opens one (the profile query and three reads beside it,
/// `client/ask.rs`), the route lookup the first time its name is seen, and the
/// first page of its grid. Six, the most it is.
const POSTS_BASELINE_REQUESTS: u64 = 6;

impl BaselineCost {
    /// The sentence somebody agrees to, naming what it covers.
    fn sentence(&self) -> String {
        let posts = match self.posts {
            0 => String::new(),
            n => format!(", and reading the newest posts of {n} account{}", plural(n)),
        };
        let mut line = format!(
            "Taking it now means walking {} followers and {} following across {} account{}{posts}: \
             roughly {} requests, {}.",
            self.followers,
            self.following,
            self.accounts,
            plural(self.accounts),
            self.requests,
            self.how_long()
        );
        if self.uncounted > 0 {
            line.push_str(&format!(
                " {} of them could not be counted, so it is longer than that.",
                self.uncounted
            ));
        }
        line
    }

    /// How long the walk takes, in words: a few minutes within one sitting, the
    /// sitting's rests past it (`pace::PAGES_PER_SITTING`), and more than a day
    /// past the day's accounts (`budget::accounts_per_day`).
    fn how_long(&self) -> String {
        use snob_ig::pace::{ACCOUNTS_PER_PAGE, PAGES_PER_SITTING};

        let per_sitting = u64::from(PAGES_PER_SITTING);
        if self.pages <= per_sitting {
            return "a few minutes".to_string();
        }
        let rests = (self.pages - 1) / per_sitting;
        let rests = format!(
            "{rests} pause{} of five to fifteen minutes, one about every {} accounts",
            plural(rests as usize),
            per_sitting * u64::from(ACCOUNTS_PER_PAGE)
        );
        let ceiling = u64::from(snob_core::budget::accounts_per_day(
            None,
            snob_core::clock::EpochMs::new(0),
        ));
        if self.followers + self.following > ceiling {
            format!(
                "over more than a day, since at most {ceiling} accounts are read in a day, and {rests}"
            )
        } else {
            format!("with {rests}")
        }
    }
}

/// Adds up what the preflight learned about every account it checked, its
/// requests as they go out from the browser or, with `browser` false,
/// directly.
fn baseline_cost(report: &snob_engine::engine::check::CheckReport, browser: bool) -> BaselineCost {
    use snob_engine::engine::check::What;

    let mut cost = BaselineCost {
        accounts: 0,
        followers: 0,
        following: 0,
        pages: 0,
        requests: 0,
        uncounted: 0,
        posts: 0,
    };
    for checked in &report.checked {
        if let What::Posts {
            baseline_at: None, ..
        } = &checked.what
        {
            // Their profile and one page of their grid: five requests, six the
            // first time the name is looked up, said as the higher.
            cost.posts += 1;
            cost.requests += POSTS_BASELINE_REQUESTS;
            continue;
        }
        let What::Account {
            followers,
            following,
            lists: true,
            ..
        } = &checked.what
        else {
            continue;
        };
        cost.accounts += 1;
        match (followers, following) {
            (Some(a), Some(b)) => {
                cost.followers += a;
                cost.following += b;
                use snob_engine::engine::walk::{pages_to_walk, requests_to_walk};
                cost.pages += pages_to_walk(*a) + pages_to_walk(*b);
                cost.requests += requests_to_walk(*a, browser) + requests_to_walk(*b, browser);
            }
            _ => cost.uncounted += 1,
        }
    }
    cost
}

/// Offers to take the first capture, saying what it costs.
async fn offer_the_baseline(
    report: &snob_engine::engine::check::CheckReport,
    secrets: &SecretStore,
    paths: &AppPaths,
    account: Option<&AccountPaths>,
) -> Result<()> {
    // Nothing to offer if a run could not happen anyway, or if there is already
    // something to compare against.
    if report.verdict() == Verdict::Failed {
        return Ok(());
    }
    if !report.wants_a_baseline() || !ui::can_show_a_menu() {
        return Ok(());
    }

    crate::ui::say!();
    ui::info(&format!(
        "There is no capture to compare against yet, so the first scheduled run \
         will lay one down and report nothing.\n{}",
        // From the browser unless this run says otherwise, as the monitor's
        // own runs will send them.
        baseline_cost(report, snob_ig::client::page::a_client_would_use_a_page()).sentence()
    ));
    if !ui::confirm("Take the first capture now?", false)? {
        ui::info("Left for the first scheduled run, which will report nothing and say so.");
        return Ok(());
    }

    let configured = config::load(paths)?;
    baseline_now(paths, configured.as_ref(), secrets, account).await
}

/// Stores a secret, or clears whatever was there when this run has none.
fn store_secret(secrets: &SecretStore, kind: Kind, value: Option<Secret>) -> Result<()> {
    match value {
        Some(secret) => secrets.save_secret(kind, &secret).with_context(|| {
            "the secret could not be stored in the keyring. Pass it on the command line \
             instead -- \"--sign-with\" or \"--header\" -- where a systemd unit can supply it \
             from an environment file."
        })?,
        None => secrets.forget_secret(kind)?,
    }
    Ok(())
}

/// The schedule questions, as the configuration they settle.
///
/// **The fields, not a rendered line**, so the file `config::template` writes
/// can be compared against the configuration it stands for.
fn ask_schedule() -> Result<WatchConfig> {
    let choice = ui::menu::choose(
        "How often should it look?",
        &[
            "Every so often (6h, 2d, 2w)",
            "On certain days, at certain times",
            "A cron expression I already have",
        ],
    )?
    .ok_or_else(|| ExitError::new(ExitCode::Interrupted, "nothing was changed".to_string()))?;

    let mut config = WatchConfig::default();

    match choice {
        0 => {
            config.every = Some(asked_until_it_holds(
                "How often? (for example 6h)",
                interval_of,
            )?)
        }
        1 => loop {
            let days = ui::prompt_line("Which days? (mon,thu -- or blank for every day)")?;
            let times = ui::prompt_line("At what times? (09:00 or 09:00,21:00)")?;
            match calendar_of(&days, &times) {
                Ok((on, at)) => {
                    config.on = on;
                    config.at = at;
                    break;
                }
                Err(e) => ui::warn(&format!("{e:#}; once more")),
            }
        },
        _ => {
            config.cron = Some(asked_until_it_holds(
                "The expression? (for example 0 9 * * 1,4)",
                cron_of,
            )?);
        }
    }

    // The schedule a run would build out of what has just been answered, which
    // is what decides how much room a jitter has -- and it closes any gap
    // between what the wizard accepts and what a run accepts: this is the
    // function `snob watch` itself calls, over the configuration about to be
    // written rather than over one answer at a time.
    let schedule = schedule_from(&Default::default(), Some(&config))?;
    config.jitter = ask_jitter(&schedule)?;

    Ok(config)
}

/// How far a run may be pushed past its moment, asked against the schedule that
/// was just settled.
///
/// **It is validated the way every other answer here is.**
fn ask_jitter(schedule: &schedule::Schedule) -> Result<Option<std::time::Duration>> {
    let room = room_for(schedule);
    if room.is_zero() {
        ui::info(
            "This schedule has no room to be pushed later: the gap between two runs is \
             already the smallest one allowed, so every run happens on its moment.",
        );
        return Ok(None);
    }

    let question = format!(
        "How far may a run be pushed later, so it does not land on the same second every \
         time? (blank for {}, \"0\" for none, at most {})",
        duration::format(schedule.jitter()),
        duration::format(room)
    );
    asked_until_it_holds(&question, |typed| jitter_of(schedule, typed))
}

/// What a jitter answer means against `schedule`: blank is the schedule's own,
/// and anything else has to fit in its room.
fn jitter_of(schedule: &schedule::Schedule, typed: &str) -> Result<Option<std::time::Duration>> {
    if typed.trim().is_empty() {
        return Ok(None);
    }
    let wanted = duration::parse(typed.trim()).map_err(|e| anyhow::anyhow!(e))?;
    // Through the clamp a run applies, rather than a comparison written again
    // here.
    if schedule.clone().with_jitter(wanted).jitter() < wanted {
        bail!(
            "{} is more room than this schedule has. At most {} can be taken out of the gap \
             between two runs without costing the next one.",
            duration::format(wanted),
            duration::format(room_for(schedule))
        );
    }
    Ok(Some(wanted))
}

/// Asks `question` until `holds` takes the answer, saying each time what was
/// wrong with it, so one mistyped answer does not end the wizard with every
/// question before it answered and nothing written.
fn asked_until_it_holds<T>(question: &str, holds: impl Fn(&str) -> Result<T>) -> Result<T> {
    loop {
        let answer = ui::prompt_line(question)?;
        match holds(&answer) {
            Ok(value) => return Ok(value),
            Err(e) => ui::warn(&format!("{e:#}; once more")),
        }
    }
}

/// The most this schedule could absorb, asked of the schedule.
fn room_for(schedule: &schedule::Schedule) -> std::time::Duration {
    schedule
        .clone()
        .with_jitter(std::time::Duration::MAX)
        .jitter()
}

// The three below are the whole of what `ask_schedule` decides, split out from
// the prompting so a test can reach them.

/// The `every` field, or what is wrong with the interval.
fn interval_of(text: &str) -> Result<std::time::Duration> {
    let every = duration::parse(text).map_err(|e| anyhow::anyhow!(e))?;
    // Validated here rather than at the first run, so "5m" is refused while the
    // person who typed it is still reading.
    schedule::Schedule::every(every)?;
    Ok(every)
}

/// The `on` and `at` fields, or what is wrong with the calendar.
fn calendar_of(days: &str, times: &str) -> Result<(Vec<String>, Vec<String>)> {
    fn listed(text: &str) -> Vec<&str> {
        text.split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect()
    }

    let days = listed(days);
    let times = listed(times);

    // Through the very function a run parses the file with, so the wizard
    // cannot accept a calendar the scheduler would then refuse.
    calendar_from(&days, &times)?;

    Ok((
        days.iter().map(|d| (*d).to_string()).collect(),
        times.iter().map(|t| (*t).to_string()).collect(),
    ))
}

/// The `cron` field, or what is wrong with the expression.
fn cron_of(expression: &str) -> Result<String> {
    schedule::Schedule::cron(expression)?;
    Ok(expression.trim().to_string())
}

/// What the webhook questions settled.
///
/// **Named fields, because two of them are `Option<Secret>` and the two are not interchangeable.**
struct WebhookAnswers {
    /// Where reports go, exactly as typed.
    url: Option<String>,
    heartbeat: bool,
    /// The extra headers that go in the file.
    headers: Vec<(String, String)>,
    /// The shared secret the body is signed with. Goes to
    /// [`Kind::WatchSigningKey`] and is **never sent anywhere** — only a MAC
    /// computed from it is.
    signing_key: Option<Secret>,
    /// The whole `Authorization` header value. Goes to [`Kind::WatchToken`] and
    /// **is sent**, verbatim, to the user's server on every request.
    token: Option<Secret>,
}

impl WebhookAnswers {
    /// Puts the two secrets in the keyring, each under the `Kind` that says what
    /// it is for.
    fn store_secrets(self, secrets: &SecretStore) -> Result<()> {
        store_secret(secrets, Kind::WatchSigningKey, self.signing_key)?;
        store_secret(secrets, Kind::WatchToken, self.token)?;
        Ok(())
    }
}

fn ask_webhook() -> Result<WebhookAnswers> {
    if !ui::confirm("Send each report to a webhook?", true)? {
        ui::info(
            "Nothing will be sent. \"snob watch --json >> events.ndjson\" is a complete way to \
             use it without one.",
        );
        return Ok(WebhookAnswers {
            url: None,
            heartbeat: false,
            headers: vec![],
            signing_key: None,
            token: None,
        });
    }

    // Not echoed, as nowhere else an address that does not parse is
    // (`webhook::shown_str`): the person has just typed it.
    let (url, parsed) = asked_until_it_holds("Where? (https://n8n.local/webhook/snob)", |url| {
        let parsed = url::Url::parse(url.trim()).context("what was typed is not an address")?;
        Ok((url.to_string(), parsed))
    })?;

    // Headers whose values are not secret go in the file; the Authorization
    // value goes to the keyring and nothing about it is written here.
    let mut typed = Vec::new();
    if ui::confirm(
        "Does it need any other headers? (written into watch.toml as they are; a token goes \
         in the next question, which keeps it in the keyring)",
        false,
    )? {
        loop {
            let line = ui::prompt_line(
                "Name: value (blank when there are no more, a name typed twice keeps the last)",
            )?;
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            let Some((name, value)) = line.split_once(':') else {
                ui::warn(&format!(
                    "\"{}\" is not a header; write it as \"Name: value\"",
                    printable(line)
                ));
                continue;
            };
            // Everything typed here is written into `watch.toml` in the clear
            // -- that is the file's contract, "no secret is in it" -- and the
            // very next question exists to put an Authorization value in the
            // keyring instead.
            if name.trim().eq_ignore_ascii_case("authorization") {
                ui::warn(
                    "an Authorization header is a secret and is not written into watch.toml. \
                     Answer the next question instead: it stores the value in the system keyring.",
                );
                continue;
            }
            typed.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let headers = last_of_each(typed);

    // Checked while the person is still here, address and headers together.
    snob_engine::watch::webhook::check(&snob_engine::watch::webhook::Webhook {
        url: parsed,
        headers: headers.clone(),
        key: None,
    })?;

    let mut token = None;
    if ui::confirm("Does it need an Authorization header?", false)? {
        loop {
            let value = ui::prompt_secret("The header value (for example \"Bearer abc123\")")?;
            if value.trim().is_empty() {
                ui::warn("an Authorization value cannot be empty; once more");
                continue;
            }
            token = Some(Secret::new(value.to_string()));
            break;
        }
    }

    let signing_key = if ui::confirm(
        "Sign the body, so the receiver can check it came from here?",
        true,
    )? {
        loop {
            let value = ui::prompt_secret("A shared secret (at least 32 characters, random)")?;
            // The same validator `--sign-with` runs, literally: one floor, one
            // trim, one sentence, so a key the flag accepts is a key the wizard
            // accepts and the other way round.
            match crate::cli::signing_secret(&value) {
                Ok(value) => break Some(Secret::new(value)),
                Err(why) => ui::warn(&format!("{why}; once more")),
            }
        }
    } else {
        None
    };

    let heartbeat = ui::confirm(
        "Send a report even when nothing changed, so silence means it stopped?",
        false,
    )?;

    // By name, so the signing key and the token cannot trade places by the
    // order of two lines.
    Ok(WebhookAnswers {
        url: Some(url.trim().to_string()),
        heartbeat,
        headers,
        signing_key,
        token,
    })
}

/// The headers a run of typed answers describes, a name given twice keeping the
/// last value.
fn last_of_each(typed: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut headers = std::collections::BTreeMap::new();
    for (name, value) in typed {
        headers.insert(name, value);
    }
    headers.into_iter().collect()
}

/// Why `name` is not taken as another account to watch, if it is not: not a
/// username at all (the rule clap holds the command line to,
/// `possible_username`), the own account under either of its names (it is
/// already the first entry, and a second would be walked twice), or one
/// already answered for.
fn refused_name(name: &str, own: Option<&str>, answered: &[Answered]) -> Option<String> {
    if !snob_core::model::possible_username(name) {
        return Some(format!(
            "\"{}\" cannot be a username: letters, digits, dots and underscores only",
            printable(name)
        ));
    }
    if name.eq_ignore_ascii_case("self") || own.is_some_and(|own| own.eq_ignore_ascii_case(name)) {
        return Some("that is your own account, which is watched already".to_string());
    }
    if answered
        .iter()
        .any(|entry| entry.target.eq_ignore_ascii_case(name))
    {
        return Some(format!("@{} is in the list already", printable(name)));
    }
    None
}

fn ask_accounts(webhook: Option<&str>, own_name: Option<&str>) -> Result<Vec<Answered>> {
    let mut own = Answered::lists("self", None);
    if ui::confirm("Report your own new posts too?", false)? {
        own.posts = Some(ask_posts()?);
    }
    let mut accounts = vec![own];

    if ui::confirm("Also watch somebody else's account?", false)? {
        loop {
            let name = ui::prompt_line("Whose? (their username, or blank to stop)")?;
            let name = snob_engine::engine::target::clean(name.trim());
            if name.is_empty() {
                break;
            }
            if let Some(why) = refused_name(name, own_name, &accounts) {
                ui::warn(&why);
                continue;
            }
            let what = ui::menu::choose(
                &format!("What of @{} should be watched?", printable(name)),
                &["Their followers and following", "Their new posts", "Both"],
            )?;
            let Some(what) = what else {
                continue;
            };
            let (lists, posts) = (what != 1, what != 0);
            // Reading their posts asks no consent, as `snob posts` does not;
            // walking their lists does, and the answer is what is recorded.
            let consent = if lists {
                ui::warn(
                    "reading somebody else's lists is a heavier request than reading your \
                     own, and Instagram is readier to refuse it. A scheduled run cannot ask, \
                     so the answer is recorded in the file.",
                );
                if !ui::confirm(&consent_question(name, webhook), false)? {
                    continue;
                }
                Some(snob_core::clock::now())
            } else {
                None
            };
            accounts.push(Answered {
                target: name.to_string(),
                consent,
                lists,
                posts: if posts { Some(ask_posts()?) } else { None },
            });
        }
    }
    Ok(accounts)
}

/// Where an account's new posts are saved, asked until the answer is blank
/// (nothing saved) or a whole path: a run on a timer starts in a directory of
/// its own, so a relative one would land wherever that is.
fn ask_posts() -> Result<config::PostsConfig> {
    loop {
        let answer = ui::prompt_line(
            "Save their photos and videos in which directory? (a whole path, or blank to \
             save nothing)",
        )?;
        match save_to_of(&answer) {
            Ok(save_to) => {
                return Ok(config::PostsConfig {
                    save_to,
                    quality: None,
                });
            }
            Err(why) => ui::warn(&why),
        }
    }
}

/// What a directory answer means: blank is nothing saved, a whole path is
/// that directory, and anything else is asked again.
fn save_to_of(answer: &str) -> std::result::Result<Option<std::path::PathBuf>, String> {
    let answer = answer.trim();
    if answer.is_empty() {
        return Ok(None);
    }
    let dir = std::path::PathBuf::from(answer);
    if dir.is_absolute() {
        Ok(Some(dir))
    } else {
        Err(format!(
            "{} is not a whole path; write it from the root of a drive",
            printable(answer)
        ))
    }
}

/// The question the recorded consent is an answer to.
///
/// **It names the webhook, because the answer is used for two things.**
///
/// An attended `snob watch once <stranger> --webhook …` is told the same before
/// `engine::ask_consent_with` asks (`once`, through `report::sent_to`): that one
/// has no access to the delivery configuration and must not be given one —
/// `ListArgs` carries none, and putting a `WatchConfig` inside `engine::list` is
/// the layer AGENTS.md keeps free of how anything is delivered.
fn consent_question(name: &str, webhook: Option<&str>) -> String {
    match webhook {
        Some(url) => format!(
            "Record that you agreed to read @{}'s lists, and to have what changes in them sent \
             to {}?",
            printable(name),
            snob_engine::watch::webhook::shown_str(url)
        ),
        None => format!(
            "Record that you agreed to read @{}'s lists?",
            printable(name)
        ),
    }
}

/// The format an address's host is known to take: Slack's and Discord's
/// incoming webhooks refuse snob's JSON, and ntfy.sh shows it as braces.
fn format_for(url: &str) -> config::WebhookFormat {
    use config::WebhookFormat;
    let Ok(url) = url::Url::parse(url) else {
        return WebhookFormat::Json;
    };
    match url.host_str().unwrap_or_default() {
        "hooks.slack.com" => WebhookFormat::Slack,
        "discord.com" | "discordapp.com" if url.path().starts_with("/api/webhooks/") => {
            WebhookFormat::Discord
        }
        "ntfy.sh" => WebhookFormat::Ntfy,
        _ => WebhookFormat::Json,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::watch::fixtures::watch_toml;

    /// `--dry-run` does not read the file it has already decided not to touch.
    ///
    /// The read was the scrutinee of a let-chain, so its `?` fired before the
    /// flag could short-circuit, and `snob watch setup --dry-run` -- documented
    /// as "print what would be written and write nothing" -- died on an existing
    /// `watch.toml` with a typo in it. That is the file somebody runs
    /// `--dry-run` to compare against.
    ///
    /// Both directions: a real run has to keep reading, or it would replace a
    /// configuration without showing what it was replacing.
    #[test]
    fn a_dry_run_does_not_read_the_file_it_will_not_touch() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        config::write(
            &paths,
            "schema = 1
evry = \"6h\"
",
        )
        .unwrap();

        assert!(
            existing_to_replace(&paths, true).unwrap().is_none(),
            "a dry run writes nothing, so it has nothing to ask about replacing"
        );
        assert!(
            existing_to_replace(&paths, false).is_err(),
            "a real run is about to overwrite it, so it still has to read it"
        );
    }

    /// The offer describes the walk it is about to make.
    ///
    /// `baseline_now` walks every `[[account]]` the file names, and the wizard
    /// writes two the moment somebody adds a friend: `self`, which usually has
    /// the captures already, and the friend, walked at `Pace::third_party()`,
    /// the same cadence with fewer retries.
    /// The sentence has to cover both.
    ///
    /// Asked of `baseline_cost` and not of `offer_the_baseline`: that one prints
    /// and prompts, and returns before the sentence without a terminal.
    #[test]
    fn the_offer_describes_the_walk_it_is_about_to_make() {
        use snob_engine::engine::check::{CheckReport, Checked, What};

        let account = |followers, following| Checked {
            what: What::Account {
                target: None,
                pk: Some(Pk::new(1)),
                followers,
                following,
                may_run_unattended: true,
                lists: true,
            },
            verdict: Verdict::Ok,
            problem: None,
        };

        let both = CheckReport {
            checked: vec![account(Some(512), Some(340)), account(Some(88), Some(12))],
        };
        let cost = baseline_cost(&both, true);
        assert_eq!(
            (cost.accounts, cost.followers, cost.following),
            (2, 600, 352),
            "every account the walk covers, not the first one with counters on it"
        );
        assert_eq!(
            cost.pages,
            72 + 9,
            "counted per account, because each of them pages on its own"
        );
        assert_eq!(
            cost.requests,
            146 + 20,
            "each list opened, and each page followed by its show_many"
        );

        let sentence = cost.sentence();
        assert!(sentence.contains("600 followers"), "{sentence}");
        assert!(sentence.contains("2 accounts"), "{sentence}");
        assert!(
            sentence.ends_with(
                "roughly 166 requests, with 2 pauses of five to fifteen minutes, \
                 one about every 480 accounts."
            ),
            "eighty-one pages fill two sittings: {sentence}"
        );

        // An account the preflight could not size is walked all the same, so it
        // is said rather than folded in as nothing.
        let partial = CheckReport {
            checked: vec![account(Some(512), Some(340)), account(None, None)],
        };
        let cost = baseline_cost(&partial, true);
        assert_eq!((cost.accounts, cost.uncounted), (2, 1));
        assert_eq!((cost.pages, cost.requests), (72, 146));
        assert!(
            cost.sentence().contains("could not be counted"),
            "{}",
            cost.sentence()
        );
    }

    /// The time the offer names grows with the walk: a few minutes within one
    /// sitting, its rests past one, and more than a day past the day's
    /// accounts. Understating it is the unsafe direction for a confirmation.
    #[test]
    fn the_offer_names_how_long_the_walk_takes() {
        let cost = |followers: u64, following: u64| BaselineCost {
            accounts: 1,
            followers,
            following,
            pages: snob_engine::engine::walk::pages_to_walk(followers)
                + snob_engine::engine::walk::pages_to_walk(following),
            requests: snob_engine::engine::walk::requests_to_walk(followers, true)
                + snob_engine::engine::walk::requests_to_walk(following, true),
            uncounted: 0,
            posts: 0,
        };

        let one_sitting = cost(300, 180).sentence();
        assert!(
            one_sitting.ends_with("roughly 82 requests, a few minutes."),
            "{one_sitting}"
        );

        let one_rest = cost(300, 192).sentence();
        assert!(
            one_rest.ends_with(
                "roughly 84 requests, with 1 pause of five to fifteen minutes, \
                 one about every 480 accounts."
            ),
            "{one_rest}"
        );

        let past_the_day = cost(1_800, 600).sentence();
        assert!(
            past_the_day.contains(
                "roughly 402 requests, over more than a day, since at most 2000 accounts \
                 are read in a day, and 4 pauses"
            ),
            "{past_the_day}"
        );
    }

    /// Consent recorded about one act is read as authority for another, so the
    /// question has to name both.
    ///
    /// `[account.consent] agreed_at` is an answer to "may this read @friend's
    /// lists?", and `Watched::may_run_unattended` then reads it as the authority
    /// for POSTing @friend's arrivals and departures to a third-party server on
    /// every run. The wizard already holds the address when it asks, so it can
    /// name the destination as well as the person.
    #[test]
    fn the_consent_question_names_where_the_names_are_sent() {
        let asked = consent_question("friend", Some("https://n8n.local/webhook/snob"));
        assert!(asked.contains("@friend"), "{asked}");
        assert!(asked.contains("https://n8n.local/webhook/snob"), "{asked}");

        // With nowhere to send it, there is nothing extra to agree to and the
        // question stays the short one.
        let plain = consent_question("friend", None);
        assert!(plain.contains("@friend"), "{plain}");
        assert!(!plain.contains("sent to"), "{plain}");

        // And the file reads back as one fact rather than two unrelated lines.
        let both = describe_config(&watch_toml(
            "schema = 1\nevery = \"6h\"\n\n[webhook]\nurl = \"https://n8n.local/hook\"\n\n\
             [[account]]\ntarget = \"self\"\n\n[[account]]\ntarget = \"friend\"\n\
             [account.consent]\nagreed_at = 1\n",
        ))
        .join("\n");
        assert!(
            both.contains("go to https://n8n.local/hook"),
            "watching somebody else and sending it somewhere is one fact: {both}"
        );

        // Your own account alone has nobody else's names in it, so there is
        // nothing to join.
        let alone = describe_config(&watch_toml(
            "schema = 1\nevery = \"6h\"\n\n[webhook]\nurl = \"https://n8n.local/hook\"\n",
        ))
        .join("\n");
        assert!(!alone.contains("Their usernames"), "{alone}");
    }

    /// Each webhook secret is stored under the `Kind` that says what it is for.
    ///
    /// One of them is sent verbatim to the user's server as the `Authorization`
    /// value on every request; the other is the shared secret every signature is
    /// computed from and must never leave this machine. Nothing downstream can
    /// tell a swap: `plan` and `sign` each read whatever is under their `Kind`.
    ///
    /// The two secrets carry different values on purpose: a test using one
    /// string cannot fail on a swap, which is the whole failure.
    ///
    /// `store_secrets` is reachable and `setup` is not -- `ui::can_show_a_menu()`
    /// is false under `cargo test` -- so the pairing lives on the type rather
    /// than in the middle of the wizard.
    #[test]
    fn each_webhook_secret_is_stored_under_the_kind_that_says_what_it_is_for() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        // Its own service name, always. The keyring belongs to the operating
        // system and not to this process.
        let secrets = SecretStore::new(paths, true).with_service(&format!(
            "snob-ig-test-webhook-answers-{}",
            std::process::id()
        ));

        let answers = WebhookAnswers {
            url: Some("https://n8n.local/webhook/snob".to_string()),
            heartbeat: false,
            headers: vec![],
            signing_key: Some(Secret::new("signing-key-never-leaves-this-machine")),
            token: Some(Secret::new("Bearer token-the-receiver-already-has")),
        };

        if answers.store_secrets(&secrets).is_err() {
            return; // no keyring on this machine; `save_secret` says why
        }

        let stored = |kind| {
            secrets
                .load_secret(kind)
                .unwrap()
                .found()
                .map(|s| s.expose().to_string())
        };

        assert_eq!(
            stored(Kind::WatchSigningKey).as_deref(),
            Some("signing-key-never-leaves-this-machine"),
            "the signing key became the Authorization value, and is now sent to the \
             receiver on every request"
        );
        assert_eq!(
            stored(Kind::WatchToken).as_deref(),
            Some("Bearer token-the-receiver-already-has"),
            "the token became the signing key, so every signature is computed with a \
             value the receiver already holds"
        );

        secrets.forget_secret(Kind::WatchSigningKey).unwrap();
        secrets.forget_secret(Kind::WatchToken).unwrap();
    }

    /// A name typed twice must not produce a file the wizard cannot read back.
    ///
    /// `[webhook.headers]` is a TOML table and TOML refuses a duplicate key, and
    /// correcting a mistyped `X-Api-Key` by typing it again is the ordinary
    /// case.
    #[test]
    fn a_header_named_twice_does_not_produce_a_file_the_tool_cannot_read() {
        let headers = last_of_each(vec![
            ("X-Api-Key".to_string(), "a".to_string()),
            ("X-Api-Key".to_string(), "b".to_string()),
        ]);

        let text = config::template(
            &watch_toml(&format!(
                "schema = 1\nevery = \"6h\"\n\n[webhook]\nurl = \"https://n8n.local/hook\"\n\n\
                 [webhook.headers]\n{}\n",
                headers
                    .iter()
                    .map(|(name, value)| format!("\"{name}\" = \"{value}\""))
                    .collect::<Vec<_>>()
                    .join("\n")
            )),
            false,
            |_| None,
        );
        let parsed = config::parse(&text, std::path::Path::new("watch.toml"))
            .expect("the wizard must not write a file it cannot read back");

        assert_eq!(
            parsed.webhook.unwrap().headers["X-Api-Key"],
            "b",
            "the value typed last is the one that was meant"
        );
    }

    /// Setup refuses a schedule the scheduler would refuse anyway, but it does
    /// it while the person who typed it is still reading.
    ///
    /// Asked of the functions `ask_schedule` calls, not of `Schedule` itself,
    /// which would say nothing about whether `setup` asks it; `config::parse`
    /// cannot catch it either. All three shapes, because the floor binds all
    /// three.
    #[test]
    fn a_schedule_below_the_floor_is_refused_at_setup() {
        assert!(interval_of("5m").is_err(), "five minutes is too often");
        assert!(
            cron_of("*/5 * * * *").is_err(),
            "the same five minutes, written the other way"
        );
        assert!(
            calendar_of("", "09:00,09:05").is_err(),
            "and again, as two moments five minutes apart"
        );

        // And the fields a good answer produces, since the file is written from
        // them.
        assert_eq!(
            interval_of("6h").unwrap(),
            std::time::Duration::from_secs(21_600)
        );
        assert_eq!(cron_of(" 0 9 * * 1,4 ").unwrap(), "0 9 * * 1,4");
        assert_eq!(
            calendar_of("mon, thu", "09:00, 21:00").unwrap(),
            (
                vec!["mon".to_string(), "thu".to_string()],
                vec!["09:00".to_string(), "21:00".to_string()]
            )
        );
        assert!(
            calendar_of("", "09:00").unwrap().0.is_empty(),
            "no days means every day, and no `on` key at all"
        );
    }

    /// The wizard and a run read a calendar with the same function, so what
    /// one accepts the other accepts, and the refusals are word for word.
    #[test]
    fn the_wizard_reads_a_calendar_the_way_a_run_does() {
        for (days, times) in [
            ("mon,thu", "09:00"),
            ("", "09:00,21:00"),
            ("tues", "09:00"),
            ("mon", "25:00"),
            ("", "09:00,09:05"),
        ] {
            let wizard = calendar_of(days, times);
            let run = calendar_from(
                &days
                    .split(',')
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .collect::<Vec<_>>(),
                &times
                    .split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>(),
            );

            assert_eq!(
                wizard.is_ok(),
                run.is_ok(),
                "{days:?} {times:?}: the wizard and a run disagree"
            );
            if let (Err(a), Err(b)) = (&wizard, &run) {
                assert_eq!(a.to_string(), b.to_string(), "{days:?} {times:?}");
            }
        }
    }

    /// The wizard offers only the jitter the schedule can absorb.
    ///
    /// `Schedule::with_jitter` clamps to what the grid can take and says
    /// nothing, so a value larger than the room becomes a smaller one at every
    /// start while `status` reads back what the file says. The room is zero for
    /// two shapes this tool advertises -- `*/15 * * * *`, whose grid is exactly
    /// the floor, and `--every 2w --on mon`, a weekly grid under a fortnightly
    /// floor -- and on those the wizard has nothing to ask and says so.
    ///
    /// The ceiling is asked of the schedule rather than worked out here:
    /// `room_for_jitter` is private, and a second copy of that arithmetic is how
    /// the wizard and a run come to disagree.
    ///
    /// What this cannot reach, said plainly: `ask_jitter`'s prompt and its
    /// refusal are behind `ui::prompt_line`, which answers nothing without a
    /// terminal, so what is pinned is the ceiling they ask against and the
    /// zero-room path that returns before asking. A schedule cannot have a
    /// default jitter larger than its room -- `fitted` clamps it at
    /// construction -- so the two are only ever told apart where there is room
    /// to spare, which is the first assertion here.
    #[test]
    fn the_wizard_offers_only_the_jitter_the_schedule_can_absorb() {
        // Six-hourly: the interval is the bound, because an interval measured
        // from the previous run slides the whole schedule.
        let interval = schedule::Schedule::every(std::time::Duration::from_secs(21_600)).unwrap();
        assert_eq!(
            room_for(&interval),
            std::time::Duration::from_secs(21_600),
            "an interval has no grid to miss, so the interval is the bound"
        );

        // The two with nothing to give. `ask_jitter` answers `None` on both
        // without asking anything, so no `jitter` key is written.
        for none_at_all in [
            schedule::Schedule::cron("*/15 * * * *").unwrap(),
            schedule::Schedule::days(
                &[snob_core::watch::schedule::Weekday::Mon],
                std::time::Duration::from_secs(14 * 24 * 3_600),
            )
            .unwrap(),
        ] {
            assert!(
                room_for(&none_at_all).is_zero(),
                "{none_at_all:?} has no room, and the question must not be asked"
            );
            assert_eq!(
                ask_jitter(&none_at_all).unwrap(),
                None,
                "nothing to ask, so nothing is written"
            );
        }

        // And a schedule with room does not accept more than it has: a run at
        // its moment pushed past the next one is the failure the ceiling exists
        // for, and it is silent when the clamp does it.
        let daily = schedule::Schedule::cron("0 9 * * *").unwrap();
        let room = room_for(&daily);
        assert!(!room.is_zero(), "a daily schedule has a day to play with");
        assert_eq!(
            daily
                .clone()
                .with_jitter(room + std::time::Duration::from_secs(60))
                .jitter(),
            room,
            "the clamp is what the wizard has to refuse in front of, not repeat"
        );
    }

    /// The wizard replaces the entries of the account it runs as, and every
    /// entry read as another account is written back as it was.
    #[test]
    fn setup_keeps_the_entries_of_other_viewers() {
        let existing = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "self"
viewer = 1

[[account]]
target = "friend"
viewer = 1
[account.consent]
agreed_at = 5

[[account]]
target = "self"
viewer = 2

[[account]]
target = "old"
viewer = 2
[account.consent]
agreed_at = 6
"#,
        );

        let written = entries_to_write(
            vec![
                Answered::lists("self", None),
                Answered::lists("new", Some(Epoch::new(9))),
            ],
            Some(Pk::new(2)),
            Some(&existing),
        );

        let shape: Vec<_> = written
            .iter()
            .map(|entry| {
                (
                    entry.target.as_str(),
                    entry.viewer,
                    entry.consent.map(|c| c.agreed_at),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                ("self", Some(Pk::new(1)), None),
                ("friend", Some(Pk::new(1)), Some(Epoch::new(5))),
                ("self", Some(Pk::new(2)), None),
                ("new", Some(Pk::new(2)), Some(Epoch::new(9))),
            ],
            "@2's entries are the answers; @1's are untouched"
        );

        let text = config::template(
            &WatchConfig {
                accounts: written,
                ..existing
            },
            false,
            |pk| (pk == Pk::new(2)).then(|| "me".to_string()),
        );
        assert!(
            text.contains(
                "viewer = 2  # @me
"
            ),
            "{text}"
        );
        assert_eq!(watch_toml(&text).accounts.len(), 4, "{text}");
    }

    /// With nobody signed in, the answers replace every entry: kept beside
    /// them, an entry naming a viewer would be read again once somebody signs
    /// in as that viewer and the unnamed answers are read as them too.
    #[test]
    fn setup_with_nobody_signed_in_replaces_every_entry() {
        let existing = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "self"
viewer = 1
"#,
        );

        let written = entries_to_write(
            vec![
                Answered::lists("self", None),
                Answered::lists("friend", Some(Epoch::new(9))),
            ],
            None,
            Some(&existing),
        );

        let shape: Vec<_> = written
            .iter()
            .map(|entry| (entry.target.as_str(), entry.viewer))
            .collect();
        assert_eq!(shape, [("self", None), ("friend", None)]);
    }

    /// An account whose posts alone are watched is written with no consent
    /// and `lists = false`, and an entry of another viewer keeps its posts.
    #[test]
    fn a_posts_only_answer_is_written_as_one_and_others_keep_their_posts() {
        let existing = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "school"
viewer = 1
lists = false
[account.posts]
"#,
        );
        let dir = if cfg!(windows) {
            r"D:\media"
        } else {
            "/srv/media"
        };
        let written = entries_to_write(
            vec![Answered {
                target: "club".into(),
                consent: None,
                lists: false,
                posts: Some(config::PostsConfig {
                    save_to: Some(dir.into()),
                    quality: None,
                }),
            }],
            Some(Pk::new(2)),
            Some(&existing),
        );
        assert_eq!(written.len(), 2);
        assert!(
            !written[0].lists && written[0].posts.is_some(),
            "kept as it was"
        );
        assert!(!written[1].lists);
        assert_eq!(written[1].consent, None);
        let text = config::template(
            &WatchConfig {
                accounts: written.clone(),
                ..existing
            },
            false,
            |_| None,
        );
        assert_eq!(watch_toml(&text).accounts, written, "{text}");
    }

    /// The offer counts the posts it will read, and not the lists of an
    /// account whose lists are not watched.
    #[test]
    fn the_offer_counts_the_posts_it_reads_and_not_the_lists_it_leaves_alone() {
        use snob_engine::engine::check::{CheckReport, Checked, What};

        let report = CheckReport {
            checked: vec![
                Checked {
                    what: What::Account {
                        target: Some("school".into()),
                        pk: Some(Pk::new(7)),
                        followers: Some(5_000),
                        following: Some(10),
                        may_run_unattended: true,
                        lists: false,
                    },
                    verdict: Verdict::Ok,
                    problem: None,
                },
                Checked {
                    what: What::Posts {
                        target: Some("school".into()),
                        save_to: None,
                        baseline_at: None,
                    },
                    verdict: Verdict::Warned,
                    problem: None,
                },
            ],
        };
        let cost = baseline_cost(&report, true);
        assert_eq!((cost.accounts, cost.followers, cost.posts), (0, 0, 1));
        assert_eq!(cost.requests, POSTS_BASELINE_REQUESTS);
        assert!(
            cost.sentence().contains("the newest posts of 1 account"),
            "{}",
            cost.sentence()
        );
    }

    /// A name is refused while the person is still there when it cannot be
    /// a username, is the own account, or was answered for already.
    #[test]
    fn a_name_that_cannot_be_watched_is_asked_again() {
        let answered = [
            Answered::lists("self", None),
            Answered::lists("friend", Some(Epoch::new(1))),
        ];
        assert!(refused_name("john doe", Some("me"), &answered).is_some());
        assert!(refused_name("self", Some("me"), &answered).is_some());
        assert!(refused_name("ME", Some("me"), &answered).is_some());
        assert!(refused_name("Friend", Some("me"), &answered).is_some());
        assert_eq!(refused_name("other.one", Some("me"), &answered), None);
    }

    /// A jitter answer is checked against the room the schedule has, and
    /// blank leaves it to the schedule.
    #[test]
    fn a_jitter_answer_holds_or_says_why_not() {
        let schedule =
            schedule::Schedule::every(std::time::Duration::from_secs(6 * 3_600)).unwrap();
        assert_eq!(jitter_of(&schedule, " ").unwrap(), None);
        assert_eq!(
            jitter_of(&schedule, "10m").unwrap(),
            Some(std::time::Duration::from_secs(600))
        );
        assert!(jitter_of(&schedule, "soon").is_err());
        // A grid whose gap is the floor itself has no room at all.
        let tight = schedule::Schedule::cron("*/15 * * * *").unwrap();
        assert!(jitter_of(&tight, "10m").is_err());
    }

    #[test]
    fn a_directory_answer_is_blank_or_a_whole_path() {
        assert_eq!(save_to_of("  "), Ok(None));
        assert!(
            save_to_of("media").is_err(),
            "a relative path is asked again"
        );
        let whole = if cfg!(windows) {
            r"D:\media"
        } else {
            "/srv/media"
        };
        assert_eq!(save_to_of(whole), Ok(Some(whole.into())));
    }

    #[test]
    fn an_address_on_a_known_service_is_set_up_in_its_format() {
        use config::WebhookFormat;
        for (url, format) in [
            (
                "https://hooks.slack.com/services/T0/B0/x",
                WebhookFormat::Slack,
            ),
            (
                "https://discord.com/api/webhooks/1/abc",
                WebhookFormat::Discord,
            ),
            ("https://ntfy.sh/my-topic", WebhookFormat::Ntfy),
            ("https://n8n.local/webhook/snob", WebhookFormat::Json),
            ("https://discord.com/channels/1", WebhookFormat::Json),
        ] {
            assert_eq!(format_for(url), format, "{url}");
        }
    }
}
