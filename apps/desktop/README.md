# snob desktop

A window on the engine the command line runs. **A skeleton**: it opens, lists
the accounts snob knows on this computer (`accounts.toml`, a file read), and
sends nothing. No view reads a session or starts a walk yet.

```bash
cd apps/desktop
cargo run            # the engine and GPUI: a few minutes the first time
cargo test
```

## Decisions

- **The engine is linked in process** (`snob-engine`), not driven through the
  `snob` binary: a view calls what a command calls (`snob_engine::app::App`,
  `engine::list`, `posts`, `media`), so every rule in the root `AGENTS.md`
  (the two writes, the pace, the first push-back, consent) holds here through
  the same code. Nothing in this crate builds an Instagram client or opens a
  database on its own.
- **The engine is handed what it needs** (`src/front.rs`, `snob_engine::host`):
  where its notices go, once per process; and per run a `host::Front`, which
  is who watches a walk (`Progress`) and who answers the consent question
  (`Ask`). The command line hands its bars and its prompt; the app will hand a
  view's model and a dialog.
- **The processes the engine starts are the command line's**
  (`snob_engine::exe::start_as`): the owner of the browsers, a job's supervisor
  and walk, the updater. The app has none of their hidden commands, so it
  points the engine at the `snob` that ships beside it and says so when there
  is none.
- **The MCP server stays `snob mcp`**: the app offers it by starting and
  configuring that command (`snob mcp config`), not by serving it itself.
- **GPUI, through `gpui-kit` pinned exactly** (Longbridge's component library
  on a published snapshot of Zed's GPUI, `gpui-pre`). Everything comes from
  crates.io; nothing is a git dependency. The fallback, should GPUI stop
  building on a target snob ships to, is egui.
- **A workspace of its own.** GPUI brings some 500 crates and does not build
  for musl. The root workspace's `Cargo.lock`, its `cargo deny` checks, its CI
  lines and the five release targets of the command line do not change because
  this directory exists.

## What is verified, and what is not

| | |
|---|---|
| Builds on Windows x86-64 (static CRT, as `.cargo/config.toml` sets it) | yes |
| Windows ARM64, Linux (glibc, X11 and Wayland), macOS | not built yet |
| Crypto in the tree | the engine's own (rustls on `aws-lc-rs`); GPUI adds none on the three desktops |
| `cargo deny` for this workspace | not set up |
| Packaging, signing, an installer, an update path | none |

## What comes next, in order

1. **One runtime for the engine.** The engine is `tokio` (two workers, as the
   command line starts it); GPUI runs its own executor. The app owns one tokio
   runtime on its own threads, a view sends it a request, and the answer comes
   back to the view through a channel. No engine future is polled on GPUI's
   main thread.
2. **`host::Front` for a view**: `Progress` into a model the view draws,
   `Ask` as a dialog that answers the engine's question, never a default yes.
3. **The first real view**: status (`snob status` reads no network), then the
   stored lists (`snapshots`, `changes`), which send nothing either. A walk
   comes after both.
4. Sign-in is the browser window snob already opens (`login`); the app starts
   it and never sees a password.
5. CI for this directory on the three desktops, `cargo deny`, then packaging.

The rules of the repository apply here unchanged: English, US spelling, few
comments, no test that reaches Instagram or the real keyring.
