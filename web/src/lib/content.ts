import { SITE } from './site.ts';
import type { TerminalLine } from './terminal.ts';
import { VERSION } from './version.ts';

/**
 * Every word on the page. The HTML, llms.txt and the Markdown twin all read it
 * from here, so a person and an agent read the same thing. Nothing here may
 * promise what the README does not: the commands and their wording come from
 * it and from `--help`.
 *
 * **What is released is the command line**, with the MCP server, the monitor
 * and the webhooks in it. The desktop app is in no release: `APP` says how
 * far it goes, and nothing here offers it for download.
 */

/** The systems the hero can tell apart. */
export type Platform = 'windows' | 'macos' | 'linux' | 'mobile';

/** Commands run one after the other, copied together. */
export interface InstallBlock {
  /** Which system, when a method has one block per system. */
  label?: string;
  commands: readonly string[];
}

/** A word of a line that links somewhere, such as a package manager's site. */
export interface TextLink {
  text: string;
  href: string;
}

/**
 * The line cut around the first `link.text` in it, so the word can be drawn
 * as a link in place; undefined when there is no link or the word is absent.
 */
export function splitAround(line: string, link: TextLink | undefined): [string, string] | undefined {
  if (!link) return undefined;
  const at = line.indexOf(link.text);
  return at < 0 ? undefined : [line.slice(0, at), line.slice(at + link.text.length)];
}

export interface InstallMethod {
  id: string;
  /** The tab's name. */
  name: string;
  /** Who it is for, one line under the tab. */
  note: string;
  /** The package manager, linked where its name appears in the note and the hero. */
  tool?: TextLink;
  /** Alternatives: each block is complete on its own. */
  blocks: readonly InstallBlock[];
  /** A page the method needs, such as the releases page for the .deb. */
  link?: { label: string; href: string };
}

const RELEASES = `${SITE.repository}/releases/latest`;

/**
 * Every way to install what is released: the command line. The package is
 * `snob` everywhere but Cargo, whose crate is `snob-cli`.
 */
export const INSTALL = [
  {
    id: 'homebrew',
    name: 'macOS · Linux',
    note: 'With Homebrew.',
    tool: { text: 'Homebrew', href: 'https://brew.sh/' },
    blocks: [{ commands: [`brew tap dennisgr7/snob ${SITE.repository}`, 'brew install snob'] }],
  },
  {
    id: 'scoop',
    name: 'Windows',
    note: 'With Scoop, on Windows 11.',
    tool: { text: 'Scoop', href: 'https://scoop.sh/' },
    blocks: [{ commands: [`scoop bucket add snob ${SITE.repository}`, 'scoop install snob'] }],
  },
  {
    id: 'deb',
    name: 'Debian · Ubuntu',
    note: 'The .deb for your architecture, downloaded from the releases page.',
    blocks: [
      { label: 'x86_64', commands: [`sudo apt install ./snob-v${VERSION}-x86_64-unknown-linux-musl.deb`] },
      { label: 'ARM64', commands: [`sudo apt install ./snob-v${VERSION}-aarch64-unknown-linux-musl.deb`] },
    ],
    link: { label: 'Releases', href: RELEASES },
  },
  {
    id: 'script',
    name: 'Script',
    note: 'Without a package manager.',
    blocks: [
      {
        label: 'macOS · Linux',
        commands: [`curl -fsSL ${SITE.raw}/packaging/install.sh | sh`],
      },
      {
        label: 'Windows 11 · PowerShell',
        commands: [`irm ${SITE.raw}/packaging/install.ps1 | iex`],
      },
    ],
  },
  {
    id: 'source',
    name: 'From source',
    note: 'With Rust and a C compiler.',
    blocks: [{ commands: [`cargo install --locked --git ${SITE.repository} snob-cli`] }],
  },
] as const satisfies readonly InstallMethod[];

type InstallId = (typeof INSTALL)[number]['id'];

const method = (id: InstallId): InstallMethod => INSTALL.find((m) => m.id === id)!;

/**
 * What the hero offers each system. Linux gets the script rather than
 * Homebrew, which many Linux machines do not have. A phone gets no command.
 */
export const HERO_INSTALL: Record<
  Exclude<Platform, 'mobile'>,
  { label: string; commands: readonly string[]; tool?: TextLink | undefined }
> = {
  macos: { label: 'macOS · Homebrew', commands: method('homebrew').blocks[0]!.commands, tool: method('homebrew').tool },
  windows: { label: 'Windows 11 · Scoop', commands: method('scoop').blocks[0]!.commands, tool: method('scoop').tool },
  linux: { label: 'Linux · install script', commands: method('script').blocks[0]!.commands },
};

/** The install tab selected first for each system, matching the hero. */
export const HERO_TAB: Record<Platform, InstallId> = {
  macos: 'homebrew',
  windows: 'scoop',
  linux: 'script',
  mobile: 'homebrew',
};

/** The link under the hero's command, down to every way to install. */
export const OTHER_WAYS = 'other ways';

/** README, under "Install", word for word. Windows 10 is not supported. */
export const SUPPORT =
  'Builds exist for Windows 11 and Linux on x86_64 and ARM64, and macOS on Apple Silicon. Everything that talks to Instagram needs a Chromium-based browser installed; no window is ever shown.';

export const ON_A_PHONE = 'Snob runs on your computer: Windows 11, macOS and Linux.';

export interface PlatformBuild {
  name: string;
  /** The architectures built for it; `arm` marks the native ARM build. */
  arches: readonly { name: string; arm?: boolean }[];
}

/** SUPPORT drawn as a table: every system, every build. */
export const PLATFORMS: readonly PlatformBuild[] = [
  { name: 'Windows 11', arches: [{ name: 'x86_64' }, { name: 'ARM64', arm: true }] },
  { name: 'macOS', arches: [{ name: 'Apple Silicon', arm: true }] },
  { name: 'Linux', arches: [{ name: 'x86_64' }, { name: 'ARM64', arm: true }] },
];

/** The ARM builds, said once: each is native, none runs under emulation. */
export const ARM = {
  label: 'ARM ready',
  text: 'A native ARM64 build on every system, nothing emulated: Apple Silicon Macs, Windows 11 on ARM laptops, and ARM64 Linux, Raspberry Pi included.',
} as const;

/**
 * The invitation to star the repository. No page can star it for the
 * visitor: that takes their GitHub session, so the link opens the repository
 * and its own Star button does the rest.
 */
export const STAR = {
  label: 'Star',
  where: 'on GitHub',
  line: 'Useful to you? A star on GitHub helps other people find it.',
  link: 'Star it on GitHub',
} as const;

/** The bar on top: the tools that have a section, and the install. */
export const NAV = {
  label: 'Sections',
  home: `${SITE.name}, home`,
  links: [
    { href: '/#command-line', label: 'command line', wide: true },
    { href: '/#agents', label: 'agents', wide: true },
    { href: '/#automations', label: 'automations', wide: true },
    { href: '/#install', label: 'install', wide: false },
  ],
} as const;

export const FOOTER = {
  disclaimer: 'Not affiliated with Instagram or Meta.',
  license: 'MIT',
  source: 'GitHub',
  index: 'llms.txt',
  by: 'by',
} as const;

/** Lines of terminal that show a command and what it printed. */
export type Sample = readonly TerminalLine[];

/** A term, most often a command or a flag, and what it means. */
export interface Point {
  term: string;
  means: string;
}

export const HERO = {
  /** The headline; `accent` closes it in the brand color. */
  title: "who doesn't follow you",
  accent: 'back?',
  lead: 'Snob knows. It is a native Instagram client for your desktop: an app that lives on your computer, and with it a command line, an MCP server for your AI agent, automations and webhooks.',
} as const;

/** The home page's title: the name, what it is, and the question. */
export const HOME_TITLE = `${SITE.name}, a native ${SITE.category}: ${HERO.title} back`;

/** Whether a part of snob is in a release, as the chip beside it says. */
export const STATUS = { released: 'released', building: 'being built' } as const;

/**
 * The desktop app, as far as it goes today (`apps/desktop`). **It is in no
 * release**, so `today` is said wherever the page shows an install, and the
 * page has no download for it. The day it ships: `released`, `description`
 * and `today` change here, and the hero offers it.
 */
export const APP = {
  name: 'The app',
  released: false,
  description:
    "A native window on your desktop, on the same engine as the tools. So far it signs you in, through a browser window of snob's own or with a pasted session ID, keeps several accounts, signs you out and has a command palette. The screens for what the tools already do are not there yet.",
  today:
    'The app is still being built and is in no release yet. What you can install today is its command line, with the MCP server, the monitor and the webhooks in it.',
  /** Above the hero's install, so the command is not taken for the app's. */
  heroNote: 'The app is still being built. What you can install today is its command line, with the MCP server and the monitor in it:',
} as const;

/** One of the tools that come with the client, and the section that shows it. */
export interface Tool {
  /** The anchor of its section. */
  id: string;
  name: string;
  /** How it is started. */
  command: string;
  released: boolean;
  description: string;
}

/** What snob is: one client, and the tools that are parts of it. */
export const CLIENT = {
  title: 'One client,',
  accent: 'four tools.',
  lead: 'snob is one client on your computer, signed in to Instagram as you. Its tools are not products of their own: each reaches the same accounts, spends the same request budget and stops at the same push-back.',
  toolsLabel: 'The tools',
  tools: [
    {
      id: 'command-line',
      name: 'The command line',
      command: 'snob',
      released: true,
      description:
        'Followers and following and how they cross, profiles, stories, highlights, posts and reels, and their photos and videos saved. Full-screen views on a terminal, JSON down a pipe.',
    },
    {
      id: 'agents',
      name: 'The MCP server',
      command: 'snob mcp',
      released: true,
      description:
        "The same commands for an AI agent, which waits for your yes before it walks another account's lists, follows or unfollows.",
    },
    {
      id: 'automations',
      name: 'Automations',
      command: 'snob watch',
      released: true,
      description:
        'The monitor: it reads the accounts you name on a schedule and reports who followed, who left and what was posted.',
    },
    {
      id: 'webhooks',
      name: 'Webhooks',
      command: 'snob watch --webhook <url>',
      released: true,
      description:
        'Each report posted to a server of yours, signed when you give it a key, or to Slack, Discord or ntfy as a message.',
    },
  ] as readonly Tool[],
} as const;

/**
 * The recording beside the headline (HeroVideo.astro): snob's own views and
 * its MCP server's answers on an invented Instagram, recorded by
 * `video/record.sh` and filmed by `scripts/video.mjs`, never a real account.
 * It shows two of the tools, not the app, and `caption` says so under it.
 * `description` is its text, for a screen reader and the Markdown twin: what
 * it shows, in order.
 */
export const HERO_VIDEO = {
  webm: '/video/hero.webm',
  mp4: '/video/hero.mp4',
  poster: '/video/hero.webp',
  /** The button under it: what it shows, and what a screen reader hears. */
  pause: { text: '[pause]', label: 'Pause the recording' },
  play: { text: '[play]', label: 'Play the recording' },
  caption: '# two of the tools at work, the command line and the MCP server, on an invented account',
  description:
    'A recording of two of snob’s tools, the command line and the MCP server, on an invented Instagram account. In the terminal, snob scan shows how its followers and following cross, opens the 14 accounts it follows that do not follow it back, and the profile of one of them; snob profile shows another account, its two stories up, and keeps one. Then, in a chat, an AI agent answers "Who doesn’t follow me back?" through snob’s MCP server, and unfollows one account once the person ticks "Go ahead" in the form snob asks with.',
} as const;

export interface Feature {
  /** The command, as typed after `snob`. */
  name: string;
  description: string;
  sample: Sample;
}

/**
 * The six cards. Output is what snob prints, with invented accounts; a line
 * starting with `#` is a comment, never output snob does not print.
 */
export const FEATURES: readonly Feature[] = [
  {
    name: 'unfollowers',
    description: 'You follow them; they do not follow you back.',
    sample: [
      { command: 'snob unfollowers' },
      { output: [{ text: '3 accounts you follow that do not follow you back - 3 of 87 - 9 requests', tone: 'dim' }] },
    ],
  },
  {
    name: 'profile',
    description: "An account's page, the way Instagram shows it.",
    sample: [{ command: 'snob profile someone' }, { output: 'Followers:    1234' }, { output: 'Following:    56' }],
  },
  {
    name: 'stories · highlights',
    description: 'See them and save them. A story is never marked as seen.',
    sample: [
      { command: 'snob stories someone -d all' },
      { output: [{ text: '# every story it has up, saved', tone: 'dim' }] },
    ],
  },
  {
    name: 'post · reel',
    description: 'See or download a post or reel, including every photo and video in it.',
    sample: [
      { command: 'snob reel <link> -d all' },
      { output: [{ text: '# every photo and video in it, saved', tone: 'dim' }] },
    ],
  },
  {
    name: 'watch',
    description: 'Keeps watching on a schedule and reports what changed.',
    sample: [
      { command: 'snob watch once' },
      { output: 'Changes for @me since Aug 14 at 09:12' },
      { output: [{ text: '  followers gained: 1', tone: 'ok' }] },
      { output: [{ text: '  followers lost: 1', tone: 'error' }] },
    ],
  },
  {
    name: 'follow · unfollow',
    description: 'The only two writes. One account at a time, after asking.',
    sample: [
      { command: 'snob unfollow someone' },
      { output: 'Unfollow @someone? [y/N] y' },
      { output: [{ text: 'No longer following @someone.', tone: 'ok' }] },
    ],
  },
];

/** The first tool, and the one the other three are commands of. */
export const COMMAND_LINE = {
  title: 'Everything you check by hand,',
  accent: 'in one command.',
  lead: 'The command line, snob, is the tool the other three are built on. On a terminal most commands open a full-screen view you move through with the arrow keys; snob --help lists everything.',
} as const;

/** The command line as a script reads it. */
export const SCRIPTS = {
  title: 'Same answer,',
  accent: 'for people and programs.',
  lead: 'Every command can print instead of opening a view, in a stable form a program can read. Down a pipe that is the default, and the default format is JSON.',
  /** The full-screen view, drawn without side borders so it lines up in any font. */
  human: {
    caption: '# on a terminal',
    label: 'The unfollowers view on a terminal',
    /** Said in the Markdown twin, where the frame is left out. */
    twin: 'On a terminal, the view (its frame left out):',
    lines: [
      { output: [{ text: '╭ unfollowers of @me — 3 accounts', tone: 'dim' }], art: true },
      { output: [{ text: '> ', tone: 'prompt' }, 'anna.example    Anna Example'] },
      { output: '  bob_builds      Bob' },
      { output: ['  camille.k       Camille     ', { text: 'verified', tone: 'dim' }] },
      { output: [{ text: '╰ ↑↓ move · enter open profile · / filter · q quit', tone: 'dim' }], art: true },
    ] as Sample,
  },
  machine: {
    caption: '# down a pipe',
    label: 'The same list as JSON, down a pipe',
    twin: 'Down a pipe:',
    lines: [
      { command: "snob unfollowers | jq '.[0]'" },
      { output: '{' },
      { output: [{ text: '  "pk"', tone: 'info' }, ': 1234567890,'] },
      { output: [{ text: '  "username"', tone: 'info' }, ': "anna.example",'] },
      { output: [{ text: '  "full_name"', tone: 'info' }, ': "Anna Example",'] },
      { output: [{ text: '  "is_private"', tone: 'info' }, ': false,'] },
      { output: [{ text: '  "is_verified"', tone: 'info' }, ': false'] },
      { output: '}' },
    ] as Sample,
  },
  flags: {
    title: 'Flags for scripts',
    points: [
      { term: '--format json|ndjson|csv|xlsx|md|table', means: 'a list or a document, in the form you need' },
      { term: '-o <file>', means: 'write it to a file' },
      { term: '--json', means: 'a status object, for commands such as whoami' },
      { term: '--no-interactive', means: 'always print, never open a view' },
      { term: '-y', means: 'answer the one confirmation in advance' },
    ] as readonly Point[],
  },
  exitCodes: {
    title: 'Exit codes',
    points: [
      { term: '0', means: 'it worked' },
      { term: '1', means: 'it failed' },
      { term: '2', means: 'the command line could not be parsed' },
      { term: '3', means: 'log in again' },
      { term: '4', means: 'the account needs verifying' },
      { term: '5', means: 'rate limited or cooling down' },
      { term: '130', means: 'stopped' },
    ] as readonly Point[],
  },
} as const;

/** README, the `snob mcp` section, in short. */
export const AGENTS = {
  title: 'The same client,',
  accent: 'for your AI agent.',
  lead: 'snob mcp serves the same commands to an AI agent: Claude Code, Claude Desktop, Codex, Cursor or VS Code. Each call runs one snob command, under the same request budget and cooldown as your terminal.',
  points: [
    { term: 'snob_status · snob_list · snob_profile', means: 'the status, the lists and how they cross, and profiles: reads only, unless you start it with --allow-writes' },
    { term: 'your yes', means: "walking another account's lists, and every follow or unfollow, waits for it, asked in the client or put to you in the chat" },
    { term: 'snob_job', means: 'a long walk runs as a job the agent asks about later, instead of holding the call for an hour' },
    { term: 'snob mcp --listen', means: 'the same over HTTP on loopback, behind a key, for a tunnel of your own' },
  ] as readonly Point[],
  /** The real output, its path and its closing hint shortened. */
  sample: {
    label: 'Printing the MCP configuration for Claude Code',
    lines: [
      { command: 'snob mcp config claude-code' },
      { output: '{' },
      { output: [{ text: '  "mcpServers"', tone: 'info' }, ': {'] },
      { output: [{ text: '    "snob"', tone: 'info' }, ': {'] },
      { output: [{ text: '      "args"', tone: 'info' }, ': ["mcp"],'] },
      { output: [{ text: '      "command"', tone: 'info' }, ': "…/snob"'] },
      { output: '    }' },
      { output: '  }' },
      { output: '}' },
      { output: [{ text: 'also claude-desktop, codex, cursor and vscode', tone: 'dim' }] },
    ] as Sample,
  },
} as const;

/**
 * The monitor and its webhooks: `snob watch --help` and the README's
 * "Watching accounts and webhooks", in short. The lines starting with `#`
 * are comments, as in the cards.
 */
export const AUTOMATIONS = {
  title: 'Set a schedule,',
  accent: 'and get told what changed.',
  lead: 'The monitor is snob watch: it reads the accounts you name on a schedule and reports who followed, who left, who was renamed and what was posted since the last report. A webhook takes each report to a server of yours.',
  monitor: {
    title: 'The monitor',
    points: [
      { term: 'snob watch --every 6h', means: 'stays up and runs on a schedule: an interval, times of day, days of the week, or a cron expression you already have' },
      { term: 'snob watch once', means: 'one run, for cron, a systemd timer or Windows Task Scheduler; a run with nothing to report costs one request for your own account' },
      { term: '[account.posts]', means: "in watch.toml, reports an account's new posts too, and with save_to keeps their photos and videos in a directory you name" },
      { term: 'snob watch check', means: 'checks the schedule, the session, the accounts and the webhook while you are still there to fix them' },
    ] as readonly Point[],
    sample: {
      label: 'Setting the monitor up',
      lines: [
        { command: 'snob watch setup' },
        { output: [{ text: '# asks how often, where to send the reports and which accounts,', tone: 'dim' }] },
        { output: [{ text: '# then writes watch.toml for you to edit', tone: 'dim' }] },
        { command: 'snob watch unit' },
        { output: [{ text: '# prints a systemd timer, a crontab line, a launch agent or a', tone: 'dim' }] },
        { output: [{ text: '# Windows scheduled task to run it with; installs nothing', tone: 'dim' }] },
      ] as Sample,
    },
  },
  webhooks: {
    /** The anchor the client's map points at. */
    id: 'webhooks',
    title: 'Webhooks',
    points: [
      { term: 'snob watch --webhook <url>', means: 'posts each report as JSON, and sends it again until your server answers 2xx, for up to a week' },
      { term: '--webhook-format slack|discord|ntfy|text', means: 'sends the summary alone, in the shape that service takes' },
      { term: 'X-Snob-Signature', means: 'with a signing key, the HMAC-SHA256 of the exact bytes of the body; snob watch setup keeps the key in the system keyring' },
      { term: 'X-Snob-Delivery', means: "the run's id: a report may arrive twice, so deduplicate on it" },
      { term: 'snob watch deliver', means: 'sends what a receiver that was down is still owed, with no request to Instagram' },
    ] as readonly Point[],
    /** The README's report, cut to its first keys. */
    sample: {
      label: 'What a webhook receives, shortened',
      lines: [
        { output: [{ text: '# what your server receives, shortened', tone: 'dim' }] },
        { output: [{ text: 'X-Snob-Event', tone: 'info' }, ': watch.changes'] },
        { output: [{ text: 'X-Snob-Delivery', tone: 'info' }, ': f1e2d3c4b5a69788-42'] },
        { output: [{ text: 'X-Snob-Signature', tone: 'info' }, ': sha256=<hex>'] },
        { output: '{' },
        { output: [{ text: '  "schema"', tone: 'info' }, ': 1,'] },
        { output: [{ text: '  "event"', tone: 'info' }, ': "watch.changes",'] },
        { output: [{ text: '  "run"', tone: 'info' }, ': { "id": "f1e2d3c4b5a69788-42", … },'] },
        { output: [{ text: '  "counts"', tone: 'info' }, ': { "followers_gained": 1, "posts_new": 1, … },'] },
        { output: [{ text: '  …', tone: 'dim' }] },
        { output: '}' },
      ] as Sample,
    },
  },
} as const;

export const BEHAVIOR = {
  title: 'A light client,',
  accent: 'on purpose.',
  lead: "Whichever tool asks, snob signs in as you and sends its requests from a Chrome, Edge, Brave or Chromium already on your machine, at a human pace. It never stores your password, and it never reads your own browser's cookies.",
  /** README, "Staying a light client", word for word. */
  honest:
    "There is no official API for most of this, so snob uses the same web API instagram.com uses, signed in as you. That is outside Instagram's Terms of Use, and the usual consequence is that Instagram asks the account to verify itself. snob keeps to modest volumes, sends only the web app's own requests, and stops at the first sign of push-back. Use it from the connection you normally browse from.",
  label: 'How snob behaves',
  checks: [
    { output: [{ text: '✓ ', tone: 'ok' }, '12 accounts a page, 1 to 3 seconds apart'] },
    { output: [{ text: '✓ ', tone: 'ok' }, 'a rest of 5 to 15 minutes every 40 pages'] },
    { output: [{ text: '✓ ', tone: 'ok' }, 'two writes, ever: follow and unfollow, after asking'] },
    { output: [{ text: '✓ ', tone: 'ok' }, 'stories are never marked as seen'] },
    { output: [{ text: '✓ ', tone: 'ok' }, 'the session is stored on your machine, in the system keyring when there is one'] },
    { output: [{ text: '! ', tone: 'warn' }, 'the first push-back stops it, and it cools down'] },
  ] as Sample,
} as const;

/** `today` is what the methods install while the app is in no release. */
export const INSTALL_SECTION = {
  title: 'Install',
  accent: 'it.',
  lead: SUPPORT,
  today:
    'Every way below installs the command line, which carries the MCP server and the monitor. The app is in no release yet, so there is nothing of it to download.',
} as const;

/** What llms.txt says of each link it lists. */
export const LLMS = {
  home: 'what snob is, what its command line, its MCP server and its monitor do, how it behaves, and how to install it.',
  repository: 'source code, releases and the README.',
} as const;
