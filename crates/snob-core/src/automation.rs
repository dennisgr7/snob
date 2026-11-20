//! An automation: steps on a canvas joined by arrows, and what running it
//! comes to. One shape for every automation, the one `watch.toml` describes
//! (`snob_store::automations::from_watch`) and the ones the app draws.
//!
//! **A flow goes one way**: a trigger starts actions, an action's accounts go
//! through filters to an output ([`can_join`]). **Nothing here writes to
//! Instagram**: every action reads, and an output only tells somebody else.
//!
//! **A disabled step is left out of a run**: a trigger never fires, an action
//! is not read and feeds nothing, a filter lets everything through, and an
//! output sends nothing.
//!
//! **A run follows the triggers that started it**: one its schedule started
//! reads what a schedule leads to; one started by hand reads what any
//! trigger does, a manual one included, which no schedule ever starts.
//!
//! Pure: no clock, no file, no request. [`Automation::problems`] says what is
//! missing; [`Automation::program`] is what a finished one runs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::filters::{Attribute, Filter};
use crate::model::{User, possible_username};
use crate::watch::schedule::{ScheduleError, Spec};
use crate::{Epoch, Pk};

/// Which shape of the automations' file this is.
pub const SCHEMA: u32 = 1;

/// The id `watch.toml`'s automation is known by.
pub const WATCH_TOML: &str = "watch";

/// A step's place in the automation's own numbering, which never reuses one.
pub type NodeId = u32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Automation {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub edges: Vec<Edge>,
}

/// One step on the canvas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: NodeId,
    pub step: Step,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Where it is drawn, in the canvas's own units.
    #[serde(default)]
    pub at: (i32, i32),
}

fn yes() -> bool {
    true
}

/// An arrow: what `from` comes to goes on to `to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
}

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// A trigger: on the schedule `snob watch` keeps (`--every`, `--on`/`--at`,
    /// `--cron`).
    Schedule { when: Spec },
    /// A trigger: when somebody runs it, from the app or with `snob watch once
    /// --automation <id>`, and never on a schedule. Braces, so a key it does
    /// not know is refused as any other step's is.
    Manual {},
    /// An action: reads an account.
    Read {
        /// `None` until somebody says whose.
        account: Option<Account>,
        what: Read,
    },
    /// A filter on the accounts an action found.
    Filter { only: Only },
    /// An output: the report, sent to an address.
    Webhook(Webhook),
}

/// Which of the four kinds a step is, which is what may follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Trigger,
    Action,
    Filter,
    Output,
}

impl Step {
    pub fn stage(&self) -> Stage {
        match self {
            Self::Schedule { .. } | Self::Manual {} => Stage::Trigger,
            Self::Read { .. } => Stage::Action,
            Self::Filter { .. } => Stage::Filter,
            Self::Webhook(_) => Stage::Output,
        }
    }

    /// A step of `stage` as it is first put on the canvas, before anything
    /// is said about it.
    pub fn new_read(what: Read) -> Self {
        Self::Read {
            account: None,
            what,
        }
    }
}

/// Whose account an action reads, read as which signed-in account, and the
/// answer to "may this walk somebody else's lists?" when it walks them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    /// `None` for the viewer's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The signed-in account it is read as; `None`, the one in use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewer: Option<Pk>,
    /// When somebody said yes, for another account's lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent: Option<Epoch>,
}

impl Account {
    pub fn own(viewer: Option<Pk>) -> Self {
        Self {
            username: None,
            viewer,
            consent: None,
        }
    }

    /// The name without its at sign, when it is somebody else's.
    pub fn name(&self) -> Option<&str> {
        self.username
            .as_deref()
            .map(|name| name.strip_prefix('@').unwrap_or(name))
    }

    /// The same reader and the same account, whatever the case of the name.
    fn same_as(&self, other: &Self) -> bool {
        self.viewer == other.viewer
            && match (self.name(), other.name()) {
                (Some(one), Some(two)) => one.eq_ignore_ascii_case(two),
                (None, None) => true,
                _ => false,
            }
    }
}

/// What an action reads of an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case", deny_unknown_fields)]
pub enum Read {
    /// Its followers and following, and who came and went: `snob watch`.
    Lists,
    /// The accounts it follows that do not follow it back.
    Unfollowers,
    /// The accounts that follow it and that it does not follow back.
    Fans,
    /// The accounts it follows that follow it back.
    Friends,
    /// Its new posts, their photos and videos saved in `save_to`.
    Posts {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save_to: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quality: Option<VideoQuality>,
    },
    /// Its profile picture, at the best size the web serves, saved in
    /// `save_to` when it changes.
    Picture { save_to: Option<PathBuf> },
    /// The accounts you follow that follow it: "Followed by".
    KnownFollowers,
    /// Its friends who are also yours.
    FriendsInCommon,
}

impl Read {
    /// Every kind, in the order the app offers them.
    pub fn all() -> [Self; 8] {
        [
            Self::Lists,
            Self::Unfollowers,
            Self::Fans,
            Self::Friends,
            Self::Posts {
                save_to: None,
                quality: None,
            },
            Self::Picture { save_to: None },
            Self::KnownFollowers,
            Self::FriendsInCommon,
        ]
    }

    /// The word a file and a report spell it with.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lists => "lists",
            Self::Unfollowers => "unfollowers",
            Self::Fans => "fans",
            Self::Friends => "friends",
            Self::Posts { .. } => "posts",
            Self::Picture { .. } => "picture",
            Self::KnownFollowers => "known_followers",
            Self::FriendsInCommon => "friends_in_common",
        }
    }

    /// Whether it walks the account's followers and following, which is
    /// what asks for consent on somebody else's (AGENTS.md, "Consent").
    pub fn walks_lists(&self) -> bool {
        matches!(
            self,
            Self::Lists | Self::Unfollowers | Self::Fans | Self::Friends | Self::FriendsInCommon
        )
    }

    /// Whether it only means something of another account than the
    /// viewer's: who of the people you follow follow it, its friends who are
    /// yours too.
    pub fn is_about_another(&self) -> bool {
        matches!(self, Self::KnownFollowers | Self::FriendsInCommon)
    }

    /// Whether it finds a set of accounts, which filters act on.
    pub fn finds_accounts(&self) -> bool {
        !matches!(self, Self::Posts { .. } | Self::Picture { .. })
    }

    fn save_to(&self) -> Option<&PathBuf> {
        match self {
            Self::Posts { save_to, .. } | Self::Picture { save_to } => save_to.as_ref(),
            _ => None,
        }
    }
}

/// What a filter lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Only {
    /// Who came and who went since the last run, not everybody.
    Changes,
    Verified,
    Unverified,
    Public,
    Private,
}

impl Only {
    pub const ALL: [Self; 5] = [
        Self::Changes,
        Self::Verified,
        Self::Unverified,
        Self::Public,
        Self::Private,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Changes => "changes",
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

/// The filters a set of accounts goes through, all at once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters(Vec<Only>);

impl Filters {
    pub fn new(only: impl IntoIterator<Item = Only>) -> Self {
        let mut kept: Vec<Only> = Vec::new();
        for one in only {
            if !kept.contains(&one) {
                kept.push(one);
            }
        }
        Self(kept)
    }

    /// Whether the set is told as what changed since the last run.
    pub fn changes(&self) -> bool {
        self.0.contains(&Only::Changes)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = Only> + '_ {
        self.0.iter().copied()
    }

    /// The accounts that pass. **Unknown counts as not**: an account whose
    /// flag was never read passes "unverified" and "public", as
    /// [`Attribute::matches`] reads it.
    pub fn apply(&self, users: Vec<User>) -> Vec<User> {
        let mut filter = Filter::default();
        for one in &self.0 {
            match one {
                Only::Changes => {}
                Only::Verified => filter.only.push(Attribute::Verified),
                Only::Unverified => filter.hide.push(Attribute::Verified),
                Only::Public => filter.hide.push(Attribute::Private),
                Only::Private => filter.only.push(Attribute::Private),
            }
        }
        filter.apply(users)
    }
}

/// Where a report goes: `[webhook]` in `watch.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Webhook {
    pub url: String,
    /// Not secrets: a token belongs in the keyring.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub heartbeat: bool,
    #[serde(default)]
    pub format: WebhookFormat,
}

/// The shape of what a webhook is sent: `json`, the report a program reads
/// and the default, or the report's summary alone as the service at the far
/// end wants it posted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    #[default]
    Json,
    Slack,
    Discord,
    Ntfy,
    Text,
}

impl WebhookFormat {
    pub const ALL: [Self; 5] = [
        Self::Json,
        Self::Slack,
        Self::Discord,
        Self::Ntfy,
        Self::Text,
    ];

    /// The word the file spells it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Slack => "slack",
            Self::Discord => "discord",
            Self::Ntfy => "ntfy",
            Self::Text => "text",
        }
    }
}

/// What a saved video is: its best tracks joined by ffmpeg, or the 720p file
/// with sound the app plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoQuality {
    Best,
    Compat,
}

impl VideoQuality {
    /// The word the file spells it with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Best => "best",
            Self::Compat => "compat",
        }
    }
}

/// Whether an arrow may go from a step of one stage to one of another.
pub fn can_join(from: Stage, to: Stage) -> bool {
    matches!(
        (from, to),
        (Stage::Trigger, Stage::Action)
            | (Stage::Action, Stage::Filter | Stage::Output)
            | (Stage::Filter, Stage::Filter | Stage::Output)
    )
}

/// Why an arrow was not drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unjoined {
    #[error("one of those steps is not there")]
    Missing,
    #[error("a step cannot lead to itself")]
    Itself,
    #[error("those two steps are already joined")]
    Already,
    #[error("a step of that kind cannot follow that one")]
    WrongWay,
    #[error("that would make the flow go round in a circle")]
    Circle,
}

/// What keeps an automation from running, or from being kept where it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error("it has no name")]
    NoName,
    #[error("it has no trigger, so nothing would ever start it")]
    NoTrigger,
    #[error("no trigger leads to an action, so a run would read nothing")]
    NoAction,
    #[error("the schedule cannot be used: {why}")]
    Schedule { node: NodeId, why: ScheduleError },
    #[error("an action does not say whose account it reads")]
    NoAccount { node: NodeId },
    #[error("\"{name}\" cannot be a username")]
    BadName { node: NodeId, name: String },
    #[error("reading the lists of @{name} needs your yes, given in the step's settings")]
    NoConsent { node: NodeId, name: String },
    #[error("this action is about another account: say whose")]
    OwnAccount { node: NodeId },
    #[error("the directory to save in is not a whole path")]
    RelativeDir { node: NodeId },
    #[error("the webhook has no address it can use")]
    NoAddress { node: NodeId },
    #[error("an automation sends to one webhook at most")]
    TwoWebhooks { node: NodeId },
    #[error("@{name}'s {what} are already read by \"{by}\"")]
    Taken {
        node: NodeId,
        name: String,
        what: &'static str,
        by: String,
    },
    #[error("watch.toml cannot hold this step; make it in an automation of its own")]
    NotInWatchToml { node: NodeId },
}

impl Problem {
    /// The step it is about, when it is about one.
    pub fn node(&self) -> Option<NodeId> {
        match self {
            Self::NoName | Self::NoTrigger | Self::NoAction => None,
            Self::Schedule { node, .. }
            | Self::NoAccount { node }
            | Self::BadName { node, .. }
            | Self::NoConsent { node, .. }
            | Self::OwnAccount { node }
            | Self::RelativeDir { node }
            | Self::NoAddress { node }
            | Self::TwoWebhooks { node }
            | Self::Taken { node, .. }
            | Self::NotInWatchToml { node } => Some(*node),
        }
    }
}

/// What a finished automation runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// Every enabled trigger's schedule: a run is due when one of them is.
    /// None for one only ever run by hand.
    pub schedules: Vec<Spec>,
    /// Every enabled action a trigger leads to, in the order of their steps:
    /// what a run by hand reads.
    pub reads: Vec<Planned>,
    pub webhook: Option<Webhook>,
}

/// What started a run, which decides the triggers it follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// Its schedule came round: it follows the schedules alone.
    BySchedule,
    /// Somebody ran it: it follows every trigger.
    ByHand,
}

/// One action of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub node: NodeId,
    pub account: Account,
    pub what: Read,
    /// The enabled filters between it and the output.
    pub only: Filters,
    /// Whether what it finds reaches the webhook.
    pub delivered: bool,
}

impl Automation {
    /// An empty one, with nothing on its canvas.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|node| node.id == id)
    }

    /// Puts `step` on the canvas at `at`, enabled, and says its id: one
    /// above every id it ever had, so an arrow drawn to a removed step can
    /// never land on a new one.
    pub fn insert(&mut self, step: Step, at: (i32, i32)) -> NodeId {
        let id = self.nodes.iter().map(|node| node.id).max().unwrap_or(0) + 1;
        self.nodes.push(Node {
            id,
            step,
            enabled: true,
            at,
        });
        id
    }

    /// Takes a step off the canvas, with its arrows.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let before = self.nodes.len();
        self.nodes.retain(|node| node.id != id);
        self.edges.retain(|edge| edge.from != id && edge.to != id);
        before != self.nodes.len()
    }

    pub fn moved(&mut self, id: NodeId, at: (i32, i32)) -> bool {
        match self.node_mut(id) {
            Some(node) if node.at != at => {
                node.at = at;
                true
            }
            _ => false,
        }
    }

    pub fn set_enabled(&mut self, id: NodeId, enabled: bool) -> bool {
        match self.node_mut(id) {
            Some(node) if node.enabled != enabled => {
                node.enabled = enabled;
                true
            }
            _ => false,
        }
    }

    /// Says what a step does. **Its stage stays**: the arrows into and out
    /// of it were drawn for that stage.
    pub fn configure(&mut self, id: NodeId, step: Step) -> bool {
        match self.node_mut(id) {
            Some(node) if node.step.stage() == step.stage() && node.step != step => {
                node.step = step;
                true
            }
            _ => false,
        }
    }

    /// Draws an arrow from `from` to `to`, or says why not.
    pub fn connect(&mut self, from: NodeId, to: NodeId) -> Result<(), Unjoined> {
        let (Some(one), Some(two)) = (self.node(from), self.node(to)) else {
            return Err(Unjoined::Missing);
        };
        if from == to {
            return Err(Unjoined::Itself);
        }
        if !can_join(one.step.stage(), two.step.stage()) {
            return Err(Unjoined::WrongWay);
        }
        if self.edges.contains(&Edge { from, to }) {
            return Err(Unjoined::Already);
        }
        if self.reaches(to, from) {
            return Err(Unjoined::Circle);
        }
        self.edges.push(Edge { from, to });
        Ok(())
    }

    pub fn disconnect(&mut self, from: NodeId, to: NodeId) -> bool {
        let before = self.edges.len();
        self.edges.retain(|edge| *edge != Edge { from, to });
        before != self.edges.len()
    }

    /// Whether an arrow path leads from `from` to `to`.
    fn reaches(&self, from: NodeId, to: NodeId) -> bool {
        let mut seen = vec![from];
        let mut at = 0;
        while let Some(&now) = seen.get(at) {
            if now == to {
                return true;
            }
            for edge in self.edges.iter().filter(|edge| edge.from == now) {
                if !seen.contains(&edge.to) {
                    seen.push(edge.to);
                }
            }
            at += 1;
        }
        false
    }

    fn after(&self, id: NodeId) -> impl Iterator<Item = &Node> + '_ {
        self.edges
            .iter()
            .filter(move |edge| edge.from == id)
            .filter_map(|edge| self.node(edge.to))
    }

    /// The steps in the order a run takes them: the triggers, then what each
    /// leads to, a column at a time; steps no trigger leads to last. What
    /// the app's summary of a flow lists.
    pub fn columns(&self) -> Vec<Vec<NodeId>> {
        let mut placed: Vec<NodeId> = Vec::new();
        let mut columns: Vec<Vec<NodeId>> = Vec::new();
        let mut column: Vec<NodeId> = self
            .nodes
            .iter()
            .filter(|node| node.step.stage() == Stage::Trigger)
            .map(|node| node.id)
            .collect();
        while !column.is_empty() {
            placed.extend(&column);
            let mut next: Vec<NodeId> = Vec::new();
            for &id in &column {
                for node in self.after(id) {
                    if !placed.contains(&node.id) && !next.contains(&node.id) {
                        next.push(node.id);
                    }
                }
            }
            columns.push(column);
            column = next;
        }
        let left: Vec<NodeId> = self
            .nodes
            .iter()
            .map(|node| node.id)
            .filter(|id| !placed.contains(id))
            .collect();
        if !left.is_empty() {
            columns.push(left);
        }
        columns
    }

    /// The enabled actions some enabled trigger a run `by` follows leads to.
    fn started(&self, by: Started) -> Vec<&Node> {
        let follows = |trigger: &Node| {
            trigger.enabled
                && match by {
                    Started::BySchedule => matches!(trigger.step, Step::Schedule { .. }),
                    Started::ByHand => trigger.step.stage() == Stage::Trigger,
                }
        };
        self.nodes
            .iter()
            .filter(|node| node.enabled && node.step.stage() == Stage::Action)
            .filter(|action| {
                self.edges
                    .iter()
                    .any(|edge| edge.to == action.id && self.node(edge.from).is_some_and(follows))
            })
            .collect()
    }

    /// What an action's accounts go through, and whether they reach an
    /// enabled output, walking every path from it.
    fn downstream(&self, action: NodeId) -> (Filters, bool) {
        let mut only = Vec::new();
        let mut delivered = false;
        let mut seen = vec![action];
        let mut at = 0;
        while let Some(&now) = seen.get(at) {
            for node in self.after(now) {
                if seen.contains(&node.id) {
                    continue;
                }
                seen.push(node.id);
                match &node.step {
                    Step::Filter { only: one } if node.enabled => only.push(*one),
                    Step::Webhook(_) if node.enabled => delivered = true,
                    _ => {}
                }
            }
            at += 1;
        }
        (Filters::new(only), delivered)
    }

    /// Everything that keeps it from running, in the order of its steps.
    /// Nothing, for one that is finished.
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.name.trim().is_empty() {
            problems.push(Problem::NoName);
        }
        let mut webhooks = 0;
        for node in self.nodes.iter().filter(|node| node.enabled) {
            let id = node.id;
            match &node.step {
                Step::Schedule { when } => {
                    if let Err(e) = when.build() {
                        problems.push(Problem::Schedule { node: id, why: e });
                    }
                }
                Step::Read { account, what } => {
                    match account {
                        None => problems.push(Problem::NoAccount { node: id }),
                        Some(account) => match account.name() {
                            Some(name) if !possible_username(name) => {
                                problems.push(Problem::BadName {
                                    node: id,
                                    name: name.to_string(),
                                });
                            }
                            Some(name) if what.walks_lists() && account.consent.is_none() => {
                                problems.push(Problem::NoConsent {
                                    node: id,
                                    name: name.to_string(),
                                });
                            }
                            None if what.is_about_another() => {
                                problems.push(Problem::OwnAccount { node: id });
                            }
                            _ => {}
                        },
                    }
                    if what.save_to().is_some_and(|dir| !dir.is_absolute()) {
                        problems.push(Problem::RelativeDir { node: id });
                    }
                }
                Step::Manual {} | Step::Filter { .. } => {}
                Step::Webhook(webhook) => {
                    webhooks += 1;
                    let usable = webhook.url.split_once("://").is_some_and(|(scheme, rest)| {
                        matches!(scheme, "http" | "https") && !rest.trim().is_empty()
                    });
                    if !usable {
                        problems.push(Problem::NoAddress { node: id });
                    }
                    if webhooks > 1 {
                        problems.push(Problem::TwoWebhooks { node: id });
                    }
                }
            }
        }
        if !self
            .nodes
            .iter()
            .any(|node| node.enabled && node.step.stage() == Stage::Trigger)
        {
            problems.push(Problem::NoTrigger);
        } else if self.started(Started::ByHand).is_empty() {
            problems.push(Problem::NoAction);
        }
        problems
    }

    /// Whether it is finished: nothing in [`Automation::problems`].
    pub fn is_finished(&self) -> bool {
        self.problems().is_empty()
    }

    /// Every enabled action an enabled trigger leads to, in the order of
    /// their steps, whether or not the automation is finished: what a run
    /// by hand reads, `watch.toml`'s too, which needs no schedule.
    pub fn planned_reads(&self) -> Vec<Planned> {
        self.reads_when(Started::ByHand)
    }

    /// What a run `by` reads: [`Automation::planned_reads`] by hand, and only
    /// what a schedule leads to when one came round.
    pub fn reads_when(&self, by: Started) -> Vec<Planned> {
        self.started(by)
            .into_iter()
            .filter_map(|node| match &node.step {
                Step::Read {
                    account: Some(account),
                    what,
                } => {
                    let (only, delivered) = self.downstream(node.id);
                    Some(Planned {
                        node: node.id,
                        account: account.clone(),
                        what: what.clone(),
                        only,
                        delivered,
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// The enabled webhook, when there is one.
    pub fn webhook(&self) -> Option<&Webhook> {
        self.nodes
            .iter()
            .filter(|node| node.enabled)
            .find_map(|node| match &node.step {
                Step::Webhook(webhook) => Some(webhook),
                _ => None,
            })
    }

    /// Every enabled trigger's schedule, in the order of their steps,
    /// whether or not the automation is finished.
    pub fn schedules(&self) -> Vec<&Spec> {
        self.nodes
            .iter()
            .filter(|node| node.enabled)
            .filter_map(|node| match &node.step {
                Step::Schedule { when } => Some(when),
                _ => None,
            })
            .collect()
    }

    /// What it runs, or what keeps it from running.
    pub fn program(&self) -> Result<Program, Vec<Problem>> {
        let problems = self.problems();
        if !problems.is_empty() {
            return Err(problems);
        }
        Ok(Program {
            schedules: self.schedules().into_iter().cloned().collect(),
            reads: self.planned_reads(),
            webhook: self.webhook().cloned(),
        })
    }

    /// What `other` already reads that this one would read too: an account's
    /// lists, or its posts, are told by one automation, since what has been
    /// reported of them is kept once.
    pub fn clashes_with(&self, other: &Self) -> Vec<Problem> {
        let (Ok(mine), Ok(theirs)) = (self.program(), other.program()) else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        for read in &mine.reads {
            let kind = |what: &Read| {
                if what.walks_lists() {
                    Some("lists")
                } else if matches!(what, Read::Posts { .. }) {
                    Some("posts")
                } else {
                    None
                }
            };
            let Some(what) = kind(&read.what) else {
                continue;
            };
            let taken = theirs.reads.iter().any(|their| {
                kind(&their.what) == Some(what) && their.account.same_as(&read.account)
            });
            if taken {
                problems.push(Problem::Taken {
                    node: read.node,
                    name: read.account.name().unwrap_or("you").to_string(),
                    what,
                    by: other.name.clone(),
                });
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn every_day() -> Step {
        Step::Schedule {
            when: Spec {
                every: Some(Duration::from_secs(86_400)),
                ..Spec::default()
            },
        }
    }

    fn reading(what: Read, name: Option<&str>) -> Step {
        Step::Read {
            account: Some(Account {
                username: name.map(str::to_string),
                viewer: Some(Pk::new(42)),
                consent: name.map(|_| Epoch::new(1_700_000_000)),
            }),
            what,
        }
    }

    fn hook() -> Step {
        Step::Webhook(Webhook {
            url: "https://n8n.local/webhook/snob".into(),
            ..Webhook::default()
        })
    }

    /// Trigger, action, filter, webhook: what most automations are.
    fn finished() -> (Automation, [NodeId; 4]) {
        let mut automation = Automation::new("a1", "Unfollowers to n8n");
        let trigger = automation.insert(every_day(), (0, 0));
        let action = automation.insert(reading(Read::Unfollowers, None), (200, 0));
        let filter = automation.insert(
            Step::Filter {
                only: Only::Changes,
            },
            (400, 0),
        );
        let output = automation.insert(hook(), (600, 0));
        automation.connect(trigger, action).unwrap();
        automation.connect(action, filter).unwrap();
        automation.connect(filter, output).unwrap();
        (automation, [trigger, action, filter, output])
    }

    #[test]
    fn arrows_go_one_way_and_never_round_in_a_circle() {
        let (mut automation, [trigger, action, filter, output]) = finished();
        assert_eq!(automation.connect(output, trigger), Err(Unjoined::WrongWay));
        assert_eq!(automation.connect(action, trigger), Err(Unjoined::WrongWay));
        assert_eq!(automation.connect(trigger, filter), Err(Unjoined::WrongWay));
        assert_eq!(automation.connect(action, filter), Err(Unjoined::Already));
        assert_eq!(automation.connect(action, action), Err(Unjoined::Itself));
        assert_eq!(automation.connect(action, 99), Err(Unjoined::Missing));
        let second = automation.insert(Step::Filter { only: Only::Public }, (500, 100));
        automation.connect(filter, second).unwrap();
        assert_eq!(automation.connect(second, filter), Err(Unjoined::Circle));
        assert!(automation.disconnect(filter, second));
        assert!(!automation.disconnect(filter, second));
    }

    #[test]
    fn an_id_is_never_given_twice_and_a_removed_step_takes_its_arrows() {
        let (mut automation, [_, action, filter, _]) = finished();
        assert!(automation.remove(filter));
        assert!(
            automation
                .edges
                .iter()
                .all(|e| e.from != filter && e.to != filter)
        );
        let again = automation.insert(Step::Filter { only: Only::Public }, (0, 0));
        assert!(again > filter && again != action);
        assert!(!automation.remove(filter));
    }

    #[test]
    fn a_finished_automation_has_no_problem_and_runs_its_actions_through_their_filters() {
        let (automation, [trigger, action, _, _]) = finished();
        assert_eq!(automation.problems(), []);
        let program = automation.program().unwrap();
        assert_eq!(program.schedules.len(), 1);
        assert_eq!(program.reads.len(), 1);
        assert_eq!(program.reads[0].node, action);
        assert!(program.reads[0].only.changes());
        assert!(program.reads[0].delivered);
        assert!(program.webhook.is_some());
        assert_eq!(automation.columns()[0], [trigger]);
        assert_eq!(automation.columns().len(), 4);
    }

    #[test]
    fn a_disabled_step_is_left_out_of_the_run() {
        let (mut automation, [trigger, action, filter, output]) = finished();
        automation.set_enabled(filter, false);
        let program = automation.program().unwrap();
        assert!(
            !program.reads[0].only.changes(),
            "a filter lets all through"
        );
        assert!(program.reads[0].delivered);

        automation.set_enabled(output, false);
        let program = automation.program().unwrap();
        assert!(!program.reads[0].delivered && program.webhook.is_none());

        automation.set_enabled(action, false);
        assert_eq!(automation.problems(), [Problem::NoAction]);
        automation.set_enabled(action, true);
        automation.set_enabled(trigger, false);
        assert_eq!(automation.problems(), [Problem::NoTrigger]);
    }

    #[test]
    fn an_unfinished_automation_says_what_each_step_is_missing() {
        let mut automation = Automation::new("a2", " ");
        let trigger = automation.insert(
            Step::Schedule {
                when: Spec::default(),
            },
            (0, 0),
        );
        let action = automation.insert(Step::new_read(Read::Lists), (200, 0));
        let stranger = automation.insert(reading(Read::Fans, Some("friend")), (200, 100));
        let output = automation.insert(Step::Webhook(Webhook::default()), (400, 0));
        automation.connect(trigger, action).unwrap();
        automation.connect(trigger, stranger).unwrap();
        automation.connect(action, output).unwrap();
        if let Some(Node {
            step: Step::Read { account, .. },
            ..
        }) = automation.nodes.iter_mut().find(|n| n.id == stranger)
        {
            account.as_mut().unwrap().consent = None;
        }
        let problems = automation.problems();
        assert!(problems.contains(&Problem::NoName));
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, Problem::Schedule { node, .. } if *node == trigger))
        );
        assert!(problems.contains(&Problem::NoAccount { node: action }));
        assert!(problems.contains(&Problem::NoConsent {
            node: stranger,
            name: "friend".into()
        }));
        assert!(problems.contains(&Problem::NoAddress { node: output }));
        assert!(automation.program().is_err());
    }

    #[test]
    fn posts_and_pictures_ask_no_consent_and_save_only_to_a_whole_path() {
        let mut automation = Automation::new("a3", "Pictures");
        let trigger = automation.insert(every_day(), (0, 0));
        let mut picture = reading(
            Read::Picture {
                save_to: Some(PathBuf::from("pictures")),
            },
            Some("friend"),
        );
        if let Step::Read { account, .. } = &mut picture {
            account.as_mut().unwrap().consent = None;
        }
        let action = automation.insert(picture, (200, 0));
        automation.connect(trigger, action).unwrap();
        assert_eq!(
            automation.problems(),
            [Problem::RelativeDir { node: action }]
        );
    }

    #[test]
    fn two_automations_never_read_the_same_lists() {
        let (one, _) = finished();
        let (mut two, [_, action, _, _]) = finished();
        two.name = "Another".into();
        assert!(matches!(
            two.clashes_with(&one).as_slice(),
            [Problem::Taken { node, what: "lists", .. }] if *node == action
        ));
        two.configure(
            action,
            reading(
                Read::Posts {
                    save_to: None,
                    quality: None,
                },
                None,
            ),
        );
        assert!(two.clashes_with(&one).is_empty(), "posts are not lists");
    }

    #[test]
    fn a_step_keeps_its_stage_when_it_is_configured() {
        let (mut automation, [trigger, action, ..]) = finished();
        assert!(!automation.configure(trigger, reading(Read::Fans, None)));
        assert!(automation.configure(action, reading(Read::Fans, None)));
        assert!(
            !automation.configure(action, reading(Read::Fans, None)),
            "no change"
        );
        assert!(automation.moved(action, (10, 10)));
        assert!(!automation.moved(action, (10, 10)));
    }

    #[test]
    fn filters_let_through_what_they_say() {
        let user = |pk, verified, private| User {
            is_verified: verified,
            is_private: private,
            ..User::named(Pk::new(pk), format!("u{pk}"))
        };
        let all = vec![
            user(1, Some(true), Some(false)),
            user(2, Some(false), Some(true)),
            user(3, None, None),
        ];
        let pks = |filters: Filters| -> Vec<u64> {
            filters
                .apply(all.clone())
                .iter()
                .map(|u| u.pk.get())
                .collect()
        };
        assert_eq!(pks(Filters::new([Only::Verified])), [1]);
        assert_eq!(pks(Filters::new([Only::Unverified])), [2, 3]);
        assert_eq!(pks(Filters::new([Only::Public])), [1, 3]);
        assert_eq!(pks(Filters::new([Only::Private])), [2]);
        assert_eq!(pks(Filters::new([Only::Changes])), [1, 2, 3]);
        assert_eq!(
            pks(Filters::new([Only::Private, Only::Verified])),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn an_automation_reads_back_as_it_was_written() {
        let (mut automation, [_, action, _, _]) = finished();
        let monthly = automation.insert(
            Step::Schedule {
                when: Spec {
                    days: vec!["1".into(), "first mon".into()],
                    months: vec!["jan".into(), "jul".into()],
                    at: vec!["09:00".into()],
                    ..Spec::default()
                },
            },
            (0, 200),
        );
        automation.connect(monthly, action).unwrap();
        let text = serde_json::to_string(&automation).unwrap();
        assert_eq!(
            serde_json::from_str::<Automation>(&text).unwrap(),
            automation
        );
        assert!(text.contains(r#""kind":"schedule""#) && text.contains(r#""what":"unfollowers""#));
        assert!(text.contains(r#""months":["jan","jul"],"days":["1","first mon"]"#));
        assert_eq!(automation.schedules().len(), 2);
        assert!(automation.is_finished());
    }

    #[test]
    fn a_manual_trigger_is_written_by_its_kind_alone_and_refuses_a_key_it_does_not_know() {
        let text = serde_json::to_string(&Step::Manual {}).unwrap();
        assert_eq!(text, r#"{"kind":"manual"}"#);
        assert_eq!(
            serde_json::from_str::<Step>(&text).unwrap(),
            Step::Manual {}
        );
        assert!(serde_json::from_str::<Step>(r#"{"kind":"manual","when":{}}"#).is_err());
    }

    #[test]
    fn a_manual_trigger_is_finished_without_a_schedule_and_read_only_by_hand() {
        let (mut automation, [trigger, action, _, _]) = finished();
        assert!(automation.remove(trigger));
        let manual = automation.insert(Step::Manual {}, (0, 0));
        automation.connect(manual, action).unwrap();
        assert_eq!(automation.problems(), []);
        let program = automation.program().unwrap();
        assert!(program.schedules.is_empty(), "nothing ever comes round");
        assert_eq!(program.reads.len(), 1);
        assert!(automation.reads_when(Started::BySchedule).is_empty());
        assert_eq!(automation.reads_when(Started::ByHand), program.reads);
    }

    #[test]
    fn a_scheduled_run_reads_only_what_a_schedule_leads_to() {
        let (mut automation, [_, action, _, _]) = finished();
        let manual = automation.insert(Step::Manual {}, (0, 100));
        let lists = automation.insert(reading(Read::Lists, None), (200, 100));
        automation.connect(manual, lists).unwrap();
        let nodes = |reads: Vec<Planned>| reads.iter().map(|read| read.node).collect::<Vec<_>>();
        assert_eq!(nodes(automation.reads_when(Started::BySchedule)), [action]);
        assert_eq!(nodes(automation.planned_reads()), [action, lists]);
        // An action two triggers lead to is read either way.
        automation.connect(manual, action).unwrap();
        assert_eq!(nodes(automation.reads_when(Started::BySchedule)), [action]);
    }
}
