# snob

Native Instagram client for the desktop, with a command line, an MCP server
for AI agents, automations and webhooks.

snob signs in as you and shows who does not follow you back, browses profiles,
stories, highlights, posts and reels, and downloads their photos and videos. Its
tools around that are snob's command line (`snob`, below), an MCP server that
lets AI agents use it under your rules, a monitor (`snob watch`) that reports
what changed in an account on a schedule, and webhooks that carry those reports
to your own server. It sends its requests from a Chrome, Edge, Brave or Chromium
already on your machine, at a human pace. It only ever changes two things,
`follow` and `unfollow`, one account at a time and after asking; it never marks
a story as seen.

> [!NOTE]
> The desktop app is not released yet. It signs in, handles several accounts,
> signs out and has a command palette, and its home screen is still a title.
> What you can install today is the command line and the MCP server, which are
> the same `snob` program.

- [Install](#install)
- [Getting started](#getting-started)
- [Commands](#commands)
- [Scripts and automation](#scripts-and-automation)
- [Watching accounts and webhooks](#watching-accounts-and-webhooks)
- [AI agents: `snob mcp`](#ai-agents-snob-mcp)
- [Staying a light client](#staying-a-light-client)

## Install

**Windows** ([Scoop](https://scoop.sh)):

```bash
scoop bucket add snob https://github.com/dennisgr7/snob
scoop install snob
```

**macOS and Linux** ([Homebrew](https://brew.sh)):

```bash
brew tap dennisgr7/snob https://github.com/dennisgr7/snob
brew install snob
```

**Debian and Ubuntu**: the `.deb` for your architecture is on the
[releases page](https://github.com/dennisgr7/snob/releases).

```bash
sudo apt install ./snob-v<version>-x86_64-unknown-linux-musl.deb
```

**Without a package manager**:

```bash
curl -fsSL https://raw.githubusercontent.com/dennisgr7/snob/main/packaging/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/dennisgr7/snob/main/packaging/install.ps1 | iex
```

The scripts read `SNOB_EDITION`. `cli` installs the command line and the MCP
server alone. `desktop` installs the app beside them as well, and stops with an
error when the release does not list it, as no release does yet. Without it the
script installs the app where a release lists it, and the command line
otherwise, which is what it does today.

**From source** (Rust and a C compiler):

```bash
cargo install --locked --git https://github.com/dennisgr7/snob snob-cli
```

Builds exist for Windows 11 and Linux on x86_64 and ARM64, and macOS on Apple
Silicon. Everything that talks to Instagram needs a Chromium-based browser
installed; no window is ever shown.

### Updating

`snob update` updates snob the way it was installed: through Homebrew, Scoop,
WinGet or Cargo, by replacing itself where the install script put it, or, for
the `.deb`, with the `apt` command it runs or prints.

On its own, snob also asks GitHub once a day whether a release is out,
downloads it (checked against the release's `SHA256SUMS`), and applies it the
next time it starts with no other snob running, or after a restart.

| Command | Does |
|---|---|
| `snob update --auto off\|notify\|download\|apply` | how much of that it does |
| `snob update --status` | what it has done |
| `SNOB_NO_UPDATE_CHECK=1` | turns it off |

None of it talks to Instagram.

## Getting started

```bash
snob login      # sign in through a browser snob opens, or paste a sessionid
snob whoami
```

Over SSH, where no window can reach you, `snob login --remote` prints an `ssh`
command that opens a tunnel from your computer and a link to open there: you log
in on that page, in snob's browser seen from afar, and the session stays on the
machine snob runs on.

> [!NOTE]
> snob never sees your password when you log in in a window or paste a session,
> and it never reads your own browser's cookies. With `--remote` what you type
> on that page is passed through snob to its browser, and kept nowhere.

## Commands

```bash
snob unfollowers                 # you follow them, they do not follow you back
snob fans                        # they follow you, you do not follow them
snob friends                     # you follow each other
snob followers someone           # any list, yours or another account's
snob profile someone             # the profile page: counts, bio, mutuals, highlights
snob pfp someone                 # the profile picture at full size
snob stories someone -d all      # save every story an account has up
snob highlights someone 2 -d all # save everything in its second highlight
snob posts someone               # the posts on its grid
snob posts someone --tab reels   # its reels; or tagged, or reposts
snob post <link> -d all          # one post by its link, every photo and video in it
snob post <link> --comments 5    # the post and five pages of its comments
snob post <link> --replies 3     # and the replies under the first three that have more
snob post <link> --replies-of <comment-pk>  # the replies under one comment, by its id
snob reel <link> -d all          # the same for a reel
snob likers <link>               # who liked a post, as many as Instagram shows
snob fetch <cdn-address>         # one file by its address, no account needed
snob follow someone
snob unfollow someone
snob watch                       # keep watching on a schedule, report changes and new posts
snob status                      # budget left, cooldown, stored lists, agents; sends nothing to Instagram
snob snapshots                   # the captures of your lists stored here; sends nothing to Instagram
snob changes --since 7d          # who joined, left or was renamed between two of them
snob followers --background      # walk as a job and get its id at once; any list command
snob jobs                        # walks left running, and "snob jobs cancel <id>"
```

On a terminal most commands open a full-screen view you move through with the
arrow keys. `snob --help` and `snob <command> --help` list everything.

## Scripts and automation

Every command can print instead of opening a view, in a stable form a program
can read.

### Output

- `--format json|ndjson|csv|xlsx|md|table` and `-o <file>` for anything that
  prints a list or a document; `--json` for status commands such as `whoami`
  and `status`, and for `follow` and `unfollow`, whose object says what
  happened in one word (`followed`, `requested`, `already_following`, ...).
- `--no-interactive` always prints. Down a pipe this is the default, and the
  default format is JSON.
- `-o -` is standard output: a list as with no `-o`, and a single downloaded
  file, as in `snob reel <link> -d 1 -o - | ffmpeg -i - ...`, which is the
  app's 720-wide MP4.
- `-y` answers the one confirmation a command may ask, in advance.

### Budget and waiting

- `snob status --budget` says what can still be sent today, for no request, and
  exits 5 while the account is cooling down. `--dry-run` on a list command says
  what its walks would cost against that, and sends nothing.
- `snob status --answers` says how long Instagram's answers took in the last
  24 hours. When the latest of them are slow, snob waits longer between pages
  (at least twice their median, at most a minute); it never goes faster.
- `--max-wait <duration>` stops a command, with exit 130, rather than let it
  wait longer than that for the request budget; a list stopped this way
  continues where it left off next time.

### Exit codes

Exit codes are stable. When the output was going to be JSON, a failure is also
one JSON object on the last line of standard error.

| Code | Means |
|---|---|
| 0 | worked |
| 1 | failed |
| 2 | bad command line |
| 3 | log in again |
| 4 | the account needs verifying |
| 5 | rate limited or cooling down |
| 130 | stopped |

### Videos at their best

A post's or a reel's video is saved at its best, usually 1080 wide, when
[ffmpeg](https://ffmpeg.org/) is installed (`winget install Gyan.FFmpeg`,
`scoop install ffmpeg`, `brew install ffmpeg`, or your package manager): snob
joins its picture and its sound into one MP4 without re-encoding. The picture
is VP9, which QuickTime does not play; `--quality compat` saves the app's
720-wide MP4, as snob does when ffmpeg is not there. `--ffmpeg <path>` or
`SNOB_FFMPEG` names another ffmpeg.

## Watching accounts and webhooks

- `snob watch --webhook <url>` posts each report as JSON, with a `summary`
  sentence and ISO dates beside the epoch seconds; `format = "slack"` (or
  `discord`, `ntfy`, `text`) under `[webhook]` in `watch.toml`, or
  `--webhook-format`, sends the summary alone, in the shape that service
  takes. `--json` writes the reports as lines instead.
- `snob watch deliver` sends what a receiver that was down is still owed, at
  once, with no request to Instagram.
- An `[[account]]` in `watch.toml` can watch its new posts beside its lists, or
  instead of them: `[account.posts]` reports each post published since the last
  run, and `save_to = '<directory>'` under it keeps their photos and videos
  there (`lists = false` leaves its followers alone, which asks no consent).
  The first run on an account's posts reports none; each run after costs its
  profile and a page of its grid, at most once an hour.
- `snob watch unit` prints what to give the system's scheduler (a systemd
  timer, a crontab line, a launch agent or a Windows scheduled task, as
  PowerShell commands; installs nothing). Each runs `snob watch once
  --when-due` every 15 minutes, which runs only when the schedule in
  `watch.toml` says so and otherwise sends nothing.

### What a webhook receives

Each report is one POST, sent again until a 2xx comes back (for a week, and
once a run), so a receiver may see one twice: **deduplicate on `run.id`**,
which is also the `X-Snob-Delivery` header. `X-Snob-Event` says which event
it is without reading the body (`watch.changes`, `watch.heartbeat`, or
`watch.preflight` from `snob watch check`), `X-Snob-Attempt` which try this
is, and with a signing key `X-Snob-Signature: sha256=<hex>` is the
HMAC-SHA256 of the exact bytes of the body under that key. The `User-Agent`
is `snob/<version>`. With `format = "json"`, the default, the body is the
report below plus a `summary`; the other formats send the summary alone.
`snob watch --json >> events.ndjson` writes the same report as one line per
account and run, without `run.id`.

<details>
<summary>A report, in full</summary>

```json
{
  "schema": 1,
  "event": "watch.changes",
  "run": {
    "id": "f1e2d3c4b5a69788-42",
    "at": 1763196400,
    "at_iso": "2025-11-15T08:46:40Z",
    "looked": true,
    "requests": 6,
    "lists": [
      { "kind": "followers", "skipped": null },
      { "kind": "following", "skipped": null }
    ],
    "posts": { "skipped": null },
    "due_at": 1763190000,
    "due_at_iso": "2025-11-15T07:00:00Z",
    "began_at": 1763196380,
    "began_at_iso": "2025-11-15T08:46:20Z",
    "ended_at": 1763196400,
    "ended_at_iso": "2025-11-15T08:46:40Z",
    "late_by": 6380,
    "late": true,
    "tool": { "name": "snob", "version": "0.9.0" }
  },
  "viewer": { "pk": 42, "username": "me" },
  "account": { "pk": 42, "username": "me", "is_self": true },
  "lists": {
    "followers": {
      "basis": "compared",
      "since": 1763110000,
      "since_iso": "2025-11-14T08:46:40Z",
      "until": 1763196400,
      "until_iso": "2025-11-15T08:46:40Z",
      "count": 310
    },
    "following": null
  },
  "posts": {
    "basis": "compared",
    "since": 1763110000,
    "since_iso": "2025-11-14T08:46:40Z",
    "until": 1763196400,
    "until_iso": "2025-11-15T08:46:40Z",
    "pages_read": 1,
    "complete": true
  },
  "counts": {
    "followers_gained": 1,
    "followers_lost": 0,
    "following_gained": 0,
    "following_lost": 0,
    "renamed": 0,
    "posts_new": 1,
    "total": 1
  },
  "events": {
    "followers_gained": [
      { "pk": 1001, "username": "someone", "profile_url": "https://www.instagram.com/someone/" }
    ],
    "followers_lost": [],
    "following_gained": [],
    "following_lost": [],
    "renamed": [],
    "posts_new": [
      {
        "pk": "3800000000000000001",
        "code": "DSs0gAAAAAB",
        "url": "https://www.instagram.com/p/DSs0gAAAAAB/",
        "kind": "photo",
        "taken_at": 1763190000,
        "taken_at_iso": "2025-11-15T07:00:00Z",
        "pinned": false,
        "item_count": 1,
        "caption": "Open day this Friday",
        "saved": null
      }
    ]
  }
}
```

</details>

`counts.total` counts the lists' changes and the renames, not the new posts,
which `counts.posts_new` counts beside it; `event` is `watch.changes` when
either has news. A list or the posts that could not be read this run say why
in `run.lists[].skipped` and `run.posts.skipped`, so an empty report is told
from one that could not look. `due_at` is `null` for a run started by hand,
and `late` says a run began more than half an hour past its moment, beyond
the jitter it was allowed. New fields may be added; `schema` changes only
when one is removed or changes meaning.

## AI agents: `snob mcp`

`snob mcp` is an MCP server for AI agents, on standard input and output. Each
call runs one snob command, under the same budget and cooldown as the terminal.

### Quick start

```bash
snob mcp config claude-code      # or claude-desktop, codex, cursor, vscode
```

It prints what to add to that client's configuration.

### What an agent can do

| Tool | What it does | Asks you first |
|---|---|---|
| `snob_status` | the status: budget, cooldown, stored lists | no |
| `snob_list` | followers, following, unfollowers, fans, friends, optionally filtered (verified, private, no picture) | for another account's lists |
| `snob_job`, `snob_jobs`, `snob_job_cancel` | check on, list and stop a walk running as a job | no |
| `snob_profile` | a profile | no |
| `snob_posts` | the posts on a grid, or a profile's reels, tagged posts and reposts | no |
| `snob_post` | one post or reel, with more pages of its comments and the replies under them | no |
| `snob_post_likers` | who liked a post | yes |
| `snob_stories`, `snob_highlights` | the stories an account has up, the highlights it keeps | no |
| `snob_snapshots`, `snob_changes` | the captures earlier walks stored, and what changed between two of them | no |
| `snob_follow`, `snob_unfollow` | follow and unfollow, with `--allow-writes` | yes |
| `snob_post_download` | save a post's photos and videos, with `--downloads-dir` | no |
| `snob_stories_download`, `snob_highlights_download` | save the stories an account has up, or what one highlight holds, with `--downloads-dir` | no |
| `snob_logout` | sign one account out of snob | yes, naming the account |
| `snob_accounts` | the accounts signed in here, with `--any-account` | no |

- Posts, stories and highlights come back without their picture and video
  addresses; an agent that asks is handed a small picture of each to look at, a
  video's cover for a video.
- A post can be read with more pages of its comments and the replies under them,
  the first threads with more than shown or comments named by their id, within
  one bounded answer that says what it left out.

> [!IMPORTANT]
> An agent never signs in: that stays yours. It never purges, never runs the
> monitor, and never writes a file anywhere but in `--downloads-dir`.

### Writes and your yes

A follow or an unfollow, signing out, walking another account's lists and
reading who liked a post run only after a yes for that one call:

- asked in the client, when it can show a form, or
- put to you by the agent in the chat. `--confirm-in-chat none|lists|all` says
  what that may cover; `all` by default.

> [!WARNING]
> snob cannot tell whether that yes was yours or the agent's, so keep the writes
> on "ask" in the client's tool permissions.

`--allow-writes` offers follow and unfollow. Agents may make at most ten
follows and unfollows a day on the account (`--max-writes-per-day` to change
it, up to 99); what you type in a terminal is not counted.

### Long lists and jobs

- Calls run a few at a time, and only one that can reach Instagram: another
  comes back busy rather than waiting.
- A long list comes back a page at a time (up to 200 accounts, without their
  pictures' addresses), and the later pages are handed back from what was
  already read, sending nothing; one given a limit still says how many accounts
  the whole list has.
- A list the agent asks for that has to be walked runs as a job: answered in the
  call when it ends within `wait_seconds` (20 by default, at most 50), and
  otherwise handed back as a job the agent asks about with `snob_job`, lists
  with `snob_jobs` and stops with `snob_job_cancel`. A client that supports MCP
  Tasks gets the job as a task instead, which it polls and can cancel on its
  own.
- A job waits at most an hour for the request budget unless the server was
  given `--max-wait`. One runs at a time per account, listed by `snob jobs` and
  in `snob status`, stopped like Ctrl+C by `snob jobs cancel <id>`, and stopped
  by `snob logout` and `snob purge` before they remove anything.

### Downloads

With `--downloads-dir <dir>`, the agent can save a post's photos and videos, the
stories an account has up, and what a highlight holds into that directory: the
agent names the post, the account or the highlight, never a path, the files get
the names snob gives every download (a story's or a highlight item's after its
id, so the name keeps meaning the same story), and none already there is
written over.

### Several accounts

The server acts as the active account, or the one `--account` names.
`--any-account` lets the agent see the accounts signed in here and say, call by
call, which one it acts as. It can sign one account out of snob after a yes
that names it, and never signs in.

### What agents did

`snob status --agents` lists what agents were told yes to in the last 24
hours.

### Upgrading

After upgrading snob, reconnect the server in the client: a server whose binary
changed under it refuses every call.

### Serving over HTTP

`snob mcp --listen 127.0.0.1:8750` serves the same over HTTP on this machine's
loopback, for a tunnel of your own (`ssh -L`, cloudflared, a VPN) to reach from
elsewhere. It refuses any address that is not loopback and any request without
the key (`Authorization: Bearer`).

**The key.** `snob mcp key new` makes the key a client presents to reach the
server over the network and keeps it in the system keyring, printing it once to
your terminal; `show` names it by a fingerprint, `revoke` removes it. On a
machine with no keyring, `SNOB_MCP_KEY` in the server's environment takes its
place (`snob mcp key new --env` makes one for it, keeping it nowhere).

**Yes over HTTP.** A yes given in the chat covers only reading lists unless
`--confirm-in-chat all`, though whoever holds the key can still confirm the
writes in a form it answers itself, since it is the client. A client on the
stateless revision of MCP has no session, so a chat yes it is handed is bound to
the call alone, and any client with the key may redeem it once.

> [!WARNING]
> TLS ends at the tunnel, whose provider sees the key and every answer, other
> people's names and lists included; put an access layer in front where the
> tunnel offers one.

> [!CAUTION]
> The server does not prove itself to its clients: they send the key to
> whatever answers on the port, so another user of the machine who listens
> there while snob is not running is handed it. Serve it only on a machine where
> nobody else can run code, and if the port could have been taken, revoke the
> key and make a new one (for a key in `SNOB_MCP_KEY`, put a new one there
> instead: revoking touches only the keyring).

**A socket instead of a port.** On Linux and macOS, `snob mcp --listen socket`
closes that gap: it serves on a socket in snob's private data directory
(`mcp.sock`, `0600`) that only you and root can connect to and nobody else can
take, and the tunnel reaches it there (`ssh -N -o ExitOnForwardFailure=yes -L
127.0.0.1:8750:<path>` from the far end, which stops if that port is taken
there, or cloudflared's `unix:<path>` service); startup prints the path.

**Clients of a remote server.**

```bash
snob mcp config codex --remote https://your-tunnel/mcp   # or any other client
```

It prints what a client needs to reach that server: no command and no key. The
client sends the key from `SNOB_MCP_TOKEN` in its own environment; never set
`SNOB_MCP_KEY` there, since it is the server's and replaces the key of any snob
started where it is set. Claude Desktop and claude.ai take the address as a
custom connector instead (Customize > Connectors), which they reach from
Anthropic's side, so it is the tunnel's https address, with the key under the
form's Request headers where your organization has them.

## Staying a light client

There is no official API for most of this, so snob uses the same web API
instagram.com uses, signed in as you. That is outside Instagram's Terms of Use,
and the usual consequence is that Instagram asks the account to verify itself.
snob keeps to modest volumes, sends only the web app's own requests, and stops
at the first sign of push-back. Use it from the connection you normally browse
from.

To remove everything snob stored on the machine, sessions included, run
`snob purge` before uninstalling.

## License

MIT.
