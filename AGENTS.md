# AGENTS.md

Rules for anyone, human or agent, changing this repository. `CLAUDE.md` points
here. Code comments cite this file by section name ("Known walls", "Costs worth
knowing", "Stories are handed to the system viewer", the write regime, ...), so
**rename a section only together with the comments that cite it**
(`grep -rn AGENTS.md crates`).

This file is the map and the rules that span files. What a module does, and
why, is in that module: **read a file's `//!` header before changing it.**

## What snob is

**snob is a desktop app**: a native Instagram client, installed with its
tools beside it in one package (the desktop edition) — the command line, the
MCP server for AI agents, the automations (the monitor, `snob watch`) and
webhooks. Servers and containers take the tools alone (the cli edition,
"Release and packaging").

| Part | Where | Binary |
|---|---|---|
| The app | `apps/desktop`, a GPUI window on the engine; not released yet | `snob-desktop` |
| The command line and the MCP server | `crates/snob-cli` (the crate keeps that name), `crates/snob-mcp` | `snob` |
| What both run on | `crates/snob-engine` and below | |

The version is in `[workspace.package]` of `Cargo.toml`; the app's is
`0.0.0` until it ships, then the same.

The command line walks followers and following and crosses them
(`unfollowers`, `fans`, `friends`, `scan`); shows profiles, stories, highlights,
posts and reels and downloads their media; reads Instagram's data export
(`import dyi`) and its own stored lists (`snapshots`, `changes`) without sending
anything; monitors accounts (`watch`, with webhooks); serves agents (`snob
mcp`); and updates itself (`snob update`). The app and the command line both
sit on `snob-engine`.

It signs in as the user, speaks Instagram's **web** API (`www.instagram.com`,
never the mobile API), and by default sends every request from a headless
Chromium-based browser installed on the machine, on a profile of its own.

## Hard lines

Each is enforced by code or a test, never by convention alone.

| Rule | Enforced by |
|---|---|
| **Two writes, ever: follow and unfollow.** No block, remove-follower, like, comment, message, share. | `snob_ig::graphql::Mutation` has two variants and no wildcard arm in its matches, so a third is a compile error; a test in `graphql.rs` pins `Mutation::ALL`. `crates/snob-ig/src/client/write.rs` is the only file that writes; `IgClient::post` takes a `Mutation`, never a path. |
| **Nothing ever marks anything as seen**: stories, reels, posts, the activity feed, threads, search history, time spent, view counts. | `crates/snob-core/tests/no_seen.rs` fails on any known spelling of such a call in any `.rs` file (it alone may name them); `snob_ig::allowlist` refuses every call it does not name. |
| **The user's own browser is never touched**, its cookie store included. | Login is a window on snob's own profile, a pasted `sessionid`, or `login --remote`, which relays snob's browser; the password reaches snob only as keystrokes relayed there, kept nowhere. |
| **A human pace, and a hard stop on the first push-back.** Slower is always acceptable; faster is argued for in the code, next to the number. | "Talking to Instagram". |
| **Consent before enumerating someone else's lists**, asked before the account is resolved. | `engine::list`; `crates/snob-cli/tests/consent.rs`. |

The no-writes claim is stated in four places that move together: the `.deb`
`extended-description` in `crates/snob-cli/Cargo.toml`, the winget locale
manifest `packaging/render.sh` renders from it, `crates/snob-ig/src/lib.rs`, and
`README.md`.

## Workspace map

Rust 2024, resolver 3. Dependencies point one way:
`snob-cli -> snob-mcp -> snob-engine -> snob-store, snob-ig -> snob-core`;
`snob-ig` never depends on `snob-store`, and `snob-engine` on no terminal crate
(`crates/snob-core/tests/layers.rs` reads the manifests).

| Crate | Holds | Must not |
|---|---|---|
| `snob-core` | The domain: `Pk`, the models (the JSON contract), set arithmetic, an automation (`automation.rs`), the monitor's diff and schedule, webhook signing and the one constant-time comparison (`watch::sign::same`), `Secret`, `Session`, the `RateBudget` trait and cooldown policy, `clock`. | Do I/O: no file, database, network or directory (`clock` excepted). |
| `snob-ig` | The Instagram web client: `client/`, `web.rs`, `allowlist.rs`, `pager.rs`, `pace.rs`, `graphql.rs`, `error.rs` (classifying answers), `http.rs` (the only place `reqwest` is configured). | Touch SQLite or the keyring, or drive a browser (it only defines the page's request shapes). |
| `snob-store` | SQLite (`store/`, migrations in `store/sql/`), paths, the secret store, `accounts.toml`, `watch.toml`, the automations' files (`automations.rs`), the layout's migration, the views' cache (`cache.rs`) and what files weigh (`disk.rs`). | Hold domain logic. |
| `snob-engine` | What a desktop app links in process: `app.rs` (`App`), `engine/` (cache, consent, cooldown, walks, estimates, monitor), the browser (`headless/`, `cdp/`, `pipe.rs`, `owner/`), jobs' state and supervisor (`jobs/`), signing in (`login.rs`, which hands back a `Stored` and tells a failure apart with `failure_of`) and out (`logout.rs`), the account's card (`viewer.rs`), what a view shows at once (`cache.rs`) and what is kept on disk (`storage.rs`), `relay/`, `update/`, the automations every front reads, keeps and runs (`automations/`), an account's page and its picture as every front reads them (`profile.rs`), reading and saving posts and stories (`posts.rs`, `media.rs`, `files.rs`), `ffmpeg.rs`, `watch/webhook.rs`, `exit.rs`, `log.rs` (the filter every program logs through, and an app's log in files), and `sentences.rs` (the plain sentences of its refusals). `host.rs` is what it is handed: a `Progress` and an `Ask` per run (`host::Front`, given to `App::open`), and a sink for notices. | Depend on `clap`, `console`, `indicatif`, `ratatui`, `crossterm`, `dialoguer`, or on `snob-mcp`/`snob-cli`; ask anybody anything itself; decide how anything looks. |
| `snob-mcp` | `snob mcp`: the server over stdio and, with its `http` feature, over HTTP (`http.rs`); its key (`key.rs`), `snob mcp config` (`config.rs`), its arguments (`args.rs`). | Build an Instagram client or open a database; depend on `snob-cli`, its tests included (the lines it builds are parsed in `crates/snob-cli/tests/mcp_lines.rs`). |
| `snob-cli` | The `snob` binary and its library: `cli.rs`, `main.rs`, `commands/`, `output/`, `report.rs`, `progress.rs` (the bars), `front.rs` (what it hands the engine), `ui/`, and the clap-tied half of jobs (`jobs/argv.rs`, `jobs/start.rs`). It names what it uses of the engine and of the server by their own crates (`snob_engine::`, `snob_mcp::`). | |
| `snob-fake-ig` | The fake Instagram the tests talk to (`world.rs`) and a tab on it with no browser (`tab.rs`). Never published. | Depend on anything above `snob-ig`; be named outside `[dev-dependencies]` by a crate that ships (`layers.rs`). |

Inside `snob-cli` and `snob-engine`:

- **A command reaches Instagram and the account's database through an `App`**
  (`App::open`, with the front `front::front` makes), never by building a
  client or opening a store itself. The exceptions say so in their header:
  `status`, `--dry-run`, `fetch`, `update`, `whoami`, `snapshots`, `changes`.
- **`engine` returns data and where it came from, never how it looks**; the
  sentences it returns are `sentences.rs`, plain text, and the commands'
  wording and printing live in `commands` and `report.rs`. Every list comes
  out of `engine::list`, its checks in a fixed order (cooldown, `--offline`,
  consent, cooldown again, counter poll, freshness, walk); it knows nothing of
  delivery.
- **The engine is handed what it needs from the command line**, never reaches
  for it: who watches a walk and who answers the consent question
  (`host::Front`), where a notice goes (`host::tell_notices_to`), what to undo
  before a forced exit (`interrupt::before_a_forced_exit`), how a job's stored
  command line reads (`jobs::Check`, the command line's `jobs::argv::walk_of`),
  and the binary to start for its own processes (`exe::start_as`, this binary
  unless told).
- `commands/watch/say.rs` is prose (rewording is free); `wire.rs` is protocol
  (`SENT_SCHEMA = 1`, `PRINTED_SCHEMA = 2`): an edit there is an integration
  change.

## Build, lint, test: what CI runs

Toolchain pinned in `rust-toolchain.toml` (`1.100.0`); bump it together with
`tools/ci/Dockerfile`. `rust-version = "1.95"` is the floor for `cargo install
--git`. Everything that resolves dependencies takes `--locked`.

`.github/workflows/ci.yml` (pushes to `main` and `dev`, every pull request):

```bash
# lint (ubuntu-latest)
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy -p snob-cli --all-targets --no-default-features --locked -- -D warnings
bash packaging/render.sh <version> SHA256SUMS   # then git diff --exit-code on Formula, bucket, packaging/winget

# test: ubuntu-latest, ubuntu-24.04-arm, windows-latest, windows-11-arm, macos-latest
# Windows and macOS (Linux never compiles their cfg code):
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
SNOB_TEST_REQUIRE_BROWSER=1 cargo nextest run --profile ci --workspace --locked
SNOB_TEST_REQUIRE_BROWSER=1 cargo nextest run --profile ci --workspace --all-features --locked
# Linux, against musl, in a session bus with gnome-keyring unlocked:
dbus-run-session -- bash -c '
  set -euo pipefail
  echo "" | gnome-keyring-daemon --unlock --components=secrets
  cargo nextest run --profile ci --workspace --locked --target "$(uname -m)-unknown-linux-musl"
  cargo nextest run --profile ci --workspace --all-features --locked --target "$(uname -m)-unknown-linux-musl"'
```

- The render check is skipped while the version is unreleased; **never
  hand-edit `Formula/`, `bucket/` or `packaging/winget/`**.
- Job **build** (pushes only): `cargo build --release --locked --target
  <triple> -p snob-cli` for the five targets, `--version`, a static check of the
  musl binaries, and the `.deb` with `cargo-deb@3.8.0`.
- `supply-chain.yml` (pushes, pull requests, Mondays): `cargo deny check
  advisories sources licenses`, never a bare `cargo deny check` (`bans` is off
  on purpose). Every `ignore` in `deny.toml` says why and when it goes.
- `weekly.yml` (Mondays, manual): `feature-powerset` (`cargo hack check
  --workspace --feature-powerset --no-dev-deps` on Linux and Windows, without
  `--locked`, which refuses the lockfile `--no-dev-deps` prunes),
  `fuzz` (`cargo-fuzz` in `fuzz/` on Linux under pinned nightly), `mutants`
  (`cargo-mutants` with `nextest` on `snob-core` and `snob-ig`, report-only), and
  `coverage` (`cargo-llvm-cov` with `nextest` over `--workspace --all-features
  --locked` on Windows).
- Locally on Windows the lines above run as written; they skip `cfg(unix)`
  code. `bash tools/ci/run.sh` runs the Linux job in Docker; `tools/ci/ci.sh`
  stays in step with `ci.yml`.
- The test lines need `cargo-nextest` (`cargo install --locked
  cargo-nextest@0.9.150`, the version CI pins). Of each pair, the first runs
  snob as it ships and the second adds the `testing` seam and the binary-level
  tests. `cargo xtask affected -- --profile agent` is the quick local loop, and the
  full default profile runs before a pull request (CI keeps running everything).
- **Never export `RUSTFLAGS`**: it replaces `.cargo/config.toml` instead of
  merging, silently dropping `+crt-static` and `/Brepro` on Windows. Add a flag
  to a table there, or to a second config file as `release.yml` does.

## Tests

- Unit tests in `#[cfg(test)] mod tests` at the bottom of the file; integration
  tests in `crates/<crate>/tests/`, fixtures in `tests/common/`. **A test name
  is a sentence stating the property**
  (`only_a_walk_against_a_test_server_skips_the_waits`).
- **No test reaches Instagram; never walk a real account's lists without the
  limiter.** HTTP is mocked with `wiremock` or served by the fake Instagram in
  `crates/snob-fake-ig` (`world.rs`). `IgClient::is_live` decides by address:
  waits are off against a local server and on against instagram.com, and no
  caller can choose otherwise.
- **No test touches the real keyring** (service `snob-ig`): every
  `SecretStore::new(` in test code has `.with_service(` in the same statement
  (`crates/snob-core/tests/keyring.rs` reads the source). Binary-level tests use
  `--sandbox-root`: the file backend, a keyring namespace of its own, and every
  file the run touches under one directory.
- **The `testing` feature** (`snob-cli/testing` -> `snob-engine/testing` ->
  `snob-ig/testing`) is the
  only seam that points snob elsewhere (`--sandbox-root`, `--ig-base-url`,
  `--through-the-browser`, `point_every_client_at`, `SANDBOX_BASE`,
  `with_page`, and `snob_engine::sandbox`, which confines whichever program
  runs the engine to one directory). A release binary has none of it:
  `crates/snob-core/tests/sandbox.rs` fails when a seam name appears without
  `#[cfg(feature = "testing")]` directly above it. `--workspace` alone runs no
  binary-level test; never narrow the CI line with `--test`, which skips the
  bin's own unit tests.
- **A test that starts a job stops it and waits for its lock before its
  `TempDir` drops**, a panic included.
- **Browser tests** skip when no browser starts, fail under
  `SNOB_TEST_REQUIRE_BROWSER=1`, and never run as root. Under nextest each
  test is a process of its own, so they run in parallel (measured on 16 and 4
  cores) and the `ci` profile retries them; `.config/nextest.toml` serializes
  the `keyring` group. Under `cargo test`, give them `--test-threads=1`.
- **A test listens on loopback only**: Windows Firewall asks about any
  program listening elsewhere, and a test binary is a new program every
  build. The one test that must hold a wildcard (`snob-mcp`'s `http.rs`)
  runs on CI or with `SNOB_TEST_WILDCARD=1`.
- **Source-reading guards** in `crates/snob-core/tests/`: `no_seen.rs`,
  `keyring.rs`, `sandbox.rs`, `nextest.rs`, `layers.rs` (which crate may depend
  on which, from the manifests), and `language.rs`, which scans
  `rs sql md toml yml yaml sh ps1 rb json` (this file too) for Spanish (escape
  `i18n-allow`), British spelling (`spelling-allow`), runs of spaces in a
  sentence in `.rs` (`layout-allow`), and a `snob ...` command to type back
  containing an at sign (no escape). `.agents/` and `pnpm-lock.yaml` are
  skipped.
- **Property tests** (`proptest`, in each parser's `mod tests`): a parser fed
  by a person, a browser or Instagram is checked never to panic on any input
  (the release profile aborts), with the rules its documentation states. A new
  parser of outside input gets one.
- **Contract snapshots** (`insta`, `.snap` files in `snapshots/` beside the
  test): one fully populated value of each main JSON shape a person, a
  script, an agent or a webhook reads: an account, a post, its comments,
  replies and likers, a grid, stories, a profile, `status`, the webhook's
  payload, an MCP list page and the error object. Not pinned yet: `snapshots`,
  `changes`, `account list`, `watch status`, `highlights`, `update`, the
  monitor's printed report (`PRINTED_SCHEMA`) and `status`'s `update` key. A changed `.snap` is a
  contract change ("Output contract"): regenerate with `INSTA_UPDATE=always`
  only on purpose, and read its diff. CI fails on a mismatch.
- **Weekly deep checks** (`.github/workflows/weekly.yml`): `fuzz/` holds the
  standalone `cargo-fuzz` workspace for outside-input parsers, built without
  `testing` under a pinned nightly (`cargo +nightly-2026-11-01 fuzz run
  <target>` on Linux/macOS); `cargo mutants --test-tool=nextest -p snob-core
  -p snob-ig` measures suite sensitivity in the pure crates (report-only, so
  surviving equivalents do not fail CI); `cargo llvm-cov nextest --workspace
  --all-features --locked` measures coverage locally or exports LCOV.
- Migrations: the `validate()` tests run the whole chain of both databases.
- Hand-run: `cargo test -p snob-cli --features testing --test headless
  serve_the_fake_world -- --ignored --nocapture` serves the fake Instagram for
  `SNOB_TEST_SERVE_SECS` (`serve_the_website_world` is what
  `web/video/record.sh` records); `cargo test -p snob-fake-ig --test drift --
  --ignored --nocapture` compares `World` answers against `SNOB_CAPTURES` (or
  `tools/capture/out`); `tools/parity/tui.sh` diffs two builds' views in tmux.

## Talking to Instagram

Every number is in code with its source beside it; change one only with a
documented reason in the same place.

### Transport

- **By default every request leaves from a tab on instagram.com** in the
  headless browser, sent with `fetch()`: handshake, HTTP/2, cookies, client
  hints and `Referer` are the browser's (`headless/`, `snob_ig::client::page`).
  `SNOB_NO_BROWSER=1` sends with `reqwest`, and Instagram can tell.
- **The engine hands itself its browsers** (`owner::ensure`, at `App::open`
  and a login's check), so a program running it, the desktop app included,
  has nothing to remember; **a client pointed at Instagram without a page
  refuses to send** unless `SNOB_NO_BROWSER` chose that (`IgError::NoBrowser`,
  `transport.rs`).
- One browser per account, run by the owner process (`owner/`, hidden
  `__browser-owner`), closed `headless::IDLE` (5 min) after its last request;
  `SNOB_NO_OWNER=1` makes a command run its own. Nothing that spends no network
  starts a browser.
- REST reads take one path on both transports, `IgClient::rest_read`
  (`client/ask.rs`).
- The API client follows no redirect; the transport follows them itself, pays
  for every hop, and never leaves instagram.com.

### Allowlist

- **Every call snob may send is a value in `snob_ig::allowlist`**; call sites
  pass those values, never a path or a name.
- **The account's tab never leaves the site**: a request or a call naming
  another origin is refused before any browser starts (`headless/mod.rs`,
  against `snob_ig::client::site`).
- **The tab's guard** (`headless::guard`) pauses every request the page makes
  and every document: GETs go on, a POST only if `allowlist::refused` lets it;
  the view count, the event log and the Facebook sync always fail, and so does
  a request whose address cannot be read.
- **A follow or unfollow goes out only as snob's own write**: armed just before
  it is fetched (`guard::Armed`), spent by the first POST matching it exactly;
  every other write, the app's under any `doc_id`, fails
  (`allowlist::refused_app`).
- A read's `doc_id` is pinned and follows the app when it moves
  (`allowlist::vouched`); a write's starts from a captured seed
  (`Mutation::seed_doc_id`) and is looked for in the page's JavaScript when
  refused.
- Cursors and pages go out only in the shapes Instagram hands out (comments,
  a profile's tabs, threads of replies: `allowlist.rs` has each).

### Headers

Every value is the page's, or computed from the User-Agent of the browser
actually installed; what cannot be computed is left out (`client_hints.rs`,
`client/headers.rs`). An absent header is honest, an invented one is not. TLS
handshake, HTTP/2 settings and header order are never imitated.

### Pace

| What | Value | Where |
|---|---|---|
| Accounts asked for per list page | 12 (the app's) | `pace::ACCOUNTS_PER_PAGE` |
| Between pages of one action | 1 to 3 s | `STEP_MS` |
| Between pages of comments, and threads of replies | 2 to 5 s | `COMMENT_STEP_MS` |
| Before a list | 1.5 to 4 s | `DWELL_MS` |
| A sitting, and the rest after it | 40 pages; 5 to 15 min | `PAGES_PER_SITTING`, `SITTING_PAUSE_MS` |
| Most pages of one walk | 2,000 | `pager::HARD_PAGE_CAP` |
| Comment pages and reply threads of one post read | 20 and 10 | `posts::MOST_COMMENT_PAGES`, `MOST_THREADS` |
| Slow answers | a wait lasts at least twice the median of the latest 5 healthy answers of 10 min (3 needed), at most 60 s | `SLOW_*`, `budget::SlowRule` |

- **The sitting outlives the process** (table `sitting`, `budget::Sitting`): the
  page that fills one draws the rest, and the next page any walk of the account
  reads, in any process, waits it out. Grid, tab, comment and reply pages count
  in it (`IgClient::count_in_the_sitting`).
- **Slow answers lengthen waits, never shorten them** (`Pacer::slowed`). Load
  headers are recorded in `answers` and never acted on.
- `--max-wait` refuses a wait for the budget before anything is charged; it
  never cuts a sitting's rest.

The persistent budget under the pace (GCRA, `store/rate_budget.rs`):

| Bucket | Allows | Where |
|---|---|---|
| Pace | one request per 3.83 s sustained, a burst of 20 | `PACE_EMISSION_MS`, `PACE_BURST_MS` |
| Daily | about 2,000 requests | `DAILY_EMISSION_MS` |
| Writes, on top of both | 3 in a row, then one per 15 min; 99 in 24 h | `WRITE_EMISSION_MS`, `WRITES_IN_A_DAY` |
| Accounts read off lists | 2,000 in any 24 h; 1,000 for a week after a push-back | `budget::accounts_per_day` |

`--same-day` reads past the day's accounts on purpose; agents never get it.

### Push-back and retries

**The first push-back stops the run and starts a cooldown**, during which
nothing is sent, not even the counter poll; stored lists are served with a
dated warning.

| Answer | Cooldown |
|---|---|
| Throttle (`RateLimited`) | 2 h, `RATE_LIMIT_COOLDOWN` |
| Action block (`FeedbackRequired`) | 12 h, `ACTION_BLOCK_COOLDOWN` |
| Challenge or checkpoint | 30 min, `CHALLENGE_COOLDOWN` |
| Push-backs on two different accounts less than an hour apart | every account, until the later cooldown ends (`budget::common_brake`, `shared.db`) |

The durations are pinned by `the_cooldowns_are_the_documented_ones`. The app's
own calls in the tab are listened to for push-back too (`headless/listen.rs`).

**Retries.** `IgError::reaction` is the single authority; never write a retry
loop of your own against Instagram.

- A list page that failed on the network or drew a 5xx is retried by
  `pager::fetch_page`, `Pace::network_retries` times (3; 2 for another
  account's lists) after a doubling backoff (from 2 s; 3 s).
- A walk that finds the machine slept reloads and waits for the network
  (`RECONNECT_WAITS_SECS`).
- A write refused in a way that says it did not happen is sent once more under a
  rediscovered `doc_id`; one that may have happened (5xx, redirect, unreadable
  200) never is.
- **A push-back, a challenge, a dead session: never.**

`SNOB_IGNORE_COOLDOWN` exists so a bug of ours cannot lock anyone out; it stays
out of the help and out of an MCP child's environment.

### Consent

- **Walking another account's lists asks first, before resolving it.** No
  terminal and no `-y` is exit 130, `consent_required`, nothing spent; a no is
  `declined`.
- `likers` asks too, for every post, the viewer's own included. Its answer
  (about a hundred accounts) is charged to the day's accounts, and a day with
  fewer than `likers::LIKERS_AT_MOST` (150) left is refused before asking.
- `stories`, `highlights`, `posts`, `post` and `profile` do not ask: consent
  covers somebody's followers and following, and `profile` walks only the
  viewer's own following.
- `watch once` has no `-y`: the monitor's consent is `watch.toml`. An entry with
  `lists = false` reads posts only and needs none. An automation's is the yes
  stored with its action (`Account::consent`), and one that walks another
  account's lists without it is not finished (`Problem::NoConsent`).

### The write regime

`commands/follow.rs`, `client/write.rs`: one account per invocation and no flag
that batches; asked before the request (`-y` answers in advance); paid from the
write bucket on top of the read buckets; refused without a CSRF token; sends
`Origin`; follows no redirect; and **once in flight it is not abandoned on
Ctrl+C**, the one exception to "Ctrl+C cancels at once".

### Identity is the pk

- **An account is its numeric `pk`, never its username.** `snob_core::Pk` has
  no `Deref`, no arithmetic and no `ToSql`/`FromSql`; SQLite goes through
  `store::pk_to_sql`/`pk_from_sql`. Crossings are by pk; a test asserts
  unfollowers + friends = following and fans + friends = followers.
- A name that cannot be a username (`model::possible_username`: 1 to 30 of
  `[A-Za-z0-9._]`, one leading at sign dropped, never `.` or `..`) is refused by
  clap, exit 2, before anything is asked or spent.

### Stored lists

- A list's counter is polled before any walk (`engine::freshness`); nothing
  moved, nothing walked.
- Each walked page commits in its own transaction: stopping is saving.
- **The account's own writes reach its stored following list**
  (`store::own_writes`, `engine::serve_stored`), said as such, never as a walk.
- **Old captures are swept by one rule** (`store::snapshots::prune`), after a
  walk that finished whole, the monitor's or not: whole captures older than 30
  days go, except the one each `watch_marks` row points at, each list's two
  newest whole captures (the pair `snob changes` compares by default), and every
  unfinished one.

### A run nobody is left to want stops

A process a program drives (a pipe or socket on standard input or output)
watches its parent and stops, as on `SIGTERM`, when it goes (`parent.rs`). Not
at a terminal, into a file, under `nohup` or `setsid`, under pid 1, or in the
processes snob detaches. `SNOB_NO_PARENT_WATCH` turns it off, out of the help.

### The monitor's posts

`engine::watch_posts`: an entry with `[account.posts]` has its profile and one
page of its grid read after its lists (up to `posts::UNTIL_PAGES`, 3, while
nothing seen before turns up), from the browser only. **At most once an hour per
account** (`READ_AT_MOST_EVERY`), nothing in a cooldown, no retry. Files go to
`save_to`, bounded per account and run (`commands/watch/posts.rs`: 1 GiB, no
file started after ten minutes).

## Secrets, network and local data

### Secrets

- **Sessions**: the system keyring (Credential Manager with
  `CRED_PERSIST_LOCAL_MACHINE`, Keychain, Secret Service), or with
  `--no-keyring` a `0600` file in a `0700` directory (DPAPI-sealed on Windows).
  One per account. A stored session that does not parse, or that holds what
  no login can have stored (`held_to_shape`), is reported naming the field and
  never quoting it (`secrets.rs`); `session.json` is read up to 64 KiB.
- **Credentials are `Secret`**: `Debug` prints `<hidden>`, zeroized on drop, read
  only through `expose()` (keep it greppable), `==` in constant time. Never log
  or print a token. The headers that carry one (`Cookie`, `X-CSRFToken`,
  `X-FB-LSD`, a webhook's stored `Authorization`) are marked sensitive.
- **The MCP key** (`snob-mcp`, `key.rs`): `snob_mcp_` and 43 base64url
  characters, nothing else accepted. Keyring only, whatever `--no-keyring`
  says; where there is none, `SNOB_MCP_KEY` in the server's environment, which
  wins wherever it is set. Clients read theirs from `SNOB_MCP_TOKEN`. Printed
  only to a terminal unless `--print`, otherwise named by a 12-digit
  fingerprint. Neither variable reaches a child (`jobs::NEVER_INHERITED`).
- The webhook's token and signing key live in the keyring, never in
  `watch.toml`, and stay `Secret` until the request is made
  (`webhook::Webhook`).

### Network clients

`reqwest` is configured only in `snob_ig::http`. **No cookie jar**: the
`cookies` feature is off workspace-wide, so `.cookie_store(true)` does not
compile; the session is an explicit header on the one client meant to carry it.

| Client | Rules |
|---|---|
| Instagram API | carries the session; redirects followed by the transport, inside instagram.com |
| CDN (`client/cdn.rs`, `http::plain`) | HTTPS, CDN hosts only, never instagram.com; no credential argument exists |
| Webhook (`watch/webhook.rs`, `http::plain`) | no redirect; `User-Agent: snob/<version>`; `http://` only to private networks; `X-Snob-Signature: sha256=<hex>` over the exact bytes; retried, since it is the user's server |
| Updater (`update/github.rs`, `http::plain_download`) | HTTPS to GitHub's release hosts only, every redirect held to them |

- A webhook body is `format` under `[webhook]` or `--webhook-format`
  (`wire::outgoing`): snob's JSON, or the summary alone for Slack, Discord, ntfy
  or text (Discord's with its link syntax escaped: the words are another
  person's). A queued row keeps its content type, so a retry goes out as
  queued.
- **TLS**: rustls (`prefer-post-quantum`) on four targets, schannel on Windows
  ARM64, where rustls does not build without LLVM. HTTP/2 is required (HTTP/1.1
  draws 429s). The platform's roots by default; `--strict-roots` narrows to
  Mozilla's (refused on Windows ARM64), `--tls-extra-root` adds PEM files;
  neither reaches the webhook. The provider under rustls is `aws-lc-rs`, which
  `prefer-post-quantum` needs; signing is RustCrypto's `hmac` and `sha2`.

### Local processes

- **The browser's DevTools protocol is on a pipe** (`pipe.rs`), never a port: a
  port hands the session to any local process.
- The owner's socket admits the same user only (`local_socket.rs`; Unix:
  `0600` and `peer_cred`; Windows: a named pipe with a DACL for this user),
  and a connection that does not say who it is within 10 s is let go
  (`owner/server.rs`).
- `login --remote`'s relay page gets JPEG frames and posts input, behind a
  one-visit token and a key in the URL fragment; the protocol never crosses the
  tunnel. It listens on a `0600` Unix socket, on a loopback port only where
  `ssh -L` cannot forward one (`relay::can_be_forwarded`).
- Rotated cookies are written back from the browser when a command leaves it
  (`headless/write_back.rs`); a login is authoritative.

### Local data

- **Per user, never per directory** (`snob-store/src/paths.rs`, app name
  `snob-ig`). On Windows the Local directory, not Roaming (WAL on a synced
  folder corrupts SQLite), with a user-only DACL written once a process per
  directory and read back after (`windows_acl`).
- Per account under `accounts/<pk>/`; `shared.db` for what accounts share.
  Kept a week: `answers` (`snob status --answers`, by call name, never path)
  and `mcp_acts` (`snob status --agents`). `watch_posts`, written only by
  `watch::commit_report`: thirty days, and each account's newest 36.
- Jobs under `accounts/<pk>/jobs/<id>/`, `0600` on Unix: a result kept a day,
  a job a week, at most 50.
- **Automations** (`snob_store::automations`): one JSON file each in the
  configuration directory's `automations/`, written whole under a lock and
  named by its id; their runs in `shared.db` (`automation_runs`, the newest
  100 each), what each action found last in the reading account's database
  (`automation_marks`). Removing one forgets its runs.
- **An app's log** (`logs/`, `snob_engine::log`): one JSON object a line,
  five files of 1 MiB, nothing older than fourteen days; a command still
  writes its lines to standard error. Every string of a line goes through
  `log::scrub`, the second net under `Secret`. A crash report stays there
  too, and is sent nowhere.
- **An account's card** (`accounts/<pk>/viewer.json`, `snob_store::viewer`):
  its name, bio and the address of its small picture, kept from what a
  login's check was answered (`login::validate_seeing`), and read again
  from the browser's tab after the app's login and its day's check of the
  session, which sends nothing (`snob_engine::viewer::look_again`). **Asked
  for once only: right after the app's login**, the account's own profile
  (`snob_engine::viewer::read_after_login`, one request), which fills the
  card and the cache with its counters and downloads its small and largest
  pictures, so the welcome, the menu and its own page draw at once. The
  picture (the cache's `pictures/<pk>.jpg`) is one download from the CDN
  when its address changes (`snob_engine::viewer::picture`). The app draws
  the kept one at once, and a new one comes in once read. Both go on a
  sign-out.
- **The views' cache** (`accounts/<pk>/cache/`, `snob_store::cache`,
  `snob_engine::cache`): what the account was last shown of each account,
  by pk (`profiles/<pk>.json`: names, bio, counts, how the two stand), and
  the small pictures it was shown (`pictures/<pk>.jpg`), and the other
  pictures a view drew (`cache::Drawing`: an account's largest, a
  highlight's cover, a post's thumbnail), so a view shows the
  last known values at once while its usual requests run. **It adds no
  request and takes none away**: filled only where an answer already
  arrives (`App::profile_named`, which a list's resolve, a profile read and
  the monitor read through; the counter poll; a picture a view downloads
  from the CDN, `Keeper::fetch_picture` and `fetch_drawing`),
  never read to decide whether to ask, so it has no refresh rule. **Kept by
  the account that saw it**, so what a private account showed one account
  is never another's; nothing is kept once the session is signed out, and
  the directory is never made where the account's is not. One sweep: 30
  days unrenewed, then the oldest past 32 MiB (`cache::KEPT_FOR`,
  `AT_MOST`). It goes whole on a sign-out and a `purge`.
- **What is kept, measured** (`snob_engine::storage::measure`): by kind
  (databases, cache, browser profiles, jobs, logs, update, scratch, other)
  and per account, **from metadata only**, no database opened.
  `storage::clear_cache` removes the cache and nothing else: never a
  session, a stored list or capture, `watch.toml`, `accounts.toml`, a job or
  its result, a log, a browser profile (each holds a session) or a
  downloaded update. The disk it is all on is asked its size and free room
  (`snob_store::disk::volume`).
- **Forgetting an account's history** (`snob_engine::storage::forget_history`)
  deletes from its database what it walked and what the monitor made of it
  (`snapshot_members`, `snapshots`, `watch_marks`, `watch_runs`,
  `watch_deliveries`, `watch_renames`, `watch_renames_sent`, `watch_posts`,
  `own_writes`, `automation_marks`, `username_history`, `accounts`, `users`),
  its jobs and their
  results and its cache, then `VACUUM`s. **It keeps everything a rule reads
  before sending**: `rate_budget`, `sitting`, `cooldowns`, `account_reads`,
  `answers`, `mcp_acts`, `watch_post_marks` (and the account rows it names),
  `watch_state` and `meta` (`snob_store::store::forget`, whose test fails on a
  table in neither list), and the session, the browser profile, the card,
  `accounts.toml`, `watch.toml` and `shared.db`. Refused while a job runs or
  its `spend.lock` or jobs lock is held; it holds both while it works.
- **One writer at a time**: `session.json` and `watch.toml` are replaced under a
  lock on `<temporary>.lock` (`paths::replace_private_alone`); `accounts.toml`
  changes only through `Registry::update`, under `accounts.toml.lock`.
  `watch.toml` and `accounts.toml` are written from a template, tested to
  round-trip.
- **`save_to` is written by the person only**, in `watch.toml`, never a flag;
  snob never writes over a file already there.
- **Reading for a report writes nothing of the account's**: `status`,
  `snapshots`, `changes` and `--dry-run` use `Store::read_existing` (no create,
  `query_only`, an older schema migrated in memory), which leaves no `-wal` or
  `-shm` file that was not there before; while another process has the
  database open they stay, and nothing is folded in.
- `snob purge` removes everything but the binary.

### Signing out

`logout`, `purge` and an agent's `snob_logout`:

1. Stop the account's jobs and **hold its jobs lock until the session is
   gone**.
2. **Sign it out for every other process** (`owner::sign_out`): its
   `signed-in` list is emptied, under `signed-in.lock`, and a killed save's
   `.session.json.tmp` removed. A walk, the monitor or an agent still holding
   the session is refused its next request with exit 3,
   `IgError::SignedOutHere` ("signed out on this computer", never "expired").
3. Release the browsers; remove the profiles once nothing uses them.

- **A sign-out holds through any later login**, which admits its own session
  only (`AccountPaths::admit_session`). A stored session signed out of is
  refused by every command at its start (`report::SIGNED_OUT_HERE`).
- **An account directory never exists without its `signed-in` list**: a new one
  is built as `accounts/<pk>.making-<pid>-<n>` with the empty list inside and
  renamed into place (`paths::make_signed_out`), so it starts with nobody
  signed in. A directory from before the list existed refuses nobody until its
  first sign-out or login.

### Migrations

A published migration is never edited; add the next numbered file in
`store/sql/` (`store/sql/shared/` for `shared.db`). No `PRAGMA foreign_keys` in
a migration; rebuild a table by create, copy, drop, rename
(`SQLITE_DBCONFIG_DEFENSIVE` forbids `writable_schema`). Counts are not `u64`
columns (`fallible_uint` is off on purpose): they go through
`store::count_to_sql`/`count_from_sql`, and a negative one reads as not known.

## The MCP server

`snob mcp` (`crates/snob-mcp`, feature `mcp` of `snob-cli`; `rmcp` pinned
exactly): a stdio
server an agent's client starts, or with `--listen` (feature `mcp-http`) the
same over HTTP on loopback (`snob-mcp`'s feature `http`), behind the MCP key. `snob mcp config <client>`
prints a client's snippet and never writes it.

### Every call is a child

- **A tool call runs one `snob` command** with the arguments a person would type
  (`tools.rs`, `child.rs`), so every rule above holds for an agent through the
  same code path.
- **The server never builds an Instagram client, opens a database or reads a
  job's file**; it reads only the MCP key. Job I/O goes through `snob __job`.
- Children get the server's globals, `--max-wait` (5 min, `snob_mcp::MAX_WAIT`, when
  the server has none; an hour, `JOB_MAX_WAIT`, for a job's walk),
  `--stop-when-stdin-closes`, and `--spend-alone` when they may reach
  Instagram, which takes `accounts/<pk>/spend.lock` before any request (held:
  `busy`).
- **A child runs in its own process group** (`child::Launch`): closing its
  input is its stop, and it is killed only after `child::GRACE`.
- Arguments are checked, never passed through, and a refusal says back at most
  40 characters of what it refused (`tools::echoed`). A tool not offered is
  JSON-RPC -32602. Everything a child prints goes through `printable`, and is
  read without assuming its shape (one panic is the server gone) and up to a
  bound: past it the child is stopped and the call fails.

### Tools

| Tool | Runs | Asks a yes | Offered |
|---|---|---|---|
| `snob_status` | `status` | no | always |
| `snob_list` | a list or crossing, filtered by `--hide`/`--only` (`verified`, `private`, `no-pfp`), with `--max-age` (a minute to a year, only in mode `auto`); a walk runs as a job (all three are the job's allowed flags too) | another account's lists | always |
| `snob_scan` | `scan`: the five counts of both lists and `followed_by` (cut to one answer, `answer::bounded_scan`); a walk runs as a job, `dry_run` and `stored` inline | another account's lists | always |
| `snob_whoami` | `whoami --json`, `--offline` sends nothing; a dead session is an answer (exit 3, 4, 5 too), `storage_path` left out | no | always |
| `snob_job`, `snob_jobs`, `snob_job_cancel` | `__job show`, `list`, `cancel` | no | always |
| `snob_profile` | `profile`, inline | no | always |
| `snob_posts` | a grid or tab, at most 3 pages | no | always |
| `snob_post` | a post, its comments and replies | no | always |
| `snob_post_likers` | `likers` | yes | always |
| `snob_stories`, `snob_highlights` | `stories`, `highlights` | no | always |
| `snob_pfp` | `pfp --json --embed`, and `--output=<downloads dir>` with `save` | no | always (`save` with `--downloads-dir`) |
| `snob_snapshots`, `snob_changes` | stored captures; nothing sent | no | always |
| `snob_post_download` | `post --download` into `--downloads-dir` | no (the client's prompt) | with `--downloads-dir` |
| `snob_stories_download`, `snob_highlights_download` | `stories -d`, `highlights <n> -d`, with `--json`, into `--downloads-dir` | no | with `--downloads-dir` |
| `snob_follow`, `snob_unfollow` | `follow`, `unfollow` | yes | with `--allow-writes` |
| `snob_logout` | `logout --only=<name>` | yes, naming the account | always |
| `snob_accounts` | `account list --json` | no | with `--any-account` |

**Never offered**: `login`, `logout --all`, `account use`, `purge`, the
monitor, `--same-day`, and **a path or file name as an argument**. The list
depends on the server's flags alone.

### The yes

- **Asked in a form the client shows**; for a client with none, the call answers
  `status: "confirmation_required"` with a question and a `confirmation` token
  the agent hands back on a yes. A no, dismissal or timeout in a form is never a
  reason to ask in the chat.
- `--confirm-in-chat none|lists|all` (`all` on stdio, `lists` with `--listen`)
  limits **the chat route only**; signing out goes through the chat only under
  `all`.
- **The yes comes from the client, not provably from the person**, so the gate
  on a write is the client's own prompt: keep the writes on "ask". **The
  question is snob's words and the validated username only** (`tools::Question`),
  never text another person wrote.
- **A yes counts once, for the command it was given for** (`seal.rs`): an HMAC
  ticket over its kind, a nonce, a deadline (10 min) and the command with its
  account (`Plan::binding`); a nonce is spent once (4,096 kept, full refuses).
  Bound to the MCP session where a request has one; a stateless HTTP call has
  none.
- **What can be refused for nothing is refused before asking**
  (`Server::refused_ahead`), through children that send nothing (`__job start
  --check`, `follow --check-ahead`, `likers --check-ahead`).
- **Every act on a yes is recorded** in `mcp_acts` before its first request.
  Agents' follows and unfollows past `--max-writes-per-day` are refused with
  nothing sent (exit 1, `mcp_write_cap`, `retry_at`); attempts count, and what a
  person types is neither recorded nor counted.

### Limits

| Limit | Value | Where |
|---|---|---|
| Calls at once; of them reaching Instagram; writes | 4; 1; 1 | `limits.rs` |
| Calls a minute | 60 | `limits.rs` |
| A list page | 200 accounts, 30,000 bytes | `answer::PAGE`, `PAGE_BYTES` |
| Lists held for later pages | 4, 30 min, 16 MiB | `pages.rs` |
| A child's output; its messages | 64 MiB; 4 MiB | `child::MOST_OUTPUT`, `MOST_SAID` |
| A job's wait in a call | 20 s default, at most 50; a task's 5 s | `tools::WAIT_SECONDS`, `MOST_WAIT_SECONDS`, `tasks::WAIT_SECONDS` |
| Posts named in `until` | 24 | `tools::MOST_UNTIL` |
| One download call | 1 GiB or 2 min, then no new file | `tools::SAVE_AT_MOST`, `SAVE_WITHIN` |
| Items named in a post's download; in a tray's | 20; 100 | `tools::MOST_ITEMS`, `MOST_TRAY_ITEMS` |
| Small pictures | 512 KiB each, 4 MiB a listing | `media::MAX_SMALL_PICTURE`, `MOST_SMALL_PICTURES` |
| Agents' writes on an account | 10 in any 24 h by default, at most 99 | `mcp_acts::WRITES_A_DAY`, `MOST_WRITES_A_DAY` |

Permits are **taken before anything is asked and never queued**: past a limit a
call answers `busy` at once. A page handed back from a list the server holds
runs nothing, takes no permit and is not counted.

### Lists, jobs and posts

- **A list comes back a page at a time** (`{total, count, offset, items,
  next_offset}`, without `pfp_url`), and **a page past the first never walks**.
  The server applies `limit`. **A list is never handed back as more than it
  is**: `served` says where and when it came from, and a walk cut short answers
  `complete: false`.
- **A list that may be walked is a job** (`tools::Run::Job`), answered inline
  when it ends within `wait_seconds`, else with its `job_id`. One job per
  account. A client that declared MCP Tasks gets a task with the job's id
  (`tasks.rs`), unless the call names `as_account`. People start the same job
  with `--background`. **`scan` is one too** (`jobs::argv::check` takes it, with
  both lists as its claim), though a person has no `--background` for it.
- **A finished job's list is held by the server that handed back a page of it**
  (`pages.rs`, under the job and the account the call acts as), so the
  `snob_job` pages after it start no child; a server that signs an account out
  forgets every list it holds.
- **Jobs** (`jobs/mod.rs`): the supervisor runs this binary (`exe::program`),
  never a path from disk, and checks the stored command line again
  (`jobs::argv`, handed in as `jobs::Check`). **Stopping is
  a `stop` file, never a signal.** An account sees only its own jobs.
- **No media address reaches an agent** (`answer::without_media_addresses` for
  posts, stories and highlights; `pfp_url` is left out of every account,
  `answer::LEFT_OUT`).
  Pictures asked for (`pictures: true`) come back as MCP image content, never a
  video; `snob_pfp` hands back the profile picture the same way (`pfp --json
  --embed`: a picture over `MAX_SMALL_PICTURE` is a `missing` entry), saved into
  `--downloads-dir` as `<user>-<digest>` (`media::short_digest`) only with
  `save`, so a changed picture is a new file and the same one is not written
  again.
- **Files only into the directory the person named** (`--downloads-dir`, which
  must exist), under the names every download gets, and **none written over**
  (`media::save_named`). A push-back ends the run (`media::ends_the_run`).
  **Stories and highlights are saved by their ids**: `stories -d --json` and
  `highlights <n> -d --json` (`output::media::Telling`,
  `media::save_telling`) name a file `<user>-story-<pk>` or
  `<user>-highlight-<pk>` (`media::stable_stem`), never by a tray position,
  which moves as stories come and go and would make a name another story's. The
  command line's own `-d` keeps `<user>-<number>`. A highlight is one number a
  call: a whole tray is not a document of items.
- A post is told apart by `pk` and `code`; `until` stops a grid at the first
  page holding a post the reader has (`posts::reached`, never a pinned one).
  Comments, replies and changes come back bounded, with what was left out
  counted.

### Accounts and hints

- A server acts as `--account` or `SNOB_ACCOUNT`, else the active account; **a
  call that names none acts as whichever is active when its child starts**.
- `--any-account` (refused beside `--account` and `SNOB_ACCOUNT`) offers
  `snob_accounts` and `as_account` on every other tool but `snob_logout`,
  passed as the child's own
  `--account`. Budget, cooldown, `spend.lock`, `mcp_acts` and jobs stay per
  account; a yes is bound to its account; the server's limits stay the server's.
- `snob_logout` always names the account; `logout --only` refuses
  (`not_this_account`) unless the run resolved it.
- **Signing in is the person's**: a failure for want of a session carries
  `answer::SIGN_IN`, and every hint meant for a person at a terminal is replaced
  by one for the agent (`answer::agent_hint`).

### Over HTTP

`snob mcp --listen` (`snob-mcp`, `http.rs`):

- **Loopback only** (else exit 2); TLS is the tunnel's job; `--public-url`
  (https) adds the tunnel's host. **On Unix, `--listen socket`** serves on
  `mcp.sock`, `0600`, this user or root only; refused on Windows.
- **The key is mandatory** (none: exit 1), checked before the body is read.
- **The server does not prove itself to its clients**: whoever binds the port
  while snob is not listening is handed the key. `--listen` on a port is for a
  machine where nobody else runs code; `socket` closes that gap.
- **`Host` and `Origin` checks guard browsers only**; against any other caller
  the key is the only control.
- **The tunnel's provider sees the key and every answer in plain text.** Startup
  and the README say so.

| Limit | Value |
|---|---|
| Path | `/mcp` only |
| Body | 256 KiB within 30 s |
| Sessions | 8 (the idlest without a call closes for a new one); 30 min idle |
| Connections | 32, counted against those that have shown the key |
| First request; headers and idle | 2 s; 10 s, 16 KiB buffer |

- `snob mcp config <client> --remote <url>` prints the address and how to send
  the key from `SNOB_MCP_TOKEN`, never a command or the key.
- Ctrl+C stops every child and **waits for their answers to be written**
  before exiting 130.

### Upgrades

`snob mcp config` prints a path that outlives an upgrade (`config::stable_exe`).
**A server whose binary changed refuses every call that would run a child**
(`upgraded`); the client restarts it. Codex's snippet carries
`tool_timeout_sec` from `snob_mcp::longest_call`.

## Updates

- **How snob was installed decides how it is updated** (`update/how.rs`):
  Homebrew, Scoop, WinGet and Cargo by their own command; a script install by
  replacing itself; the `.deb` only by `apt` as root (run on a terminal, printed
  otherwise). A build in a Cargo `target` directory never updates.
- **Every command takes part first** (`update::at_start`): it applies a staged
  version when no other snob runs (`running.lock`) and starts the day's check
  detached (`__update fetch`). `snob mcp`, the owner and jobs take no part.
- **Checked against the release's `SHA256SUMS`** on download and before it is
  applied; unpacked by the system's `tar`. Never backward, never a
  pre-release.
- State in `update.json` (`snob update --status`, and `update` in `snob status
  --json`), with `--auto off|notify|download|apply` (default `apply`).
- **Never** in a sandbox (but a `testing` build with `SNOB_TEST_UPDATE_ORIGIN`),
  with `CI` or `SNOB_NO_UPDATE_CHECK` set, or on a pre-release build. No retry.
- **An install is updated as the edition it is** (`update/edition.rs`): the
  app's file beside `snob` is the desktop edition, never a setting. Its two
  binaries come from one archive, are written whole beside theirs before
  either is swapped (`apply::replace_all`), the app first and `snob` last.
  The app counts as a running snob while it is open (`Role::Counted`), so
  nothing is applied under it. `edition` is in `snob update --status --json`
  and `snob status --json`.

## Output contract

- **Standard output carries data only**; progress, prompts, warnings and
  full-screen views go to standard error.
- `--format json|ndjson|csv|xlsx|md|table` and `-o <file>` for lists and
  documents; `--json` for status commands; `-o -` is standard output. Not a
  terminal means JSON and no view; `--no-interactive` forces printing.
- **The JSON shapes are the types in `snob-core` (`model.rs`)**, separate from
  `snob-ig`'s wire models. A shape change is a contract change; fields are
  added, never removed or redefined.
- **When the result would have been JSON, a failure is also one JSON object on
  the last line of standard error** (`report::error_json_for`): `code`, `exit`,
  `message`, `hint`, `url`, `cooldown_until` and `retry_at` (epoch seconds), and
  a finer `reason` (`ExitError::with_reason`): `busy`, `mcp_write_cap`,
  `walked_elsewhere`, `nothing_stored`, `stored_moments`, `budget_wait`,
  `consent_required`, `declined`, `no_such_account`, `no_such_snapshot`,
  `not_delivered`, `not_this_account`, `too_many_jobs`, `job_stopping`,
  `no_such_job`, `refused_argv`.

| Exit | Token | Means |
|---|---|---|
| 0 | `ok` | worked (a list cut short by `--limit` or `--max-pages` too) |
| 1 | `error` | failed, or a result refused as incomplete |
| 2 | | bad command line (clap); nothing done |
| 3 | `no_session` | no session, a dead one, or signed out here |
| 4 | `challenge` | the account needs verifying |
| 5 | `rate_limited` | throttled or in cooldown (`snob status` exits 5 in cooldown whatever was asked) |
| 130 | `interrupted` | Ctrl+C, a confirmation not given, a wait past `--max-wait`, standard input closing under `--stop-when-stdin-closes`, the driving program gone |

One vocabulary for `exit.rs`, `snob_core::watch::RunOutcome`,
`watch_runs.outcome` and the `--json` `outcome`.

- **Every string another person wrote goes through
  `snob_core::model::printable`** before a terminal, a JSON list or a report. It
  removes `Cc`, `Cf` and every `Default_Ignorable_Code_Point`, keeping a
  variation selector only after a symbol that takes one
  (`takes_a_presentation`) or a keycap base.
- **No at sign in a command handed over to be typed back**: PowerShell eats an
  unquoted one, and the command answers about the user's own account.

## Code conventions

### Comments

**Few, only the necessary ones:**

- every `// SAFETY:`;
- where a magic number came from (a capture and its date, a measurement, a
  paper);
- a non-obvious why the code cannot say;
- the guard escapes;
- one short `//!` header per module: what it is for, which rule it keeps;
- one short sentence on a public item whose name does not say it.

No narration, no history, no rationale repeated in several places: one home per
rule. Bold marks the claim a reader must not miss.

### Language

English, **US spelling**. A test enforces it (`language.rs`).

### Rust

- Errors: `thiserror` enums in the libraries below the engine, `anyhow` in
  `snob-engine`, `snob-mcp` and `snob-cli`, with `ExitError` for anything with
  an exit code (`exit::exit_code_for`).
  Instagram's answers are classified in `snob_ig::error` (status codes lie; the
  body says why); everything in `snob_ig::model` but `pk` is optional.
- Lints: `undocumented_unsafe_blocks` and `return_self_not_must_use`, added one
  at a time, each for a defect this codebase had; no `pedantic` or `nursery`.
  The one `unsafe fn` is `snob_store::windows_user::User::of`.
- Every dependency and feature in a `Cargo.toml` says why it is there and why
  its features are what they are; features are narrowed to what is used. No
  OpenSSL and no `ring` on any target; `aws-lc-rs` only as the provider under
  rustls, on the four targets that build it.
- Release profile: `opt-level = "s"`, fat LTO, one codegen unit, stripped,
  **`panic = "abort"`**: no destructor runs on a panic, so durable state is
  committed as it is made, and code fed by a browser never panics.
- `tokio` with exactly two workers (`main.rs`), so a blocked task cannot starve
  the Ctrl+C handler.
- **Every future `main`, `run` and `dispatch` awaits goes through `boxed`**
  (`main.rs`), a new command's arm included, or a debug build overflows the
  1 MiB main stack on Windows.

### Terminal UI

- `ratatui` draws; keys come through `ui/browser/input.rs`, never
  `console::Term::read_key`. `ui::tui::Tui` is the one claim on the terminal,
  restored on drop; a frame leaves in one write, and an unchanged one writes
  nothing.
- **A color is named by its role** (`ui/theme.rs`: accent, ok, warn, error,
  info), each one of the 16 ANSI colors; `SNOB_THEME=brand` is the one
  exception. The selection is reverse video.
- A view entered from another is lent its guard and scratch directory; from
  the profile card never call `ui::stories::browse` or
  `ui::highlights::browse`.
- **A fetch a key starts runs under a `Busy`** (`ui/browser/busy.rs`) that says
  what is going on and that `q` or Ctrl+C stops it.
- **A view's keys are in a table beside its binding** (`KEYS`, shown by `?`),
  and a test presses every key it names: change both together. `h j k l` are
  the arrows.
- `stories` and `highlights` are separate commands because a highlight has a
  tray level and an items level (`snob highlights someone 2`); `posts` has both
  too. Shared flags: `cli::MediaActionArgs`.

### Stories are handed to the system viewer

A view writes the file into the session's scratch directory
(`ui/browser/scratch.rs`) and opens it with `opener` in the user's own viewer.
**The one thing drawn in the terminal is a still picture** (`ui/preview.rs`,
feature `preview`): on `p`, or small beside the text once the cursor rests.
Never a video, never anything marked as seen. Where the terminal is known to
draw pixels it gets kitty's protocol, iTerm2's images or sixel
(`ui/graphics.rs`), elsewhere half blocks; the terminal is never asked, and
`SNOB_IMAGES` overrides the guess.

### The monitor

- **A run compares against what was last reported** (`watch_marks`), not the
  newest capture; a report is queued before its mark moves.
- **A post is new when its pk was never seen and it was taken after the newest
  one seen** (`snob_core::watch::posts`); a first read lays a baseline and says
  nothing. A `taken_at` more than a day ahead of this machine's clock never
  becomes that floor (`watch_posts::AHEAD_AT_MOST`). **`counts.total` does not
  count posts.**
- The delivery queue is drained once per run, after every account; a report is
  owed for a week. `watch once` exits 1 (`not_delivered`) when one it tried did
  not arrive; `watch deliver` is the drain alone, sending nothing to Instagram.
  Receivers deduplicate on `run_id`, never on the queue's reused row id.
- `watch unit` prints, never installs, a unit running `watch once --when-due`
  every 15 minutes; the schedule is evaluated only in snob, and a run not due
  reads no session and sends nothing. **A run counts from its start.**
- `--every` is a floor from the last run, `--on/--at` or `--cron` a calendar;
  the next run is on the calendar and past the floor (`--every 2w --on mon`,
  which cron cannot say), on the wall clock. Cron day-of-month and day-of-week
  combine with OR.
- A run records when it was due, began and ended (`RunTiming`); late is past
  `watch::LATE_AFTER` (30 min) beyond its jitter. What it reports as spent is
  what the pacer was charged.

### Automations

`snob watch` and the app's automations are one thing: steps on a canvas
joined by arrows (`snob_core::automation`), triggers to actions to filters to
one webhook (`can_join`), a disabled step left out of a run.

- **`watch.toml` is the automation `watch`** (`snob_store::automations::
  from_watch`), and `snob watch` builds its entries from it
  (`snob_engine::automations::watched`); the app writes it back only when
  the file can hold it (`to_watch`, else `Problem::NotInWatchToml`). The
  others are JSON files under the configuration directory's `automations/`,
  no secret in them.
- **What an action reads**: lists and posts are the monitor's tick; the rest
  (`automations::read`: unfollowers, fans, friends, the friends two accounts
  have in common, who of the people you follow follow an account, a profile
  picture) read every list through `engine::list`. **Nothing writes to
  Instagram.** An account's lists or posts are read by one automation at most
  (`clashes_with`), since what has been reported of them is kept once.
- **Filters**: verified, unverified, public, private (`filters::Filter`), and
  "changes": what came and went since the last report, against the mark
  `automation_marks` keeps in the reading account's database, moved in the
  transaction that queues the report.
- **When**: each schedule trigger is a `watch.toml` schedule (`Spec`, built in
  one place). `snob watch` (the scheduled mode) and a timer's `watch once
  --when-due` run every automation that is due, from its last start in
  `shared.db`'s `automation_runs` (its history, the newest 100); `watch once
  --automation <id>` runs one now, which the app starts
  (`automations::run_now`). An app automation's webhook gets its own headers
  and **never the keyring's token or signing key**, set up for `watch.toml`'s
  address; its report is event `automation.found` (`wire::found_payload`).

## Platforms

Releases: `x86_64-pc-windows-msvc` (`x86-64-v2`, Windows 11 only),
`aarch64-pc-windows-msvc`, `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl` (static), `aarch64-apple-darwin`; no Intel macOS.
The five triples appear in `ci.yml`, `release.yml`, `deny.toml` and
`packaging/render.sh`: a new target touches all four. The desktop edition
adds `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` for the app
alone (`release.yml`'s job `desktop`, `update::edition::APP_TARGET`,
`install.sh`).

Platform code compiles only on that OS's CI job:

| File | Windows (`windows-sys`) | Unix (`libc`) |
|---|---|---|
| `pipe.rs` | handle list and job object | CDP pipe on fds 3 and 4, own process group |
| `detach.rs` | own handle list, out of the job | `setsid` |
| `local_socket.rs` | named pipe, user DACL (`windows_user.rs`) | `0600` socket, `peer_cred` |
| `owner/end_of_session.rs` | hidden window hearing the session end | |
| `parent.rs` | ToolHelp, `OpenProcess`, `WaitForSingleObject` | `pidfd` (Linux), `kqueue` (macOS), `getppid` |
| `headless/` | | `kill(pid, 0)`, `gethostname`, refusing root |
| `relay/`, `snob-mcp`'s `http/socket.rs` | | `peer_cred` against `geteuid` |
| `power/` | power requests, EcoQoS | logind inhibitor, `SCHED_BATCH` (Linux); `caffeinate` (macOS) |
| `snob-store/src/paths.rs` | the data directory's DACL | scratch root opened with `O_NOFOLLOW`, `0700` |
| `snob-store/src/disk.rs` | `GetDiskFreeSpaceExW` | `statvfs` |
| `snob_ig::awake` | `QueryUnbiasedInterruptTime` | |
| `update/` | | `access(W_OK)`, `exec` |

Static CRT and `/Brepro` on Windows come from `.cargo/config.toml`.

## Known walls

What Instagram does that snob can only report:

- **Truncated lists.** A walk short of the declared size past
  `pager::TRUNCATION_THRESHOLD` (10,000, a guess) ends `StopReason::Truncated`,
  which is no success and no baseline. Some accounts' `following` hits it every
  time, so `watch status` treats a list never read as unhealthy.
- **Profiles that answer 400** (some business accounts): the id comes from
  search, with no counters, so truncation and freshness cannot be judged
  (`report::counters_unknowable`).
- **Private accounts the viewer does not follow**: refused before any walk;
  their grid is not asked for.
- **A collaboration accepted later** keeps its old `taken_at`: the monitor does
  not report it. **More new posts between two runs than three pages hold** are
  reported as far as three pages go, and said incomplete
  (`snob posts <name> --pages N` reads further).

## Costs worth knowing

A walk is not compute: about 0.69 s of CPU for a 12,000-follower walk against
some fifteen minutes of pacing. Optimize for requests sent and binary size.

| Setting | Measured | Where |
|---|---|---|
| `opt-level = "s"` | 23.7% off the Windows ARM64 binary; at most +7.7% on CPU-bound exports | `[profile.release]` |
| Static CRT on Windows | +126,976 bytes; imported DLLs 20 down to 11 | `.cargo/config.toml` |
| Feature `xlsx` | about 498 KiB and six crates | `crates/snob-cli/Cargo.toml` |
| Windows keyring without `search` | 863,744 bytes, 11.4% | `Cargo.toml` |
| Feature `mcp-http` | 486,912 bytes, 4.8% of the Windows x86-64 binary | `crates/snob-cli/Cargo.toml`, `crates/snob-mcp/Cargo.toml` |
| Feature `preview` | 102,400 bytes, 0.9% | `crates/snob-cli/Cargo.toml` |

All four features are on by default; `--no-default-features` drops them.

## Open questions

Assumptions not yet confirmed against the real app. Settle them with a capture
(`tools/capture/record.js`, redacted into the ignored `tools/capture/out/`;
`summarize.js` reads them):

- the paths and thresholds the listener counts as push-back
  (`headless/listen.rs`);
- the prefix of `CLAIM_HANDED_OUT` (`headless/guard.rs`);
- whether Instagram varies on `Sec-Fetch-*` (`client/headers.rs`);
- what a call without `X-FB-Entry-Nonce` draws (`web::AppCalls`);
- `pager::TRUNCATION_THRESHOLD`;
- a profile's tabs (`posts --tab`): where the app keeps their six `doc_id`s
  (seeds from two captures), what a moved one answers, how many reels a next
  page asks for (3 or 5 seen; snob sends 3), what a private account's tab
  answers (snob asks none);
- the second page of a thread of replies, never captured: snob reads the first
  and logs the answer's keys when it says there is more;
- whether the document's `PolarisViewer` carries the account's bio, as it does
  its name and picture (`page_values::Face`): read when there, never needed;
- keys the fake Instagram sends that two captures never showed
  (`snob-fake-ig`'s drift test lists them): the hover card's `biography`,
  `category`, `external_url`, `is_business` and `friendship_status.muting`;
  `play_count` on grid items; a reel's `caption` and `media_notes`;
  `video_versions: null` on reels, reposts and tagged posts; and
  `has_anonymous_profile_picture`. snob reads some of them, all optional: a test
  may pass on a key the real query never sends. Each leaves the fake once a
  capture of the case that should carry it (a video in the grid, a post with
  notes) still lacks it.

## Website

`web/`: a static Astro site (Tailwind 4) on Cloudflare Workers from `dist/`
(`wrangler.jsonc`, `snob.dennisgr7.dev`). `web/worker/index.ts` runs for the
hero's videos only, to answer byte ranges, and sets its own headers. Workers
Builds deploys `main` and previews every other branch. Node `24.22.3`, pnpm
`12.18.0` via Corepack, exact versions, a 24 h release quarantine and an
install-script allowlist (`web/pnpm-workspace.yaml`).

`.github/workflows/web.yml`, on changes under `web/`:

```bash
cd web
pnpm install --frozen-lockfile
pnpm exec playwright install --with-deps chromium
pnpm check && pnpm build && pnpm test   # Playwright against `wrangler dev` serving dist/
```

- `web/DESIGN.md` is the reasoning behind the look; a change to the look starts
  there.
- **Every word on the page is in `web/src/lib/content.ts`** (shared by the HTML,
  `llms.txt` and `index.md`), and **it may promise nothing the README and
  `--help` do not.** The version is read from `Cargo.toml`; never copy it.
- Scripts are hashed into the CSP: no `is:inline` code, no `style` attributes;
  everything works without JavaScript.
- `pnpm brand` regenerates `public/` images; `bash web/video/record.sh` then
  `pnpm video` re-film the hero (both committed) when a view it shows changes.

## Desktop app

`apps/desktop/`: a GPUI window on the engine, linked in process, **from a cold
start to somebody signed in and welcomed**: the frame and its band, signing
in through snob's own browser window or with a pasted session ID, several
accounts, signing out, a command palette, and the settings: the theme and
the language chosen over the system's, the log, and what snob keeps on the
computer: the disk it is on, and each account's, signed in or not, with
its cache to clear and its history to delete after a question. A clearing
or a deletion is the shell's and runs on in the engine whatever screen is
shown. Once somebody is signed in the frame
opens on a rail of sections (home, search, automations). Home and search
are a title until what a scan shows comes next; automations has an
introduction shown once an account, a list with its search, an
automation's page (its runs on the left, its flow in words, a pencil to
edit, a run now), and an editor on the engine's automations ("Automations"):
steps put from the panel, moved, joined and configured in a dialog (a
schedule, an action's account and consent, a webhook), disabled from a
node's menu, **kept at every change** with undo, and an unfinished one
asked about on leaving (discard, or back to the editor). The automations
run in the command line's scheduler, never in the app. From the account's menu
(and the palette), the account's own **profile page**, the same page for
any other account opened from one of its lists: the picture (large,
zoomed, copied or saved from a viewer of its own), the names, the bio, the
counters that open the two lists, the highlights and the first page of
three tabs, laid out with the card above the posts or beside them. A
list's rows come in as the walk reads them, and a list can move out to a
window of its own.

**A workspace of its own**, with its own `Cargo.lock`: GPUI brings some 500
crates and does not build for musl. Build and test it from that directory;
no line of `ci.yml` builds it, and `Formula/`, `bucket/` and WinGet know
nothing of it. `release.yml` builds it into the desktop edition only once
its version is the release's ("Release and packaging"). GPUI comes through
`gpui-kit`, pinned exactly, everything from crates.io.

### How it is put together

- **Every rule of this file holds there through the engine.** It builds no
  Instagram client and opens no database of its own: `backend/engine.rs`
  calls what a command calls (`snob_engine::login`, `logout`, `app`,
  `viewer`, `log`, `browser`).
- **A view never waits on the engine**, which runs on a tokio runtime of two
  threads of the app's own (`runtime.rs`). A view asks through
  `backend::Backend` and is answered on a channel; the trait is the engine
  (`backend/engine.rs`) and, in a test build, a script
  (`backend/scripted.rs`).
- **What is shown is decided apart from how it is drawn** (`state/`): the
  screen and why (`flow.rs`), the frame's shape and its way between them
  (`frame.rs`), what the band says (`status.rs`), what a pasted ID is
  (`paste.rs`), what the palette offers (`commands.rs`), what the settings
  hold and what their search shows (`settings.rs`, `storage.rs`), what is
  remembered between runs (`prefs.rs`), when the app rests (`rest.rs`),
  the automations section (`automations.rs`), the automation being edited
  with its undo and its saves (`editing.rs`), and how a profile is laid
  out, zoomed and when its lists ask first (`profile.rs`). No window, clock
  or network there.
- **A theme or a language chosen holds whatever the system says**; the
  contrast is always the system's (`ui/look.rs`). **What is kept is measured
  from the files' sizes** and clearing it removes the cache alone
  (`snob_engine::storage`), never a session, a list or a log.
- **A view never calls an operating system** (`system/`): it reads `Settings`
  and takes `Event`s from one trait, which GPUI and Windows answer
  (`native.rs`, `windows.rs`) and a test speaks for (`simulated.rs`). One
  listener at a time: a window opened again takes over.
- **A pasted session is sent as the browser it was copied from**, named
  under the field when several are installed. The engine keeps that name
  with the session and follows the browser's later versions
  (`browser::refresh_user_agent`). The marks beside the names are the app's
  own plain drawings, not the browsers' artwork.
- **The processes the engine starts are the command line's**
  (`exe::start_as`): the app points the engine at the `snob` beside it and
  logs when there is none.
- **Every word is in `locales/en/snob.ftl` and `locales/es/snob.ftl`**, chosen
  by the system's language unless one is chosen in the settings, and held to
  the same messages by a test. Spanish belongs there; `language.rs` does not
  scan `.ftl`.
- **Closing the window is leaving, but on macOS**, where the app stays in the
  Dock and a click there opens the window again (`main.rs`).

### What it never does

- **Nothing is sent to decide what a start shows**: who is signed in is read
  from files. The same files are read again when the window is looked at
  again, which is how what the command line did meanwhile is seen.
- **The session of the account in use is asked about once a day at most,
  never in a cooldown** (`Backend::expired_session`); a login counts.
- **Nothing is sent to find a network**: the app follows what the system
  says, and where it does not say, assumes one.
- **No secret reaches the log, the diagnostic report or a file of the
  app's.** The report makes accounts anonymous and is copied by the person,
  never sent.
- **A profile page reads what `snob profile` reads, less the stories**
  (`Backend::profile`, `tiles`, `list`): a ring says a story is up and its
  menu's "Open story" opens nothing, reads nothing and marks nothing as
  seen. **Another account's list is asked about in the app's own question
  before anything of it is sent**, and only a yes calls `engine::list`
  (`state::profile::asks_first`). A post, a highlight and a story are not
  opened; a row of a list shows a picture only when the cache already holds
  one; the large picture comes from the CDN, never from the API. No follow
  or unfollow is offered.
- **A build without the feature `testing` has no test mode**: no
  `--test-root`, no simulated system, no control channel
  (`src/control.rs`). The harness's privacy suite reads both of these off
  the real binary.

### Its tests

```bash
cd apps/desktop
cargo fmt --check
cargo clippy --all-targets --features testing --locked -- -D warnings
cargo clippy --all-targets --locked -- -D warnings
cargo harness quick     # what a change is judged by
cargo harness all       # and the two suites that run nightly
cargo harness demo      # the test build, by hand, on a fake Instagram
```

`cargo harness` (`examples/harness/`, an example so that it links only
dev-dependencies) owns the fake Instagram and the app it starts, in a
directory of its own with no real keyring, and writes
`target/harness/report.html`. A scenario that fails leaves its last steps as
pictures, one of the window when it failed, and the app's log.

| Suite | What it checks |
|---|---|
| `logic` | `cargo test --features testing`: the state, the backend on the engine against the fake Instagram, the views in process |
| `behavior` | the real binary, driven through its control channel, from a first start to signing out with the keyboard alone |
| `visual` | a picture of every screen in six looks (light, dark, both with more contrast, twice the text, 800 by 600), against `tests/approved/<os>/` |
| `accessibility` | a name, a role and 24 by 24 on every control; one title a screen; every control reached with Tab |
| `privacy` | no secret in the log, the report or the files; no test mode in a build without the feature |
| `performance` (nightly) | start, memory and processor at rest, size, against this machine's `tests/baseline/<os>.json`; fails on 10% worse |
| `endurance` (nightly) | 200 sleeps and 60 changes of account in one run, which must still answer and not have grown |

- **The control channel** is the local socket of the same user under the test
  root, one JSON request a line. It reads the window's accessibility nodes
  and acts on one by its id, as a screen reader would, or moves the pointer
  one step at a time, so a drag is looked at half-way; it never reaches into
  a view.
- `--approve` takes this run's pictures and numbers as the approved ones.
  **The pictures are committed, a system each; the numbers are a machine's
  own** and are not, so a runner with no baseline records them and fails
  nothing.
- **A test build lists as many browsers as it is told** (`--browsers`), so a
  picture is of the same ones on every machine. That changes the list alone:
  a session is still sent as a browser that is installed.
- **A test build says the same sizes of what is kept when told**
  (`--fixed-storage`), for the same reason. That changes what is shown
  alone: clearing the cache still removes it.
- `.github/workflows/desktop.yml` runs the lines above on the five runners of
  `ci.yml`, the nightly suites on a timer; `bash tools/ci/run-desktop.sh`
  runs the Linux ones in Docker under Xvfb, with a browser in the image
  (`tools/ci/desktop.sh` stays in step with the workflow). The source-reading
  guards of `crates/snob-core/tests/` read `apps/desktop` too, from the root.

### What is verified, and what is not

| | |
|---|---|
| Windows 11 x86-64 | lints, the five suites of a change, the two nightly ones |
| Linux x86-64 (glibc) in Docker under Xvfb, on Mesa's CPU Vulkan | lints, and the suites of a change but for the pictures: GPUI does not draw the window to a picture there, and the visual suite says so |
| Windows ARM64, Linux ARM64, Wayland, macOS, GitHub's runners | **not yet**: `desktop.yml` has never run |
| What Windows is set to (contrast theme, animations, transparency, battery saver, text size, network, session end) | read and followed; a change is tested by sending the window its message, not by changing the machine |
| The same on macOS and Linux | **not asked yet**: the app follows only the theme there |
| Staying in the Dock on macOS, and opening the window again from it | **written, never run** |
| A screen reader's own reading (Narrator, VoiceOver, Orca), a live region heard | not verified: the tree is checked by role and name, not a reader's speech |
| Windows 11 Snap Layouts over the caption buttons the app draws | yes, when the drag area is a sibling of the buttons |
| The list of several browsers under the field | drawn and audited from a test build told to list three; one browser on the machine it was written on |
| The app's own update | the command line replaces it with itself (the desktop edition, "Updates"), tested in `crates/snob-cli/tests/sandbox.rs` on Unix only; the app's own update screens still ask a backend that answers none, tested on the scripted backend |
| System notifications | shown through GPUI; on macOS only from inside a bundle |
| `cargo deny` for this workspace, packaging, signing, an installer | none |

**At rest the app asks for no frame** and holds about 85 MiB. **Not looked
at and still for ten seconds, it rests** (`state/rest.rs`): its own process
goes to EcoQoS and the lowest priority on Windows, which Task Manager marks
with its efficiency leaf, and to `SCHED_BATCH` on Linux; macOS's App Nap
does the same unasked (`power::qos::this_program`). It wakes before the
next frame is drawn or the window is looked at; the browsers and the
`snob` it starts keep their own rule. GPUI wakes
every window on each refresh on Windows (its `VSyncProvider` thread),
minimized or not: on a 164 Hz display a release build spends 0.9 to 1.2% of
one processor there, some 10 ms a second, and a debug build 1.6%.

The app's `Cargo.lock` holds a second GPUI, `gpui-fast`, that is never built:
GPUI's test support names it as an optional dependency, and Cargo locks what
a feature names whether or not it is enabled.

## Release and packaging

1. Changes land on `dev` by pull request; Dependabot targets `dev` weekly
   (cargo updates lockfile-only).
2. A release commit sets the version (subject `Snob CLI X.Y.Z`); `dev` goes to
   `main` by pull request, reviewed by `claude-code-review.yml`.
3. Push a tag `vX.Y.Z` on `main`. `release.yml` refuses one that is not an
   ancestor of `origin/main` or disagrees with `Cargo.toml`, builds the five
   targets reproducibly, publishes `SHA256SUMS` and a provenance attestation,
   runs `packaging/render.sh`, commits "Point the formula and the manifest at
   vX.Y.Z" to `main` and cherry-picks it onto `dev`.
4. `Formula/snob.rb` and `bucket/snob.json` are read straight from this
   repository; `packaging/winget/` goes to `microsoft/winget-pkgs` by hand.
   All three install the cli edition. `packaging/install.sh` and
   `install.ps1` install the latest release (`SNOB_VERSION`,
   `SNOB_INSTALL_DIR`, `SNOB_EDITION=desktop|cli`): the desktop edition
   where `SHA256SUMS` lists it for the machine, else the cli edition, said
   so. A release up to 0.9.0 is verified as signed in `dennisgr7/snob-cli`,
   the repository's name then.

**Two editions of one release**, archived by `packaging/package.sh` alone
(the one definition of what each holds; it rebuilds 0.9.0's archives byte
for byte):

| Edition | Archive | Holds | Targets |
|---|---|---|---|
| desktop | `snob-desktop-v<version>-<target>` | `snob-desktop` and `snob` | the two Windows ones, and Linux as `-gnu` (GPUI needs glibc); no macOS until there is a bundle |
| cli | `snob-v<version>-<target>`, and the `.deb` | `snob` | the five |

- The cli archive keeps the name every released snob updates from and
  Homebrew, Scoop and WinGet install from.
- The desktop edition's `snob` is the file the cli archive for that machine
  holds, taken out of it, never built twice; on Linux, the static one.
- **The app ships only when `apps/desktop/Cargo.toml` says the tag's
  version**; at `0.0.0` job `desktop` is skipped and the release is the cli
  edition's alone. Any other version is refused by `check` and by
  `crates/snob-core/tests/layers.rs`.
- `packaging/docker/Dockerfile` builds the cli edition into an image from a
  released binary checked against `SHA256SUMS`: Chromium, no root, no
  keyring (`--no-keyring`, `SNOB_MCP_KEY`), no self-update, data under
  `/data`. `release.yml`'s job `image` publishes it for each release, for
  amd64 and arm64, as `ghcr.io/dennisgr7/snob:<version>`, and `:latest`
  when it is the newest version, attested like the archives.

Workflows pin every action to a commit SHA with its version in a comment, and
start from `permissions: contents: read`. `claude.yml` and
`claude-code-review.yml` take effect only once identical on `main`.

## Git

- Work goes to `dev`; `main` takes only what `dev` has. Topic branches are named
  by kind and subject (`feat/ssh-remote-login`, `deps/rustls-0.23.46`) and
  merged into `dev` by pull request.
- Commit subjects are English sentences in the imperative, saying what the
  change does for the person using snob, with no type prefix and no final
  period ("Bind the tunnel's near end to loopback by name"). The body, wrapped
  at about 72 columns, says why: the failure, the change, what was measured.
  Dependency bumps name the advisories they answer.
- No `Co-Authored-By`, "Generated with" or other trailers; never push unless
  asked.
- Never commit what snob produces (exports at the root, `tools/capture/out/`,
  anything under `/docs/`). A new root-level `.md` is ignored unless
  `.gitignore` lists it.
