//! The `snob` binary: the command line parsed, the run wired (trust, paths,
//! the keyring, the updater, the owner of the browsers), and the command
//! dispatched as the account it resolves to.

use anyhow::Context;
use clap::Parser;
use snob_cli::cli::{Cli, Command};
use snob_cli::commands;
use snob_cli::commands::sets::SetOp;
use snob_core::model::ListKind;
use snob_engine::account::Unresolved;
use snob_engine::exit::ExitCode;
use snob_store::paths::{AccountPaths, AppPaths};
use snob_store::registry::Registry;
use snob_store::secrets::SecretStore;

/// Two workers, always two, whatever the machine has.
///
/// Too few because on a one-vCPU server or container the default is **one**,
/// and that is the case that breaks something.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    // First, so even `snob` typed with no command (`no_command`) has it.
    snob_cli::front::install();
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if e.kind() == clap::error::ErrorKind::MissingSubcommand => {
            return no_command();
        }
        Err(e) => e.exit(),
    };
    if let Err(refused) = cli.refuse_a_named_account() {
        refused.exit();
    }
    init_tracing(cli.verbose, matches!(cli.command, Command::BrowserOwner));
    restore_terminal_on_panic();

    let wording = wording_for(&cli);
    // Awaited in place, the future of `run` would be part of this function's
    // own, which a debug build copies from frame to frame on its way into
    // the runtime (`boxed`, below, has the numbers).
    let code = match boxed(|| run(cli)).await {
        Ok(code) => code,
        Err(e) => {
            snob_cli::report::print_error(&e, wording);
            snob_engine::exit::exit_code_for(&e)
        }
    };
    std::process::ExitCode::from(snob_engine::exit::code(code))
}

/// Makes a future in a frame of its own and hands it back on the heap.
///
/// **Every future `main`, `run` and `dispatch` await is made through this,
/// and a new command's must be** (AGENTS.md, "Code conventions"): a debug
/// build gives every arm's future a place in `dispatch`'s frame at once, on a
/// main thread Windows gives 1 MiB. Measured on `snob status --jobs --json`
/// with a job running, a debug build with `testing` on Windows, November
/// 2026: 402 KiB down with only `run`'s future boxed, 60 KiB with every one;
/// to spare, 96 to 112 KiB with nothing boxed, 480 to 512 now.
///
/// **A closure and not the future**: an argument is made in the caller's
/// frame before the call, so `boxed(run(cli))` would leave its place where it
/// was. `#[inline(never)]` says the same to a build that does optimize.
#[inline(never)]
fn boxed<F: Future>(make: impl FnOnce() -> F) -> std::pin::Pin<Box<F>> {
    Box::pin(make())
}

/// The list command and its arguments, when it was given `--background`.
fn in_the_background(command: &Command) -> Option<(&'static str, &snob_cli::cli::ListArgs)> {
    let (name, args) = match command {
        Command::Followers(args) => ("followers", args),
        Command::Following(args) => ("following", args),
        Command::Unfollowers(args) => ("unfollowers", args),
        Command::Fans(args) => ("fans", args),
        Command::Friends(args) => ("friends", args),
        _ => return None,
    };
    args.background.then_some((name, args))
}

/// `snob` typed with no command.
fn no_command() -> std::process::ExitCode {
    use clap::CommandFactory;
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        Cli::command()
            .error(
                clap::error::ErrorKind::MissingSubcommand,
                "snob needs a command; \"snob --help\" lists them",
            )
            .exit();
    }
    let line = std::env::args_os().chain(std::iter::once("status".into()));
    let cli = Cli::try_parse_from(line).unwrap_or_else(|e| e.exit());
    let env = std::env::var("SNOB_ACCOUNT").ok();
    let shown = wiring(&cli).and_then(|(paths, _, _)| {
        commands::start::run(&paths, cli.account.as_deref(), env.as_deref())
    });
    let code = shown.unwrap_or_else(|e| {
        snob_cli::report::print_error(&e, snob_cli::report::Wording::Prose);
        snob_engine::exit::exit_code_for(&e)
    });
    std::process::ExitCode::from(snob_engine::exit::code(code))
}

/// Whether a failure is told in JSON: when the answer was going to be.
fn wording_for(cli: &Cli) -> snob_cli::report::Wording {
    use snob_cli::cli::{AccountCommand, Format, JobsCommand, WatchCommand};
    use snob_cli::output::effective_format;
    use snob_cli::report::Wording;

    let document = |format: Option<Format>, to: Option<&std::path::Path>| {
        matches!(effective_format(format, to), Format::Json | Format::Ndjson)
    };
    let json = match &cli.command {
        Command::Followers(args)
        | Command::Following(args)
        | Command::Unfollowers(args)
        | Command::Fans(args)
        | Command::Friends(args) => document(args.output.format, args.output.path.as_deref()),
        Command::Scan(args) => document(args.output.format, args.output.path.as_deref()),
        Command::Snapshots(args) => document(args.output.format, args.output.path.as_deref()),
        Command::Changes(args) => document(args.output.format, args.output.path.as_deref()),
        // `-o` is where the listing goes only when listing; with `-d` it is
        // where the download goes, and names no format.
        Command::Stories(args) => {
            document(args.list.format.map(Format::from), args.action.listing_to())
        }
        Command::Highlights(args) => {
            document(args.list.format.map(Format::from), args.action.listing_to())
        }
        Command::Posts(args) => {
            document(args.list.format.map(Format::from), args.action.listing_to())
        }
        Command::Post(args) => {
            args.json || document(args.list.format.map(Format::from), args.action.listing_to())
        }
        Command::Likers(args) => document(args.output.format, args.output.path.as_deref()),
        Command::Profile(args) => matches!(
            effective_format(args.format.map(Format::from), args.output.as_deref()),
            Format::Json
        ),
        Command::Whoami(args) => args.output.json,
        Command::Status(args) => args.output.json,
        Command::Account(AccountCommand::List(args)) => args.output.json,
        Command::Follow(args) | Command::Unfollow(args) => args.output.json,
        // It takes no format flag and answers in JSON down a pipe, the way
        // `commands::import::run` decides it.
        Command::Import(_) => document(None, None),
        Command::Watch(args) => match &args.command {
            None => args.run.output.json,
            Some(WatchCommand::Once(once)) => once.output.json,
            Some(WatchCommand::Check(check)) => check.output.json,
            Some(WatchCommand::Status(status)) => status.output.json,
            Some(WatchCommand::Deliver(deliver)) => deliver.output.json,
            Some(WatchCommand::Unit(_)) => false,
            Some(WatchCommand::Diff(diff)) => diff.output.json,
            Some(WatchCommand::Setup(_)) => false,
        },
        Command::Account(AccountCommand::Use(_))
        | Command::Login(_)
        | Command::Purge(_)
        | Command::Pfp(_)
        | Command::Fetch(_)
        | Command::BrowserOwner => false,
        // Read by a program only as `snob mcp` runs it (`--only`), which
        // tells its refusal apart by the reason.
        Command::Logout(args) => args.only.is_some(),
        Command::Update(args) => args.json,
        // Detached, with nobody reading: their errors go to the log.
        Command::Updater(_) => false,
        // A job's own processes answer in JSON, as the server reading them
        // does (`jobs::start`); the supervisor's errors go to its log.
        Command::Job(snob_cli::cli::JobCommand::Run(_)) => false,
        Command::Job(_) => true,
        Command::Jobs(args) => match &args.command {
            None => args.list.output.json,
            Some(JobsCommand::List(list)) => list.output.json,
            Some(JobsCommand::Show(show)) => show.output.json,
            Some(JobsCommand::Cancel(cancel)) => cancel.output.json,
        },
        // The protocol is on standard output; its failures are JSON-RPC.
        #[cfg(feature = "mcp")]
        Command::Mcp(_) => false,
    };
    if json { Wording::Json } else { Wording::Prose }
}

/// Whether a file really holds PEM certificates.
///
/// **Emptiness is the case that matters**, and it is why this is not a bare
/// `is_err()`.
fn holds_a_certificate(pem: &[u8]) -> anyhow::Result<()> {
    match snob_ig::http::reqwest::Certificate::from_pem_bundle(pem) {
        Ok(found) if !found.is_empty() => Ok(()),
        Ok(_) => anyhow::bail!("it holds no PEM certificates"),
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

fn trust_from(cli: &Cli) -> anyhow::Result<snob_ig::http::Trust> {
    if !cli.strict_roots {
        return Ok(snob_ig::http::Trust::Platform);
    }
    if !snob_ig::http::CAN_NARROW {
        anyhow::bail!(
            "--strict-roots does nothing on this build, so it is refused rather than \
             ignored.\n\
             This is the Windows on ARM64 binary, which uses the operating system's TLS \
             stack; that stack has no way to be told \"these roots and no others\"."
        );
    }

    let mut extra = Vec::new();
    for path in &cli.tls_extra_root {
        // Read and checked here rather than at the first request, so a typo in
        // a path is an error before anything has been walked -- and so the
        // failure names the file rather than arriving as a handshake error
        // hours later.
        let pem =
            std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
        holds_a_certificate(&pem)
            .with_context(|| format!("{} is not usable as an extra root", path.display()))?;
        extra.push(pem);
    }
    Ok(snob_ig::http::Trust::Narrow { extra })
}

/// Where this run keeps its files, whether the keyring is off, and — for a
/// sandbox — the keyring namespace it is confined to.
///
/// **Forcing the backend is not enough on its own.**
fn wiring(cli: &Cli) -> anyhow::Result<(AppPaths, bool, Option<String>)> {
    // Before any client exists, which is what `use_trust` requires: a run
    // cannot change what it trusts halfway through.
    let trust = trust_from(cli)?;
    if let Err(already) = snob_ig::http::use_trust(trust.clone())
        && already != trust
    {
        anyhow::bail!("the trust store was already decided for this run");
    }

    #[cfg(feature = "testing")]
    if let Some(root) = &cli.sandbox_root {
        if let Some(base) = cli.ig_base_url.clone() {
            // Before any client is built, and once.
            snob_ig::client::point_every_client_at(base)
                .map_err(|already| anyhow::anyhow!("already pointed at {already}"))?;
        }
        // The browser path against the fake server too, so it can be driven end
        // to end.
        if cli.through_the_browser {
            snob_ig::client::page::use_the_page_off_instagram();
        }
        return Ok((
            AppPaths::rooted_at(root),
            true,
            Some(sandbox_keyring_namespace(root)),
        ));
    }
    Ok((AppPaths::discover()?, cli.no_keyring, None))
}

/// The keyring service name a sandbox run is confined to.
#[cfg(feature = "testing")]
fn sandbox_keyring_namespace(root: &std::path::Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("snob-ig-sandbox-{hash:016x}")
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let (paths, no_keyring, keyring_namespace) = wiring(&cli)?;
    snob_engine::app::wait_at_most(cli.max_wait);
    let sandboxed = keyring_namespace.is_some();

    // The updater: its own processes, `snob update`, and every other command
    // taking part (`update::at_start`), first, since applying a version may run
    // this command again from the new binary.
    match &cli.command {
        Command::Updater(snob_cli::cli::UpdaterCommand::Fetch) => {
            boxed(|| snob_engine::update::fetch(&paths)).await?;
            return Ok(ExitCode::Ok);
        }
        Command::Updater(snob_cli::cli::UpdaterCommand::Apply(apply)) => {
            snob_engine::update::apply_detached(&paths, &apply.exe)?;
            return Ok(ExitCode::Ok);
        }
        _ => {}
    }
    if let Command::Update(args) = cli.command {
        return boxed(|| commands::update::run(args, &paths, sandboxed)).await;
    }
    let role = match &cli.command {
        Command::BrowserOwner | Command::Updater(_) | Command::Update(_) => None,
        #[cfg(feature = "mcp")]
        Command::Mcp(_) => None,
        Command::Job(snob_cli::cli::JobCommand::Run(_)) => Some(snob_engine::update::Role::Counted),
        Command::Job(_) => None,
        _ => Some(snob_engine::update::Role::Command),
    };
    let _updating = match role {
        Some(role) => Some(snob_engine::update::at_start(
            &paths,
            sandboxed,
            role,
            &detached_globals(&cli)?,
        )),
        None => None,
    };
    if cli.stop_when_stdin_closes {
        snob_engine::app::stop_when_stdin_closes();
    }
    // The owner of the browsers and a job's supervisor are started to
    // outlive the command that started them (`detach`); everything else stops
    // with a program that drove it and went (`parent`).
    snob_engine::app::stop_when_parent_exits(matches!(
        cli.command,
        Command::BrowserOwner
            | Command::Job(snob_cli::cli::JobCommand::Run(_))
            | Command::Updater(_)
    ));
    if cli.spend_alone {
        snob_engine::app::spend_alone();
    }
    if cli.progress_json {
        snob_cli::progress::progress_json();
    }
    // Before the layout, the owner and the session: the server spends nothing
    // and stores nothing itself.
    #[cfg(feature = "mcp")]
    if let Command::Mcp(args) = &cli.command {
        return match &args.command {
            Some(snob_cli::cli::McpCommand::Config(config)) => commands::mcp::config::run(
                config,
                mcp_globals(&cli, cli.max_wait)?,
                cli.max_wait.unwrap_or(commands::mcp::MAX_WAIT),
            ),
            // The key is the machine's, in the keyring whatever
            // `--no-keyring` says (`commands::mcp::key`); a sandbox's under
            // its own name, like everything else it keeps there.
            #[cfg(feature = "mcp-http")]
            Some(snob_cli::cli::McpCommand::Key(key)) => {
                let store = SecretStore::new(paths.clone(), no_keyring);
                let store = match &keyring_namespace {
                    Some(service) => store.with_service(service),
                    None => store,
                };
                commands::mcp::key::run(key, &store)
            }
            None => {
                // `--max-wait` is the server's to hand each child, which
                // waits as long as what it runs allows (`mcp::Waits`).
                let launch = commands::mcp::child::Launch::new(
                    snob_engine::exe::program()?,
                    mcp_globals(&cli, None)?,
                );
                // The account the server is held to, by a flag or by the
                // environment its children inherit; a server for any account
                // is held to none, so the two are refused together.
                let env = std::env::var("SNOB_ACCOUNT").ok();
                let pinned = snob_engine::account::given(cli.account.as_deref(), env.as_deref());
                if args.any_account && pinned.is_some() {
                    return Err(snob_engine::exit::ExitError::new(
                        ExitCode::Error,
                        "--any-account lets a call say which account it acts as, and \
                         SNOB_ACCOUNT holds this server to one",
                    )
                    .with_hint("unset SNOB_ACCOUNT for this server, or leave --any-account out")
                    .into());
                }
                let settings = commands::mcp::Settings {
                    writes: args.allow_writes,
                    any_account: args.any_account,
                    pinned: pinned.and_then(commands::mcp::tools::acting_name),
                    downloads: args.downloads_dir.clone(),
                    in_chat: args.confirm_in_chat,
                    writes_a_day: args.max_writes_per_day,
                    max_wait: cli.max_wait,
                };
                // Over HTTP the one thing the server reads is the key its
                // clients present, from where `snob mcp key` keeps it.
                #[cfg(feature = "mcp-http")]
                if let Some(at) = args.listen {
                    let store = SecretStore::new(paths.clone(), no_keyring);
                    let store = match &keyring_namespace {
                        Some(service) => store.with_service(service),
                        None => store,
                    };
                    let listen = commands::mcp::http::Listen {
                        at,
                        public: args.public_url.clone(),
                        data_dir: paths.data_dir().to_path_buf(),
                    };
                    return boxed(|| commands::mcp::http::serve(launch, settings, listen, &store))
                        .await;
                }
                boxed(|| commands::mcp::serve(launch, settings)).await
            }
        };
    }
    if let Command::BrowserOwner = cli.command {
        let linger = if keyring_namespace.is_some() {
            snob_engine::owner::Linger::for_a_sandbox()
        } else {
            snob_engine::owner::Linger::for_people()
        };
        boxed(|| snob_engine::owner::serve(&paths, linger)).await?;
        return Ok(ExitCode::Ok);
    }
    let store = SecretStore::new(paths.clone(), no_keyring);
    let store = match &keyring_namespace {
        Some(service) => store.with_service(service),
        None => store,
    };

    // The single-account layout moves under its account before anything reads
    // it, with no browser of an older layout's owner still writing into it.
    if !matches!(
        cli.command,
        Command::Purge(_) | Command::Import(_) | Command::Fetch(_)
    ) && snob_store::layout::pending(&paths)
    {
        boxed(|| snob_engine::owner::quit(&paths)).await;
        let settled = snob_store::layout::settle(&paths, &store)?;
        if let Some(parked) = settled.parked {
            snob_cli::ui::warn(&format!(
                "a database from before several accounts could be signed in was set aside \
                 at {}, because its account already had one; nothing reads it now",
                parked.display()
            ));
        }
        if let Some(why) = settled.unreadable {
            snob_cli::ui::warn(&format!(
                "the session stored before several accounts could be signed in does not \
                 read ({why}), so it was set aside and nobody is signed in; log in again \
                 with \"snob login\". It may still be active on Instagram."
            ));
        }
    }

    // The jobs send nothing themselves and use no browser: a walk a job runs
    // is a process of its own, which comes through here again.
    if matches!(cli.command, Command::Job(_) | Command::Jobs(_)) {
        return boxed(|| snob_cli::jobs::dispatch::dispatch(&cli, &store, &paths)).await;
    }

    // Every request to Instagram goes out from a browser from here on, unless
    // this run was told not to — see `headless/`.
    if !snob_engine::headless::env_flag("SNOB_NO_BROWSER") {
        let start = if snob_engine::headless::env_flag("SNOB_NO_OWNER") {
            None
        } else {
            Some(owner_args(&cli)?)
        };
        snob_engine::owner::install(&paths, start);
    }
    snob_engine::owner::attended(somebody_waits(&cli));
    // A copy for the command, which consumes its store; the write-back needs
    // the same store, on the same keyring service, once it is done.
    let for_the_command = store.clone();
    let outcome = boxed(|| dispatch(cli, for_the_command, &paths, sandboxed)).await;
    // Whatever the command came to: the browser kept the session current
    // while it ran, and the stored copy learns what it kept.
    boxed(|| snob_engine::owner::finish(&store, &paths)).await;
    outcome
}

/// Whether somebody is waiting on this command, for the pace its browsers
/// run at (`owner::attended`): a command typed in a terminal, and not the
/// scheduled monitor, which runs for days and is waited on by nobody.
fn somebody_waits(cli: &Cli) -> bool {
    use std::io::IsTerminal;
    let monitor = matches!(&cli.command, Command::Watch(args) if args.command.is_none());
    !monitor && std::io::stderr().is_terminal()
}

/// Runs the command, as the account it resolves to when it acts as one.
async fn dispatch(
    cli: Cli,
    store: SecretStore,
    paths: &AppPaths,
    sandboxed: bool,
) -> anyhow::Result<ExitCode> {
    // Read here rather than by clap: `purge` is narrowed to one account only
    // by a flag somebody typed, never by the environment.
    let env = std::env::var("SNOB_ACCOUNT").ok();
    // Read only by a command that acts as an account: a damaged registry must
    // not stop `purge` from removing it.
    let resolved = || {
        let registry = Registry::load(paths)?;
        let found = snob_engine::account::resolve_if_any(
            cli.account.as_deref(),
            env.as_deref(),
            &registry,
        )?;
        if let Some(found) = &found {
            snob_cli::report::act_as(found.clone());
        }
        Ok::<_, anyhow::Error>(found.map(|found| paths.account(found.pk)))
    };
    // A command that has nothing to do without an account refuses as every
    // command does with no session.
    let required =
        || -> anyhow::Result<AccountPaths> { resolved()?.ok_or_else(commands::common::no_session) };
    let session = |account: &AccountPaths| store.session_of(account);

    // Whether this run was told which account, rather than left to the
    // registry.
    let named = snob_engine::account::given(cli.account.as_deref(), env.as_deref()).is_some();

    // A list walk asked for in the background is a job's to run, and needs
    // the whole command line (`jobs::argv::globals`), which the match below
    // takes apart.
    if let Some((command, args)) = in_the_background(&cli.command) {
        let account = required()?;
        return boxed(|| commands::background::run(&cli, command, args, &store, &account)).await;
    }

    match cli.command {
        Command::Login(args) => {
            // An account being added is none of those signed in here, and
            // with none active there is none to log in again as.
            let renewing = if args.add {
                None
            } else {
                let registry = Registry::load(paths)?;
                match snob_engine::account::resolve(
                    cli.account.as_deref(),
                    env.as_deref(),
                    &registry,
                ) {
                    Ok(found) => {
                        snob_cli::report::act_as(found.clone());
                        Some(found)
                    }
                    Err(Unresolved::NoAccount | Unresolved::NoneChosen) => None,
                    Err(refused) => return Err(refused.into_error()),
                }
            };
            boxed(|| commands::login::run(args, store, paths, renewing, named)).await
        }
        Command::Whoami(args) => {
            let account = resolved()?;
            boxed(|| commands::whoami::run(args, store, account)).await
        }
        Command::Status(args) => commands::status::run(args, &store, paths, resolved()?, sandboxed),
        Command::Logout(args) => {
            let account = if args.all { None } else { resolved()? };
            // `--only` (`snob_logout`): the account this run resolved has to be
            // the one named, or nothing is signed out.
            if let Some(only) = &args.only {
                let registry = Registry::load(paths)?;
                let named = snob_engine::account::named(only, &registry)
                    .ok()
                    .map(|named| named.pk);
                let acting = account.as_ref().map(AccountPaths::pk);
                if named.is_none() || named != acting {
                    return Err(snob_engine::exit::ExitError::new(
                        ExitCode::Error,
                        "This run acts as another account than the one it was asked to sign \
                         out, or the account is not one signed in here. Nothing was signed out.",
                    )
                    .with_reason("not_this_account")
                    .into());
                }
            }
            boxed(|| commands::logout::run(args, store, paths, account)).await
        }
        Command::Purge(args) => match cli.account.as_deref() {
            Some(name) => {
                boxed(|| commands::purge::run_for_account(args, store, paths, name)).await
            }
            // The whole data directory goes, the owner's log with it, so the
            // owner is asked to leave, not only to close its browsers, once
            // the jobs that would start it again have stopped.
            None => boxed(|| commands::purge::run_everything(args, store, paths)).await,
        },
        Command::Account(command) => commands::account::run(
            command,
            &store,
            paths,
            cli.account.as_deref(),
            env.as_deref(),
        ),
        Command::Followers(args) => {
            let account = required()?;
            boxed(|| commands::lists::run(args, store, &account, ListKind::Followers)).await
        }
        Command::Following(args) => {
            let account = required()?;
            boxed(|| commands::lists::run(args, store, &account, ListKind::Following)).await
        }
        Command::Profile(args) => {
            let account = required()?;
            boxed(|| commands::profile::run(args, store, &account)).await
        }
        Command::Scan(args) => {
            let account = required()?;
            boxed(|| commands::scan::run(args, store, &account)).await
        }
        Command::Snapshots(args) => commands::snapshots::run(args, &required()?),
        Command::Changes(args) => commands::changes::run(args, &required()?),
        Command::Unfollowers(args) => {
            let account = required()?;
            boxed(|| commands::sets::run(args, store, &account, SetOp::Unfollowers)).await
        }
        Command::Fans(args) => {
            let account = required()?;
            boxed(|| commands::sets::run(args, store, &account, SetOp::Fans)).await
        }
        Command::Friends(args) => {
            let account = required()?;
            boxed(|| commands::sets::run(args, store, &account, SetOp::Friends)).await
        }
        Command::Pfp(args) => {
            let account = required()?;
            boxed(|| commands::pfp::run(args, store, &account)).await
        }
        Command::Stories(args) => {
            let account = required()?;
            boxed(|| commands::stories::run(args, store, &account)).await
        }
        Command::Highlights(args) => {
            let account = required()?;
            boxed(|| commands::highlights::run(args, store, &account)).await
        }
        Command::Posts(args) => {
            let account = required()?;
            boxed(|| commands::posts::run(args, store, &account)).await
        }
        Command::Post(args) => {
            let account = required()?;
            boxed(|| commands::post::run(args, store, &account)).await
        }
        Command::Likers(args) => {
            let account = required()?;
            boxed(|| commands::likers::run(args, store, &account)).await
        }
        Command::Follow(args) => {
            let account = required()?;
            let verb = commands::follow::Verb::Follow;
            let secrets = session(&account);
            boxed(|| commands::follow::run(args, verb, secrets, &account)).await
        }
        Command::Unfollow(args) => {
            let account = required()?;
            let verb = commands::follow::Verb::Unfollow;
            let secrets = session(&account);
            boxed(|| commands::follow::run(args, verb, secrets, &account)).await
        }
        Command::Watch(args) => {
            use snob_cli::cli::WatchCommand;
            // `status`, `deliver` and `unit` act as no account: they read every
            // account's database, or none.
            let needs_none = matches!(
                args.command,
                Some(WatchCommand::Status(_) | WatchCommand::Deliver(_) | WatchCommand::Unit(_))
            );
            let account = if needs_none && !named {
                resolved().ok().flatten()
            } else {
                resolved()?
            };
            boxed(|| commands::watch::run(args, store, paths, account)).await
        }
        Command::Import(command) => commands::import::run(command),
        Command::Fetch(args) => {
            // Only the User-Agent is read off the account, and only when one
            // was not given; a registry or session that will not read is no
            // reason not to fetch a public file.
            let account_ua = || -> anyhow::Result<Option<String>> {
                let account = match resolved() {
                    Ok(Some(account)) => account,
                    Ok(None) => return Ok(None),
                    Err(e) if named => return Err(e),
                    Err(_) => return Ok(None),
                };
                Ok(session(&account)
                    .load()
                    .ok()
                    .flatten()
                    .map(|session| session.user_agent))
            };
            boxed(|| commands::fetch::run(args, account_ua)).await
        }
        // Answered at the top, and in `run`.
        Command::BrowserOwner
        | Command::Job(_)
        | Command::Jobs(_)
        | Command::Update(_)
        | Command::Updater(_) => Ok(ExitCode::Ok),
        // Answered in `run`.
        #[cfg(feature = "mcp")]
        Command::Mcp(_) => Ok(ExitCode::Ok),
    }
}

/// What the owner of the browsers is started with: the global flags that decide
/// where its files are, so that it lands on the same ones, and the hidden
/// command.
fn owner_args(cli: &Cli) -> anyhow::Result<Vec<std::ffi::OsString>> {
    let mut args = detached_globals(cli)?;
    args.push(snob_engine::owner::COMMAND.into());
    Ok(args)
}

/// The global flags a process started detached is given ahead of its hidden
/// command, the owner's and the updater's alike: where its files are, so it
/// lands on the same ones (a sandbox root made absolute, since it does not
/// run from here, `detach`), and `--verbose`.
fn detached_globals(cli: &Cli) -> anyhow::Result<Vec<std::ffi::OsString>> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    #[cfg(feature = "testing")]
    if let Some(root) = &cli.sandbox_root {
        args.push("--sandbox-root".into());
        args.push(std::path::absolute(root)?.into());
    }
    if cli.verbose {
        args.push("--verbose".into());
    }
    Ok(args)
}

/// What every command `snob mcp` runs for a tool call is started with, ahead of
/// its own arguments: the global flags the server was given, so the child lands
/// on the same files, account and trust, and `--max-wait` when `max_wait` is
/// given.
#[cfg(feature = "mcp")]
fn mcp_globals(
    cli: &Cli,
    max_wait: Option<std::time::Duration>,
) -> anyhow::Result<Vec<std::ffi::OsString>> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if let Some(account) = &cli.account {
        args.push("--account".into());
        args.push(account.into());
    }
    if cli.no_keyring {
        args.push("--no-keyring".into());
    }
    if cli.strict_roots {
        args.push("--strict-roots".into());
        for root in &cli.tls_extra_root {
            args.push("--tls-extra-root".into());
            args.push(std::path::absolute(root)?.into());
        }
    }
    // Whole seconds, which is all `--max-wait` can be written in. **Zero
    // stays zero**: it means "never wait for the budget", and a floor of one
    // second here would hand every child a wait the person did not allow.
    if let Some(max_wait) = max_wait {
        args.push("--max-wait".into());
        args.push(format!("{}s", max_wait.as_secs()).into());
    }
    #[cfg(feature = "testing")]
    if let Some(root) = &cli.sandbox_root {
        args.push("--sandbox-root".into());
        args.push(std::path::absolute(root)?.into());
        if let Some(base) = &cli.ig_base_url {
            args.push("--ig-base-url".into());
            args.push(base.as_str().into());
        }
        if cli.through_the_browser {
            args.push("--through-the-browser".into());
        }
    }
    Ok(args)
}

/// Gives the cursor back if the process dies with a menu on screen.
fn restore_terminal_on_panic() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        snob_cli::ui::restore_terminal();
        previous(info);
    }));
}

/// What is logged, and at which level.
fn log_filter(
    verbose: bool,
    spec: Option<&str>,
    owner: bool,
) -> tracing_subscriber::filter::Targets {
    use tracing::Level;
    use tracing_subscriber::filter::Targets;

    if verbose {
        return Targets::new()
            .with_target("snob", Level::DEBUG)
            .with_target("snob_ig", Level::DEBUG)
            .with_target("snob_core", Level::DEBUG)
            .with_target("snob_store", Level::DEBUG)
            .with_target("snob_cli", Level::DEBUG)
            .with_target("snob_engine", Level::DEBUG);
    }
    let quiet = || {
        let quiet = Targets::new().with_default(Level::WARN);
        if owner {
            quiet.with_target("snob_engine::owner::server", Level::INFO)
        } else {
            quiet
        }
    };
    match spec {
        Some(spec) => with_moved_targets(spec)
            .parse::<Targets>()
            .unwrap_or_else(|e| {
                eprintln!("warning: SNOB_LOG was ignored: {e}");
                quiet()
            }),
        None => quiet(),
    }
}

/// A `SNOB_LOG` spec with, ahead of every pair that names `snob_cli`, the same
/// level for where that code lives now: most of what was `snob_cli::<module>`
/// is `snob_engine::<module>`, and `snob_cli::commands::mcp` is `snob_mcp`, so
/// a spec written before the crate was split would select nothing.
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

    let mut pairs = Vec::new();
    for pair in spec.split(',') {
        let pair = pair.trim();
        let (target, level) = pair.split_at(pair.find('=').unwrap_or(pair.len()));
        let Some(module) = under(target, "snob_cli") else {
            continue;
        };
        match under(module, "::commands::mcp") {
            Some(rest) => pairs.push(format!("snob_mcp{rest}{level}")),
            None => {
                pairs.push(format!("snob_engine{module}{level}"));
                if module.is_empty() || module.starts_with('[') {
                    pairs.push(format!("snob_mcp{module}{level}"));
                }
            }
        }
    }
    pairs.push(spec.to_string());
    pairs.join(",")
}

/// A command's lines are read as it runs; the owner's, in its log, later, so
/// they say when.
fn init_tracing(verbose: bool, owner: bool) {
    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::util::SubscriberInitExt as _;

    let spec = std::env::var("SNOB_LOG").ok();
    let filter = log_filter(verbose, spec.as_deref(), owner);
    let lines = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        // **The filter decides, and nothing before it.**
        .with_max_level(tracing::Level::TRACE);
    if owner || verbose {
        lines.finish().with(filter).init();
    } else {
        lines.without_time().finish().with(filter).init();
    }
}

#[cfg(test)]
mod filter_tests {
    use super::log_filter;
    use tracing::Level;

    /// The two settings a person reaches for, and the one they mistype.
    #[test]
    fn the_filter_reads_what_was_asked_for() {
        let verbose = log_filter(true, None, false);
        assert!(verbose.would_enable("snob_ig::client", &Level::DEBUG));
        assert!(!verbose.would_enable("hyper_util::client", &Level::DEBUG));
        assert!(!verbose.would_enable("snob_ig::client", &Level::TRACE));

        let quiet = log_filter(false, None, false);
        assert!(quiet.would_enable("snob_ig::client", &Level::WARN));
        assert!(!quiet.would_enable("snob_ig::client", &Level::INFO));
        assert!(!quiet.would_enable("snob_engine::owner::server", &Level::INFO));

        // The owner says when it starts, and otherwise no more than a command.
        let owner = log_filter(false, None, true);
        assert!(owner.would_enable("snob_engine::owner::server", &Level::INFO));
        assert!(!owner.would_enable("snob_engine::owner::server", &Level::DEBUG));
        assert!(!owner.would_enable("snob_engine::headless", &Level::INFO));

        let chosen = log_filter(false, Some("snob_store=debug,warn"), false);
        assert!(chosen.would_enable("snob_store::secrets", &Level::DEBUG));
        assert!(!chosen.would_enable("snob_ig::client", &Level::DEBUG));
        assert!(chosen.would_enable("snob_ig::client", &Level::WARN));

        // A spec that does not parse falls back to the quiet default rather
        // than to nothing at all, so a typo does not also silence warnings.
        let typo = log_filter(false, Some("snob_ig=loud"), false);
        assert!(typo.would_enable("snob_ig::client", &Level::WARN));
        assert!(!typo.would_enable("snob_ig::client", &Level::INFO));
    }

    /// A spec saved when every module was `snob_cli`'s still selects them
    /// where they are now, in the engine and in the server.
    #[test]
    fn a_spec_that_names_a_module_by_its_old_crate_still_selects_it() {
        let old = log_filter(
            false,
            Some("snob_cli::headless=debug,snob_cli::commands::mcp=trace,warn"),
            false,
        );
        assert!(old.would_enable("snob_engine::headless::guard", &Level::DEBUG));
        assert!(!old.would_enable("snob_engine::headless::guard", &Level::TRACE));
        assert!(!old.would_enable("snob_engine::owner::server", &Level::DEBUG));
        assert!(old.would_enable("snob_mcp::child", &Level::TRACE));
        assert!(old.would_enable("snob_mcp", &Level::TRACE));
        assert!(!old.would_enable("snob_engine::cdp", &Level::INFO));
        assert!(old.would_enable("snob_ig::client", &Level::WARN));

        // The whole crate, as it was: the commands, the engine and the server.
        let whole = log_filter(false, Some("snob_cli=debug"), false);
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
        let stayed = log_filter(false, Some("snob_cli::jobs=debug,snob_client=debug"), false);
        assert!(stayed.would_enable("snob_cli::jobs::start", &Level::DEBUG));
        assert!(stayed.would_enable("snob_engine::jobs::supervise", &Level::DEBUG));
        assert!(!stayed.would_enable("snob_engine::owner", &Level::DEBUG));

        // What the person wrote for the new name is not overridden by what
        // the old one implies, in either order.
        for spec in [
            "snob_cli=trace,snob_engine=warn",
            "snob_engine=warn,snob_cli=trace",
        ] {
            let both = log_filter(false, Some(spec), false);
            assert!(both.would_enable("snob_cli::commands::lists", &Level::TRACE));
            assert!(
                !both.would_enable("snob_engine::owner", &Level::INFO),
                "{spec}"
            );
            assert!(both.would_enable("snob_mcp::http", &Level::TRACE), "{spec}");
        }

        // A spec that does not parse is still said and still quiet.
        assert_eq!(
            super::with_moved_targets("snob_cli::owner=loud"),
            "snob_engine::owner=loud,snob_cli::owner=loud"
        );
    }
}

#[cfg(all(test, feature = "mcp"))]
mod mcp_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn a_max_wait_of_zero_reaches_the_children_as_zero() {
        let cli = Cli::try_parse_from(["snob", "--max-wait", "0", "mcp"]).expect("it parses");
        // Printed by `snob mcp config` in front of `mcp`, as it was given.
        let globals = mcp_globals(&cli, cli.max_wait).expect("no path to resolve");
        let at = globals
            .iter()
            .position(|arg| arg == "--max-wait")
            .expect("the limit is passed on");
        assert_eq!(globals[at + 1], "0s");
        // And handed by the server to every child, a job's walk included.
        let waits = commands::mcp::Waits::given(cli.max_wait);
        assert_eq!(waits.call, std::time::Duration::ZERO);
        assert_eq!(waits.job, std::time::Duration::ZERO);

        // None given: none in the server's globals, and its own defaults.
        let cli = Cli::try_parse_from(["snob", "mcp"]).expect("it parses");
        let globals = mcp_globals(&cli, None).expect("no path to resolve");
        assert!(
            !globals.iter().any(|arg| arg == "--max-wait"),
            "{globals:?}"
        );
        let waits = commands::mcp::Waits::given(cli.max_wait);
        assert_eq!(waits.call, commands::mcp::MAX_WAIT);
        assert_eq!(waits.job, commands::mcp::JOB_MAX_WAIT);
    }
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use clap::Parser;

    /// The one property the whole sandbox rests on.
    ///
    /// Not "it returns something": it returns something that is **not** the
    /// name the real entries are under. `SecretStore` reaches the keyring on
    /// every backend, so this string is the only thing standing between a
    /// sandbox `login` and the developer's own session.
    #[test]
    fn a_sandbox_never_shares_the_real_keyring_service() {
        let cli = Cli::try_parse_from(["snob", "--sandbox-root", "/tmp/one", "whoami"])
            .expect("the flag parses");
        let (_, prefer_file, namespace) = wiring(&cli).expect("a sandbox needs no discovery");

        assert!(prefer_file, "a sandbox stores its session in a file");
        let namespace = namespace.expect("a sandbox is given a keyring namespace of its own");
        assert_ne!(
            namespace, "snob-ig",
            "a sandbox on the real service deletes the real session"
        );
        assert!(namespace.starts_with("snob-ig-sandbox-"), "{namespace}");
    }

    /// One process writes the entry and another reads it back, so a name that
    /// changed between invocations would lose the session every time.
    #[test]
    fn the_namespace_is_the_same_answer_twice_and_differs_per_root() {
        let one = sandbox_keyring_namespace(std::path::Path::new("/tmp/one"));
        let again = sandbox_keyring_namespace(std::path::Path::new("/tmp/one"));
        let other = sandbox_keyring_namespace(std::path::Path::new("/tmp/two"));

        assert_eq!(one, again, "the same root has to name the same entries");
        assert_ne!(
            one, other,
            "two sandboxes at once must not be able to delete each other's entries"
        );
    }

    /// Without the flag there is no namespace, so a real run is untouched by
    /// any of this.
    #[test]
    fn an_ordinary_run_is_left_on_the_real_service() {
        let cli = Cli::try_parse_from(["snob", "whoami"]).expect("it parses");
        let (_, _, namespace) = wiring(&cli).expect("discovery works on a test machine");
        assert_eq!(namespace, None);
    }
    /// A file that holds no certificate is refused, and that is the case a bare
    /// error check misses.
    ///
    /// `from_pem_bundle` looks for BEGIN/END blocks and answers `Ok` with an
    /// empty list when it finds none, so plain text, a DER file, or a path that
    /// happened to exist all passed. The narrowing would then hand
    /// `tls_certs_only` Mozilla's roots and none of the user's -- the one
    /// outcome the flag exists to prevent -- and it would fail at the handshake
    /// against Instagram, hours later, and nowhere else.
    ///
    /// Checked against a real certificate rather than only against rejections,
    /// because a predicate that refuses everything also passes the first half.
    #[test]
    fn an_extra_root_has_to_be_a_certificate() {
        assert!(
            holds_a_certificate(
                b"not a certificate
"
            )
            .is_err()
        );
        assert!(holds_a_certificate(b"").is_err());
        assert!(
            holds_a_certificate(
                b"-----BEGIN CERTIFICATE-----
not base64
-----END CERTIFICATE-----
"
            )
            .is_err(),
            "a block that is not a certificate is not one"
        );

        // One of Mozilla's own, so the accepting half is exercised too.
        let real = snob_ig::http::reqwest::Certificate::from_der(
            &webpki_root_certs::TLS_SERVER_ROOT_CERTS[0],
        );
        assert!(real.is_ok(), "the bundled roots are certificates");
    }
}
