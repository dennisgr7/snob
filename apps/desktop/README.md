# snob desktop

A window on the engine the command line runs. It goes **from a cold start to
somebody signed in and welcomed**: the frame and its band, signing in through
snob's own browser window or with a pasted session ID, several accounts and
changing between them, signing out, the settings of its log, a command
palette. The home screen is a title: what a scan shows comes next.

```bash
cd apps/desktop
cargo run                       # the engine and GPUI: a few minutes the first time
cargo harness                   # every test of the app, from one place; asks which
cargo harness quick             # what a pull request runs
cargo harness demo              # the test build, by hand, on a fake Instagram
```

## How it is put together

- **The engine is linked in process** (`snob-engine`), not driven through the
  `snob` binary: a view asks what a command asks, so every rule in the root
  `AGENTS.md` (the two writes, the pace, the first push-back, consent) holds
  here through the same code. Nothing in this crate builds an Instagram
  client or opens a database on its own.
- **A view never waits on the engine.** The engine runs on a tokio runtime of
  two threads of the app's own (`runtime.rs`); a view asks through
  `backend::Backend` and is answered on a channel. The trait has two
  implementations: the engine (`backend/engine.rs`) and, in a test build, a
  script (`backend/scripted.rs`).
- **What is shown is decided apart from how it is drawn** (`state/`): the
  screen and why (`flow.rs`), what the band says and which of several things
  wins (`status.rs`), what a pasted ID is (`paste.rs`), what the palette
  offers (`commands.rs`), what is remembered between runs (`prefs.rs`). No
  window, clock or network in any of it.
- **A view never calls an operating system** (`system/`): it reads `Settings`
  and takes `Event`s from one trait, which GPUI and Windows answer
  (`native.rs`, `windows.rs`) and a test speaks for (`simulated.rs`).
- **A pasted session is sent as the browser it was copied from**, which the
  person names under the field when more than one is installed: the engine
  keeps that name with the session and follows the browser's later versions
  (`snob_engine::browser::refresh_user_agent`). The marks beside the names
  are the app's own plain drawings, not the browsers' artwork.
- **The words are Fluent files** (`locales/en`, `locales/es`), chosen by the
  system's language; a test holds the two to the same messages.
- **The processes the engine starts are the command line's**
  (`snob_engine::exe::start_as`): the owner of the browsers, a job's
  supervisor, the updater. The app points the engine at the `snob` beside it
  and logs when there is none.
- **GPUI, through `gpui-kit` pinned exactly**; everything from crates.io.
  **A workspace of its own**: GPUI brings some 500 crates and does not build
  for musl, and the command line's lockfile, `cargo deny` checks, CI lines
  and five release targets do not change because this directory exists.

## What it never does

- **Nothing is sent to decide what to show at a start**: who is signed in is
  read from files. The session of the account in use is asked about behind
  the window, **once a day at most and never during a pause** Instagram asked
  for (`Backend::expired_session`).
- **Nothing is sent to see what the command line did meanwhile**: when the
  window is looked at again, the files a start reads are read again.
- **Nothing is sent to find out whether there is a network**: the app follows
  what the system says, and where the system does not say, assumes one.
- **No secret reaches the log, the diagnostic report or a file of the app's**
  (the harness's privacy suite reads all three after a sign-in). The report
  makes accounts anonymous, and is copied by the person, never sent.
- **A release build has no test mode**: not among its dependencies, none of
  its arguments in the binary, no control channel (same suite).

## Tests

`cargo harness` (`examples/harness/`, an example so that it links only
dev-dependencies) owns the fake Instagram (`crates/snob-fake-ig`) and the app
it starts, in a directory of its own with no real keyring, and writes
`target/harness/report.html`.

| Suite | What it checks | When |
|---|---|---|
| `logic` | `cargo test --features testing`: the state, the backend on the engine against the fake Instagram, the views in process | every change |
| `behavior` | the real binary, driven through its control channel, from a first start to signing out with the keyboard alone | every change |
| `visual` | a picture of every screen, light, dark, both with more contrast, at twice the text and at 800 by 600, against `tests/approved/<os>/` | every change |
| `accessibility` | a name, a role and 24 by 24 on every control; one title a screen; every control reached with Tab | every change |
| `privacy` | no secret in the log, the report or the files; no test mode in a build without the feature | every change |
| `performance` | start, memory and processor at rest, size, against this machine's `tests/baseline/<os>.json` (not committed); fails on 10% worse | nightly |
| `endurance` | 200 sleeps and 60 changes of account in one run, which must still answer and not have grown | nightly |

`--approve` takes this run's pictures and numbers as the approved ones. The
pictures are committed, a system each; **the numbers are a machine's own**, so
a runner with no baseline records them and fails nothing. A scenario that
fails leaves its last eight steps as pictures, one of the window
when it failed, and the app's log.

**The control channel** (`src/control.rs`, feature `testing` only): the local
socket of the same user under the test root, one JSON request a line. It
reads the window's accessibility nodes and acts on one by its id, as a screen
reader would; it never reaches into a view.

On Linux: `bash tools/ci/run-desktop.sh` runs the lints and the harness in
Docker under Xvfb. `.github/workflows/desktop.yml` runs the same on the five
runners of the command line.

## What is verified, and what is not

| | |
|---|---|
| Windows 11 x86-64: lints, the five suites of a change, the two nightly ones | yes |
| Linux x86-64 (glibc) in Docker under Xvfb, on Mesa's CPU Vulkan | yes: lints, and the suites of a change but for the pictures |
| Windows ARM64, Linux ARM64, Wayland, macOS, GitHub's runners | **not yet**: they need the branch on GitHub |
| What Windows is set to (contrast theme, animations, transparency, battery saver, text size, network, session end) | read and followed; a change is tested by sending the window its message, not by changing the machine |
| The same on macOS and Linux | **not asked yet**: the app behaves as on a system that set none, and follows only the theme |
| A screen reader's own reading (Narrator, VoiceOver, Orca) | not verified: the tree is checked, by role and name, not a reader's speech |
| A live region announced | set through AccessKit; not heard |
| Windows 11 Snap Layouts over the caption buttons the app draws | yes |
| The window drawn to a picture | Windows yes; Linux no ("not implemented for this platform"), where the visual suite says so and compares nothing |
| Nothing drawn at rest | the app asks for no frame at rest, and holds about 85 MiB. **GPUI wakes every window on each refresh on Windows** (its `VSyncProvider` thread), minimized or not: on a 164 Hz display a release build spends 0.9 to 1.2% of one processor there, some 10 ms a second, and a debug build 1.6% |
| Closing the window on macOS leaves the app in the Dock, and a click there opens the window again | **written, never run**: there is no macOS here. Elsewhere closing the window is leaving |
| The list of browsers under the field | one browser on this machine; the list of several is drawn and audited from a test build told to list three |
| The app's own update | **none**: there is no package of the app to fetch. The screens for it are tested on the scripted backend |
| System notifications | shown through GPUI; on macOS only from inside a bundle |
| `cargo deny` for this workspace, packaging, signing, an installer | none |

`Cargo.lock` holds a second GPUI, `gpui-fast`, that is never built: GPUI's
test support names it as an optional dependency, and Cargo locks what a
feature names whether or not it is enabled.

The rules of the repository apply here unchanged: English, US spelling, few
comments, no test that reaches Instagram or the real keyring.
