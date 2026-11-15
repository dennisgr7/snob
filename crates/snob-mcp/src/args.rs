//! `snob mcp`'s arguments, which the command line embeds as its `mcp`
//! command.

use clap::{Args, Subcommand, ValueEnum};

/// `snob mcp`: serve, or print what a client needs to start it.
#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: Option<McpCommand>,

    /// Offer follow and unfollow as well, each only after a yes for that call,
    /// asked in the client or by the agent in the chat
    #[arg(long)]
    pub allow_writes: bool,

    /// What a yes the agent relays from the chat may confirm, for a client
    /// that cannot show a form: none, reading other accounts' lists, or that
    #[cfg_attr(
        feature = "http",
        doc = " and the writes too [default: all; lists with --listen]"
    )]
    #[cfg_attr(not(feature = "http"), doc = " and the writes too [default: all]")]
    //
    // An `Option` rather than a default value, so that whoever reads it can
    // tell "not given" from "all": over HTTP the default is narrower
    // (`http::serve`), and one value for both would hide that the
    // choice was never made.
    #[arg(long, value_enum, value_name = "WHAT")]
    pub confirm_in_chat: Option<ConfirmInChat>,

    /// Serve over HTTP on this address of this machine's loopback, such as
    /// 127.0.0.1:8750, or on Linux and macOS on "socket", a socket in snob's
    /// data directory only you can reach, for a tunnel of your own; clients
    /// present the key "snob mcp key new" makes
    #[cfg(feature = "http")]
    #[arg(
        long,
        value_name = "ADDRESS",
        value_parser = crate::http::listen_at
    )]
    pub listen: Option<crate::http::ListenAt>,

    /// With --listen, the https address the tunnel serves this at, so that
    /// requests addressed to it are taken
    #[cfg(feature = "http")]
    #[arg(
        long,
        requires = "listen",
        value_name = "URL",
        value_parser = crate::http::public_url
    )]
    pub public_url: Option<url::Url>,

    /// The follows and unfollows agents may make in any 24 hours on the
    /// account, with --allow-writes [default: 10; at most 99, what the write
    /// budget allows in a day anyway]
    #[arg(
        long,
        requires = "allow_writes",
        value_name = "N",
        value_parser = clap::value_parser!(u32)
            .range(1..=i64::from(snob_store::store::mcp_acts::MOST_WRITES_A_DAY))
    )]
    pub max_writes_per_day: Option<u32>,

    /// Let the agent save posts' photos and videos into this directory,
    /// which has to exist. The agent never chooses where a file goes or
    /// what it is called, and nothing already there is written over
    #[arg(long, value_name = "DIR", value_parser = downloads_dir)]
    pub downloads_dir: Option<String>,

    /// Let the agent see which accounts are signed in here and say, call by
    /// call, which one it acts as. Without it the server acts as the active
    /// account, or the one --account names, and no other
    // Not with `--account`, which holds the server to one
    // (`Cli::refuse_a_named_account`).
    #[arg(long)]
    pub any_account: bool,
}

/// `snob mcp --downloads-dir`: a directory that is there, as its absolute
/// path, refused by clap (exit 2) otherwise. **It has to exist**: one made
/// from a mistyped path would be files nobody finds, and making directories
/// is not something a server started in the background should do. In UTF-8,
/// since it becomes an argument of every download's command line
/// (`tools::download`).
fn downloads_dir(value: &str) -> Result<String, String> {
    let path = std::path::Path::new(value);
    if !path.is_dir() {
        return Err("that is not a directory that exists; make it first".into());
    }
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    absolute
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| "its path is not valid UTF-8".to_string())
}

/// What `snob mcp` takes a yes in the chat for (`Server::consent`).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmInChat {
    /// Nothing: a call that needs a yes needs a client that can show a form.
    None,
    /// Walking another account's lists, and not the writes.
    Lists,
    /// The walks and the writes.
    All,
}

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// Print what to add to a client's configuration to start this server,
    /// or to reach one already serving over HTTP
    #[command(
        after_help = "Printed, never written: the snippet goes to standard output, and \
                      where it goes to standard error.\n\n\
                      --remote <URL> configures the client to reach a \"snob mcp --listen\" \
                      already running, at the https address your tunnel serves it at (or \
                      http on this machine's loopback, for ssh -L), instead of starting one. \
                      The snippet holds no key and starts nothing: the client sends the key \
                      from SNOB_MCP_TOKEN in its own environment, which you set to what \
                      \"snob mcp key show --reveal\" prints on the server's machine. Never \
                      set SNOB_MCP_KEY for a client: that is the server's, and it replaces \
                      the key of any snob started where it is set. The server's flags \
                      (--allow-writes, --confirm-in-chat, --max-writes-per-day) go on its own \
                      command line, and are refused here with --remote. Claude Desktop and \
                      claude.ai take a remote server as a custom connector in their \
                      settings, so for them only the address is printed."
    )]
    Config(McpConfigArgs),

    /// Keep the key a client presents to reach this server over the
    /// network, in the system keyring
    #[command(
        subcommand,
        after_help = "One key, kept in the system keyring under its own entry, whatever \
                      --no-keyring says: there is no file form. On a machine with no keyring \
                      (a server, a container), set SNOB_MCP_KEY in the server's environment \
                      instead, to a key \"snob mcp key new\" printed; it wins over the \
                      keyring wherever it is set, so set it only where the server runs. \
                      Only a key in snob's own form is taken: snob_mcp_ and 43 characters, \
                      256 random bits.\n\n\
                      The key is printed only to a terminal, unless --print says otherwise, \
                      so that a program running this does not keep it in what it reads. \
                      \"snob mcp key new --force\" replaces it, and every client given the \
                      old one stops working; \"snob mcp key revoke\" removes it. A server \
                      already running keeps the key it started with until it is restarted. \
                      \"snob purge\" removes it too."
    )]
    #[cfg(feature = "http")]
    Key(McpKeyCommand),
}

/// `snob mcp key` (`key.rs`).
#[cfg(feature = "http")]
#[derive(Subcommand, Debug)]
pub enum McpKeyCommand {
    /// Make a key, keep it in the system keyring, and print it this once
    New(McpKeyNewArgs),
    /// Say where the key is read from, and its fingerprint
    Show(McpKeyShowArgs),
    /// Remove the key from the system keyring
    Revoke,
}

#[cfg(feature = "http")]
#[derive(Args, Debug)]
pub struct McpKeyNewArgs {
    /// Replace the key there is; every client given it stops working
    #[arg(long)]
    pub force: bool,

    /// Print the key even when standard output is not a terminal
    #[arg(long)]
    pub print: bool,

    /// Make a key for SNOB_MCP_KEY: printed, kept nowhere, and the system
    /// keyring left as it is
    #[arg(long, conflicts_with = "force")]
    pub env: bool,
}

#[cfg(feature = "http")]
#[derive(Args, Debug)]
pub struct McpKeyShowArgs {
    /// Print the key itself, on a terminal
    #[arg(long)]
    pub reveal: bool,

    /// With --reveal, print it even when standard output is not a terminal
    #[arg(long, requires = "reveal")]
    pub print: bool,
}

#[derive(Args, Debug)]
pub struct McpConfigArgs {
    /// The client that will start it
    #[arg(value_enum)]
    pub client: McpClient,

    /// Start it with --allow-writes
    #[arg(long)]
    pub allow_writes: bool,

    /// Start it with --confirm-in-chat
    #[arg(long, value_enum, value_name = "WHAT")]
    pub confirm_in_chat: Option<ConfirmInChat>,

    /// Reach a server already serving over HTTP at this address rather
    /// than start one; the client sends the key from SNOB_MCP_TOKEN
    //
    // The server's flags describe a process this does not start, so they
    // are refused rather than printed somewhere they do nothing.
    #[arg(
        long,
        value_name = "URL",
        value_parser = crate::config::RemoteUrl,
        conflicts_with_all = [
            "allow_writes",
            "confirm_in_chat",
            "max_writes_per_day",
            "downloads_dir"
        ]
    )]
    pub remote: Option<url::Url>,

    /// Start it with --downloads-dir
    #[arg(long, value_name = "DIR", value_parser = downloads_dir)]
    pub downloads_dir: Option<String>,

    /// Start it with --any-account
    #[arg(long, conflicts_with = "remote")]
    pub any_account: bool,

    /// Start it with --max-writes-per-day
    #[arg(
        long,
        requires = "allow_writes",
        value_name = "N",
        value_parser = clap::value_parser!(u32)
            .range(1..=i64::from(snob_store::store::mcp_acts::MOST_WRITES_A_DAY))
    )]
    pub max_writes_per_day: Option<u32>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpClient {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
    Cursor,
    Vscode,
}

/// How the server obtained a yes, as the child is told it
/// (`--confirmed-via`). **The command line has an enum of its own for the
/// flag**, since it takes the flag in builds without this crate; a test in
/// `tools.rs` holds the two to one vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmedVia {
    /// A form the client showed, answered in the call (2026-07-28 and later).
    Form,
    /// A form the client showed, asked with `elicitation/create` while the
    /// call waited (the older revisions).
    Elicitation,
    /// A token the agent handed back after asking in the chat.
    Chat,
}

impl ConfirmedVia {
    pub const ALL: [Self; 3] = [Self::Form, Self::Elicitation, Self::Chat];

    /// The token stored and printed, which is also the flag's value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Form => "form",
            Self::Elicitation => "elicitation",
            Self::Chat => "chat",
        }
    }
}
