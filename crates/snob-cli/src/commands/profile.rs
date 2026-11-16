//! `snob profile`: an account the way its page shows it, read by the engine
//! (`snob_engine::profile`, which says what it costs), printed here.

use anyhow::Result;
use snob_engine::app::{App, Viewer};
use snob_engine::engine::target;
use snob_engine::exit::ExitCode;
pub use snob_engine::profile::{
    Highlight, Mutual, MutualPolicy, Profile, Reads, Relation, Visibility, fetch, read,
};
use snob_store::paths::AccountPaths;
use snob_store::secrets::SecretStore;

use crate::cli::{Format, ProfileArgs, ProfileFormat};
use crate::commands::common::{self, Switched};
use crate::output::{self, Presentation, Rendered};
use crate::report;
use crate::ui;

/// How many names the "followed by" line puts before it starts counting.
const NAMES_SHOWN: usize = 3;

pub async fn run(args: ProfileArgs, store: SecretStore, paths: &AccountPaths) -> Result<ExitCode> {
    // Settled before the session is opened and long before a request is
    // spent, like every destination here.
    let format = output::effective_format(args.format.map(Into::into), args.output.as_deref());
    if matches!(format, Format::Csv | Format::Xlsx | Format::Ndjson) {
        anyhow::bail!(
            "a profile has no {} form; it can be a table, json or md",
            format!("{format:?}").to_ascii_lowercase()
        );
    }
    output::check_destination(format, args.output.as_deref())?;

    let browses = args.browses(crate::ui::a_human_would_watch_the_listing_scroll_by());

    // A bar only for the browser, whose clicks can start real walks.
    let app = common::reader(&store, paths, args.interactive, browses)?;
    let typed = common::target_or_own(&app, args.target.as_deref()).await?;

    let before = app.client().pacer().spent();
    let policy = if browses {
        // The browser pays for the mutual walk at the click that asks for it.
        MutualPolicy::Defer
    } else {
        MutualPolicy::WalkNow
    };
    let known = target::known_pk_of(&app, args.target.as_deref(), &typed)?;
    let profile = read(&app, &typed, Reads::all(policy), known).await?;

    if app.cancel().is_canceled() {
        return Ok(ExitCode::Interrupted);
    }

    // The card instead of the document — the default at a terminal, the
    // decision made above.
    if browses {
        // A pacer wait during the fetch may have drawn a bar; the card must not
        // share the screen with it.
        app.progress().finish();
        return browse(app, profile, args.target, before, &store, paths).await;
    }
    let spent = app.client().pacer().spent().saturating_sub(before);

    let presentation = Presentation::detect(args.output.as_deref());
    let hints = format == Format::Table && presentation.interactive;
    let rendered = render(&profile, format, args.target.is_some(), hints)?;
    output::write_rendered(&Rendered::Text(rendered), args.output.as_deref())?;

    let mut line = format!(
        "profile of @{} - {}",
        profile.username,
        report::requests(spent)
    );
    if let Some(mutual) = &profile.mutual
        && !mutual.complete
    {
        line.push_str(&format!(
            " - {} of the {} accounts you follow that follow them are named; the rest were not \
             walked",
            mutual.people.len(),
            mutual.count
        ));
    }
    ui::info(&line);

    Ok(ExitCode::Ok)
}

/// The card, as the account the command runs as and then as each account the
/// card switches to.
async fn browse(
    app: Box<App>,
    profile: Profile,
    target: Option<String>,
    before: u32,
    store: &SecretStore,
    paths: &AccountPaths,
) -> Result<ExitCode> {
    struct Session {
        app: Box<App>,
        profile: Profile,
        /// The account shown, `None` when it is the viewer's own.
        target: Option<String>,
        before: u32,
    }
    common::switching(
        Session {
            app,
            profile,
            target,
            before,
        },
        async |s: &mut Session, note: String| {
            let target = s.target.as_deref();
            crate::ui::profile::browse(&mut s.app, &s.profile, target, paths, store, note).await
        },
        async |s: &Session, to: Viewer| {
            let name = s.profile.username.clone();
            let from = s.app.viewer();
            let switched = common::switch(store, paths, from, &to, true, async |app: &mut App| {
                let known = target::known_pk(app, &name)?;
                read(app, &name, Reads::all(MutualPolicy::Defer), known).await
            })
            .await?;
            Ok(match switched {
                Switched::To((app, profile)) => {
                    app.progress().finish();
                    let target = shown(&profile, app.viewer().pk);
                    Switched::To(Session {
                        app,
                        profile,
                        target,
                        before: 0,
                    })
                }
                Switched::Refused(note) => Switched::Refused(note),
            })
        },
        |s: &Session, several: bool| {
            let spent = s.app.client().pacer().spent().saturating_sub(s.before);
            let whom = if several {
                format!(" as {}", s.app.viewer().label())
            } else {
                String::new()
            };
            ui::info(&format!(
                "profile of @{}{whom} - {}",
                s.profile.username,
                report::requests(spent)
            ));
        },
    )
    .await
}

/// The account a card switched to `viewer` shows: named, so the same account
/// is shown again, or `None` when it is the viewer's own.
fn shown(profile: &Profile, viewer: snob_core::Pk) -> Option<String> {
    (profile.pk != viewer).then(|| profile.username.clone())
}

fn render(profile: &Profile, format: Format, explicit_target: bool, hints: bool) -> Result<String> {
    Ok(match format {
        Format::Json => {
            let mut s = serde_json::to_string_pretty(&as_json(profile))?;
            s.push('\n');
            s
        }
        Format::Md => as_markdown(profile),
        _ => as_text(profile, explicit_target, hints),
    })
}

/// Every value here is a stable token, never a human-facing string: rewording
/// a message must not be able to break this contract.
fn as_json(profile: &Profile) -> serde_json::Value {
    let visible = |v: &Visibility<serde_json::Value>| match v {
        Visibility::Shown(value) => value.clone(),
        Visibility::Hidden => serde_json::Value::Null,
    };
    let highlights = match &profile.highlights {
        Visibility::Shown(list) => Visibility::Shown(serde_json::json!(
            list.iter()
                .enumerate()
                .map(|(index, h)| serde_json::json!({
                    "number": index + 1,
                    "id": h.id,
                    "title": h.title,
                    "items": h.items,
                    "updated_at": h.updated_at,
                }))
                .collect::<Vec<_>>()
        )),
        Visibility::Hidden => Visibility::Hidden,
    };
    let stories_up = match profile.stories_up() {
        Some(Visibility::Shown(n)) => Visibility::Shown(serde_json::json!(n)),
        Some(Visibility::Hidden) | None => Visibility::Hidden,
    };
    serde_json::json!({
        "username": profile.username,
        "full_name": profile.full_name,
        "biography": profile.biography,
        "external_url": profile.external_url,
        "private": profile.is_private,
        "verified": profile.is_verified,
        "category": profile.category,
        "counts": {
            "followers": profile.followers,
            "following": profile.following,
            "posts": profile.posts,
        },
        "relation": profile.relation.map(|r| serde_json::json!({
            "you_follow": r.you_follow,
            "follows_you": r.follows_you,
            "you_requested": r.you_requested,
            "they_requested": r.they_requested,
        })),
        "followed_by": profile.mutual.as_ref().map(|m| serde_json::json!({
            "count": m.count,
            "accounts": m.people,
            "complete": m.complete,
        })),
        // `null` for a part the account keeps from non-followers, which is
        // not the same as an empty list or a zero.
        "highlights": visible(&highlights),
        "stories_up": visible(&stories_up),
        "hidden": !matches!(profile.highlights, Visibility::Shown(_)),
        "read_at": profile.read_at,
    })
}

/// "Followed by @ana, @luis, @eva and 30 others".
fn followed_by_line(mutual: &Mutual) -> Option<String> {
    if mutual.count == 0 {
        return None;
    }
    let names: Vec<String> = if mutual.preview.is_empty() {
        mutual
            .people
            .iter()
            .take(NAMES_SHOWN)
            .map(|u| u.safe_username())
            .collect()
    } else {
        mutual.preview.iter().take(NAMES_SHOWN).cloned().collect()
    };
    let shown = names.len();
    if shown == 0 {
        return Some(format!("Followed by {} you follow", others(mutual.count)));
    }
    let mut parts: Vec<String> = names.iter().map(|n| format!("@{n}")).collect();
    let rest = mutual.count.saturating_sub(shown as u64);
    if rest > 0 {
        parts.push(others(rest));
    }
    Some(format!(
        "Followed by {}",
        match parts.as_slice() {
            [one] => one.clone(),
            [start @ .., last] => format!("{} and {last}", start.join(", ")),
            [] => unreachable!("shown is at least one"),
        }
    ))
}

fn others(n: u64) -> String {
    if n == 1 {
        "1 other".to_string()
    } else {
        format!("{n} others")
    }
}

/// The flags a name is shown with, as the page shows them next to it.
pub(crate) fn badges(profile: &Profile) -> Vec<&'static str> {
    let mut badges = Vec::new();
    if profile.is_verified {
        badges.push("verified");
    }
    if profile.is_private {
        badges.push("private");
    }
    badges
}

fn count(n: Option<u64>) -> String {
    n.map(|n| n.to_string()).unwrap_or_else(|| "?".to_string())
}

pub(crate) fn relation_line(relation: Relation) -> String {
    let you = match (relation.you_follow, relation.you_requested) {
        (true, _) => "you follow them",
        (false, true) => "you asked to follow them",
        (false, false) => "you do not follow them",
    };
    let they = match (relation.follows_you, relation.they_requested) {
        (true, _) => "they follow you",
        (false, true) => "they asked to follow you",
        (false, false) => "they do not follow you",
    };
    format!("{you}, {they}")
}

fn as_text(profile: &Profile, explicit_target: bool, hints: bool) -> String {
    // No at sign in anything handed over to be typed back; AGENTS.md says
    // why, and `scan` does the same.
    let suffix = if explicit_target {
        format!(" {}", profile.username)
    } else {
        String::new()
    };

    let mut rows: Vec<String> = Vec::new();

    let mut head = format!("@{}", profile.username);
    if let Some(name) = &profile.full_name {
        head.push_str(&format!("  {name}"));
    }
    let badges = badges(profile);
    if !badges.is_empty() {
        head.push_str(&format!("  ({})", badges.join(", ")));
    }
    rows.push(head);
    if let Some(category) = &profile.category {
        rows.push(category.clone());
    }
    if let Some(bio) = &profile.biography {
        for line in bio.lines() {
            rows.push(format!("  {line}"));
        }
    }
    if let Some(url) = &profile.external_url {
        rows.push(format!("  {url}"));
    }
    rows.push(String::new());

    rows.push(format!("{:<14}{}", "Followers:", count(profile.followers)));
    rows.push(format!("{:<14}{}", "Following:", count(profile.following)));
    rows.push(format!("{:<14}{}", "Posts:", count(profile.posts)));

    if let Some(relation) = profile.relation {
        rows.push(String::new());
        rows.push(capitalize(&relation_line(relation)));
    }
    if let Some(mutual) = &profile.mutual
        && let Some(line) = followed_by_line(mutual)
    {
        // The whole list under the sentence, wrapped rather than one a line:
        // it is what the requests were spent on, and thirty names in a
        // paragraph read; thirty lines scroll the counters off the screen.
        if mutual.people.is_empty() {
            rows.push(line);
        } else {
            rows.push(format!("{line}:"));
            rows.extend(wrapped(
                mutual
                    .people
                    .iter()
                    .map(|p| format!("@{}", p.safe_username())),
                78,
            ));
        }
    }

    rows.push(String::new());
    match &profile.highlights {
        Visibility::Shown(list) if list.is_empty() => rows.push("Highlights:   none".to_string()),
        Visibility::Shown(list) => {
            rows.push(format!("{:<14}{}", "Highlights:", list.len()));
            // Columns, not characters: a title in CJK or with an emoji is two
            // columns a character, and `{:<width$}` pads by characters, which
            // would set the item counts beside such a title crooked.
            let width = list
                .iter()
                .map(|h| console::measure_text_width(&h.title))
                .max()
                .unwrap_or(0)
                .min(24);
            for (index, h) in list.iter().enumerate() {
                let mut row = format!(
                    "  {:>2}  {}  {}",
                    index + 1,
                    console::pad_str(&clip(&h.title, 24), width, console::Alignment::Left, None),
                    match h.items {
                        Some(1) => "1 item".to_string(),
                        Some(n) => format!("{n} items"),
                        None => String::new(),
                    }
                );
                if let Some(at) = h.updated_at {
                    row.push_str(&format!(
                        ", updated {}",
                        snob_engine::sentences::stored_on(at)
                    ));
                }
                // A tray that knows no count leaves the padding behind.
                rows.push(row.trim_end().to_string());
            }
        }
        Visibility::Hidden => rows.push(
            "Highlights:   not visible: the account is private and you do not follow it"
                .to_string(),
        ),
    }
    match profile.stories_up() {
        Some(Visibility::Shown(0)) => rows.push(format!("{:<14}none", "Stories up:")),
        Some(Visibility::Shown(n)) => rows.push(format!("{:<14}{n}", "Stories up:")),
        Some(Visibility::Hidden) | None => {}
    }

    if hints {
        rows.push(String::new());
        rows.push(format!(
            "{:<14}snob scan{suffix}        followers and following, crossed (walks both lists)",
            "Next:"
        ));
        if matches!(profile.stories_up(), Some(Visibility::Shown(n)) if n > 0) {
            rows.push(format!(
                "{:<14}snob stories{suffix}     see and download what is up",
                ""
            ));
        }
        rows.push(format!(
            "{:<14}snob pfp{suffix}         the profile picture at full size",
            ""
        ));
    }

    let mut s = String::new();
    for row in rows {
        s.push_str(row.trim_end());
        s.push('\n');
    }
    s
}

/// Names in a paragraph, two spaces in, broken before the name that would
/// cross the width.
fn wrapped(names: impl Iterator<Item = String>, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for name in names {
        let length = name.chars().count();
        if !line.is_empty() && line.chars().count() + 1 + length > width {
            lines.push(std::mem::take(&mut line));
        }
        if line.is_empty() {
            line.push_str("  ");
        } else {
            line.push(' ');
        }
        line.push_str(&name);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Cuts a title to a display width, with an ellipsis when it had to.
///
/// Measured in columns, not characters: a title of wide characters clipped
/// to 24 characters is 48 columns across.
fn clip(text: &str, width: usize) -> String {
    console::truncate_str(text, width, "…").into_owned()
}

fn as_markdown(profile: &Profile) -> String {
    let mut s = String::new();
    let badges = badges(profile);
    // Every string the account or Instagram wrote goes through
    // `md::escape`, which keeps HTML in it from being drawn; the address
    // below is the one exception, and is encoded instead.
    s.push_str(&format!("# @{}", output::md::escape(&profile.username)));
    if let Some(name) = &profile.full_name {
        s.push_str(&format!(" — {}", output::md::escape(name)));
    }
    if !badges.is_empty() {
        s.push_str(&format!(" ({})", badges.join(", ")));
    }
    s.push_str("\n\n");
    if let Some(category) = &profile.category {
        s.push_str(&format!("*{}*\n\n", output::md::escape(category)));
    }
    if let Some(bio) = &profile.biography {
        for line in bio.lines() {
            s.push_str(&format!("> {}\n", output::md::escape(line)));
        }
        s.push('\n');
    }
    if let Some(url) = &profile.external_url {
        // An autolink ends at the first `>` and breaks on a space, so a link
        // whose text carries either would spill the rest into the document as
        // markdown of the server's choosing.
        let safe = url
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace(' ', "%20");
        s.push_str(&format!("<{safe}>\n\n"));
    }

    s.push_str("| Followers | Following | Posts |\n|---:|---:|---:|\n");
    s.push_str(&format!(
        "| {} | {} | {} |\n\n",
        count(profile.followers),
        count(profile.following),
        count(profile.posts)
    ));

    if let Some(relation) = profile.relation {
        s.push_str(&format!("{}.\n\n", capitalize(&relation_line(relation))));
    }
    if let Some(mutual) = &profile.mutual {
        if let Some(line) = followed_by_line(mutual) {
            s.push_str(&format!("{}.\n\n", output::md::escape(&line)));
        }
        if !mutual.people.is_empty() {
            for person in &mutual.people {
                s.push_str(&format!(
                    "- @{}\n",
                    output::md::escape(&person.safe_username())
                ));
            }
            s.push('\n');
        }
    }

    match &profile.highlights {
        Visibility::Shown(list) if list.is_empty() => s.push_str("No highlights.\n"),
        Visibility::Shown(list) => {
            s.push_str("## Highlights\n\n| # | Title | Items | Updated |\n|---:|---|---:|---|\n");
            for (index, h) in list.iter().enumerate() {
                s.push_str(&format!(
                    "| {} | {} | {} | {} |\n",
                    index + 1,
                    output::md::escape(&h.title),
                    h.items.map(|n| n.to_string()).unwrap_or_default(),
                    h.updated_at
                        .map(snob_engine::sentences::stored_on)
                        .unwrap_or_default()
                ));
            }
        }
        Visibility::Hidden => {
            s.push_str("Highlights and stories are not visible: the account is private and you do not follow it.\n");
        }
    }
    match profile.stories_up() {
        Some(Visibility::Shown(0)) => s.push_str("\nNothing up right now.\n"),
        Some(Visibility::Shown(1)) => s.push_str("\n1 story up right now.\n"),
        Some(Visibility::Shown(n)) => s.push_str(&format!("\n{n} stories up right now.\n")),
        Some(Visibility::Hidden) | None => {}
    }
    s.push_str(&format!(
        "\n*Read {}.*\n",
        snob_engine::sentences::stored_on(profile.read_at)
    ));
    s
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl From<ProfileFormat> for Format {
    fn from(format: ProfileFormat) -> Self {
        match format {
            ProfileFormat::Table => Self::Table,
            ProfileFormat::Json => Self::Json,
            ProfileFormat::Md => Self::Md,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::model::User;
    use snob_core::{Epoch, Pk};
    use snob_engine::media::Story;

    fn person(name: &str) -> User {
        User {
            pk: Pk::new(1),
            username: name.to_string(),
            full_name: None,
            is_private: None,
            is_verified: None,
            pfp_url: None,
        }
    }

    fn story() -> Story {
        Story {
            kind: snob_engine::media::Kind::Photo,
            taken_at: Epoch::new(0),
            expiring_at: None,
            url: None,
            mentions: Vec::new(),
            thumb: None,
        }
    }

    fn sample() -> Profile {
        Profile {
            pk: Pk::new(7),
            username: "someone".to_string(),
            full_name: Some("Some One".to_string()),
            biography: Some("first line\nsecond line".to_string()),
            external_url: None,
            is_private: true,
            is_verified: false,
            category: None,
            followers: Some(244),
            following: Some(319),
            posts: Some(0),
            relation: Some(Relation {
                you_follow: true,
                follows_you: true,
                you_requested: false,
                they_requested: false,
            }),
            mutual: Some(Mutual {
                count: 33,
                preview: ["ana", "luis", "eva"].map(String::from).to_vec(),
                people: ["pat", "ana", "luis", "eva"]
                    .iter()
                    .map(|n| person(n))
                    .collect(),
                complete: false,
            }),
            highlights: Visibility::Shown(vec![Highlight {
                id: "highlight:1".to_string(),
                title: "trip".to_string(),
                items: Some(5),
                updated_at: None,
                cover: None,
            }]),
            pfp_url: None,
            pfp_full_size: None,
            pfp_anonymous: false,
            pfp_small: None,
            story_up: true,
            stories: Some(Visibility::Shown(vec![story(), story()])),
            read_at: Epoch::new(0),
        }
    }

    /// After a switch the card shows the same account, by name, and as the
    /// viewer's own when the switch was to it.
    #[test]
    fn a_switched_card_shows_the_same_account() {
        let profile = sample();
        assert_eq!(shown(&profile, Pk::new(8)).as_deref(), Some("someone"));
        assert_eq!(shown(&profile, Pk::new(7)), None);
    }

    /// The sentence is the page's: three names and the page's count, whatever
    /// the walk managed.
    #[test]
    fn the_followed_by_line_names_three_and_counts_the_rest() {
        let mutual = sample().mutual.unwrap();
        assert_eq!(
            followed_by_line(&mutual).as_deref(),
            Some("Followed by @ana, @luis, @eva and 30 others")
        );

        let nobody = Mutual::default();
        assert_eq!(followed_by_line(&nobody), None);

        // The walk stands in when the page sent no names.
        let one = Mutual {
            count: 1,
            preview: Vec::new(),
            people: vec![person("ana")],
            complete: true,
        };
        assert_eq!(followed_by_line(&one).as_deref(), Some("Followed by @ana"));
    }

    /// The list is a paragraph: broken before the name that would cross the
    /// width, never inside one, and never an empty line.
    #[test]
    fn the_mutual_list_wraps_before_the_width() {
        let names = ["@aaaa", "@bbbb", "@cccc", "@dddd"].map(String::from);
        let lines = wrapped(names.into_iter(), 14);
        assert_eq!(lines, ["  @aaaa @bbbb", "  @cccc @dddd"]);
        assert!(wrapped(std::iter::empty(), 14).is_empty());
        // One name wider than the width still gets its line.
        assert_eq!(wrapped(["@".repeat(20)].into_iter(), 10).len(), 1);
    }

    /// A hidden part is a sentence of its own, not an empty list.
    #[test]
    fn a_private_account_you_do_not_follow_says_what_it_keeps_back() {
        let mut profile = sample();
        profile.highlights = Visibility::Hidden;
        profile.stories = Some(Visibility::Hidden);
        let text = as_text(&profile, true, false);
        assert!(text.contains("not visible"), "{text}");
        assert!(!text.contains("Stories up"), "{text}");

        let json = as_json(&profile);
        assert!(json["highlights"].is_null());
        assert!(json["stories_up"].is_null());
        assert_eq!(json["hidden"], true);
    }

    /// The hints hand over a command to be typed back, and so carry no at sign.
    #[test]
    fn the_hints_name_the_account_without_an_at_sign() {
        let text = as_text(&sample(), true, true);
        assert!(text.contains("snob scan someone"), "{text}");
        assert!(text.contains("snob stories someone"), "{text}");
        // No at sign on any line that hands a command over to be typed.
        assert!(
            text.lines()
                .filter(|l| l.contains("snob "))
                .all(|l| !l.contains('@')),
            "{text}"
        );

        // Your own account: no name to repeat, and no relation to state.
        let mut own = sample();
        own.relation = None;
        own.mutual = None;
        let text = as_text(&own, false, true);
        assert!(
            text.contains("snob scan\n") || text.contains("snob scan "),
            "{text}"
        );
        assert!(!text.contains("you follow them"), "{text}");
    }

    #[test]
    fn the_json_carries_stable_tokens() {
        let json = as_json(&sample());
        assert_eq!(json["counts"]["followers"], 244);
        assert_eq!(json["relation"]["follows_you"], true);
        assert_eq!(json["followed_by"]["count"], 33);
        assert_eq!(json["followed_by"]["complete"], false);
        assert_eq!(json["highlights"][0]["number"], 1);
        assert_eq!(json["highlights"][0]["id"], "highlight:1");
        assert_eq!(json["stories_up"], 2);
    }

    #[test]
    fn the_markdown_is_a_document() {
        let md = as_markdown(&sample());
        assert!(md.starts_with("# @someone — Some One (private)\n"), "{md}");
        assert!(md.contains("> first line\n> second line\n"), "{md}");
        assert!(md.contains("| 244 | 319 | 0 |"), "{md}");
        assert!(md.contains("## Highlights"), "{md}");
    }

    /// The profile document draws no HTML the account wrote, in its name,
    /// its category, its bio, the names it shares with you or a highlight's
    /// title: each is shown as text. The address is the one `<` and `>` of
    /// the document, around an autolink.
    #[test]
    fn the_markdown_draws_no_html_the_account_wrote() {
        let html = r#"<img src="https://somewhere.test/t.png">"#;
        let mut profile = sample();
        profile.full_name = Some(html.to_string());
        profile.category = Some(html.to_string());
        profile.biography = Some(format!("hello\n{html}"));
        if let Some(mutual) = profile.mutual.as_mut() {
            mutual.preview = vec![html.to_string()];
        }
        profile.highlights = Visibility::Shown(vec![Highlight {
            id: "highlight:1".to_string(),
            title: html.to_string(),
            items: Some(1),
            updated_at: None,
            cover: None,
        }]);
        profile.external_url = Some("https://example.test/".to_string());

        let md = as_markdown(&profile);
        assert_eq!(md.matches('<').count(), 1, "{md}");
        // The autolink's, and the quote marks of the bio's two lines.
        assert_eq!(md.matches('>').count(), 1 + 2, "{md}");
        assert!(md.contains("<https://example.test/>"), "{md}");
        assert_eq!(md.matches("&lt;img").count(), 5, "{md}");
    }

    #[test]
    fn a_long_title_is_clipped_with_an_ellipsis() {
        assert_eq!(clip("short", 24), "short");
        let long = "a".repeat(30);
        let clipped = clip(&long, 24);
        assert_eq!(clipped.chars().count(), 24);
        assert!(clipped.ends_with('…'));

        // Two columns a character: clipped to 24 columns, not 24 characters.
        let wide = clip(&"字".repeat(30), 24);
        assert!(console::measure_text_width(&wide) <= 24, "{wide}");
        assert!(wide.ends_with('…'));
    }

    #[test]
    fn a_populated_profile_serializes_to_the_contract_shape() {
        let mut profile = sample();
        profile.external_url = Some("https://example.test/me".into());
        profile.category = Some("Photographer".into());
        profile.pfp_url = Some("https://cdn.example/pfp.jpg".into());
        profile.pfp_full_size = Some("https://cdn.example/pfp-full.jpg".into());
        profile.read_at = Epoch::new(1_773_576_000);
        if let Some(mutual) = profile.mutual.as_mut() {
            for (index, account) in mutual.people.iter_mut().enumerate() {
                account.pk = Pk::new(10 + index as u64);
                account.full_name = Some("Friend Name".into());
                account.is_private = Some(false);
                account.is_verified = Some(true);
                account.pfp_url = Some("https://cdn.example/friend.jpg".into());
            }
        }
        profile.highlights = Visibility::Shown(vec![Highlight {
            id: "highlight:1".into(),
            title: "trip".into(),
            items: Some(5),
            updated_at: Some(Epoch::new(1_773_489_600)),
            cover: Some("https://cdn.example/cover.jpg".into()),
        }]);
        profile.stories = Some(Visibility::Shown(vec![Story {
            kind: snob_engine::media::Kind::Photo,
            taken_at: Epoch::new(1_773_576_000),
            expiring_at: Some(Epoch::new(1_773_662_400)),
            url: Some("https://cdn.example/story.jpg".into()),
            mentions: vec!["friend".into()],
            thumb: Some("https://cdn.example/story-thumb.jpg".into()),
        }]));
        insta::assert_snapshot!(serde_json::to_string_pretty(&as_json(&profile)).unwrap());
    }
}
