//! Which accounts a run watches, and on whose say-so.

use snob_core::Pk;
use snob_store::config::{AccountConfig, WatchConfig};

use snob_engine::engine::watch::{PostsWatch, Watched};

/// Which account a scheduled run watches, as whom, and whether it may.
pub(super) fn watched_from(
    target: Option<String>,
    configured: Option<&WatchConfig>,
    in_use: Option<Pk>,
) -> Vec<Watched> {
    if let Some(name) = target {
        // As the viewer its entry names, when the file has one: read as the
        // account in use instead, the baseline would start again in another
        // account's database. With no entry, the account in use.
        let entry = entry_named(&name, configured);
        let viewer = entry.and_then(|entry| entry.viewer).or(in_use);
        return vec![from_entry(&name, entry).read_as(viewer)];
    }

    let listed: Vec<Watched> = configured
        .into_iter()
        .flat_map(|c| c.accounts.iter())
        .map(|account| {
            let watched = if account.is_own() {
                Watched::own().watching(account.lists, posts_of(account))
            } else {
                // Its own entry, not the first of that name: two viewers can
                // watch one account, and one's consent is not the other's.
                from_entry(&account.target, Some(account))
            };
            watched.read_as(account.viewer.or(in_use))
        })
        .collect();

    // A file with no `[[account]]` at all means the obvious thing rather than
    // nothing: somebody who configured a schedule and a webhook and never
    // mentioned an account meant their own.
    if listed.is_empty() {
        vec![Watched::own().read_as(in_use)]
    } else {
        listed
    }
}

/// One account, with whatever answer its entry records.
///
/// **The at sign comes off here, on both sides**, which makes this the boundary
/// a `Watched` is built at, as every other reader in the tool cleans
/// (`target::resolve`, `target::from_store`, `target::label`).
///
/// **What is watched comes from the same entry**, a name typed included: the
/// file is where somebody said to watch an account's posts and where to save
/// them, and an argument says neither.
fn from_entry(name: &str, entry: Option<&AccountConfig>) -> Watched {
    let name = snob_engine::engine::target::clean(name);
    let recorded = entry.and_then(|account| account.consent);

    let watched = if recorded.is_some() {
        // The table is what matters, not what is in it.
        Watched::consented(name.to_string(), snob_engine::engine::watch::Consent)
    } else {
        Watched::asking(name.to_string())
    };
    match entry {
        Some(account) => watched.watching(account.lists, posts_of(account)),
        None => watched,
    }
}

/// The first entry for somebody else's account named `name`, the at sign off
/// both sides.
fn entry_named<'a>(name: &str, configured: Option<&'a WatchConfig>) -> Option<&'a AccountConfig> {
    let name = snob_engine::engine::target::clean(name);
    configured
        .into_iter()
        .flat_map(|c| c.accounts.iter())
        .find(|account| {
            !account.is_own()
                && snob_engine::engine::target::clean(&account.target).eq_ignore_ascii_case(name)
        })
}

/// What `[account.posts]` says, in the engine's terms.
fn posts_of(account: &AccountConfig) -> Option<PostsWatch> {
    account.posts.as_ref().map(|posts| PostsWatch {
        save_to: posts.save_to.clone(),
        quality: posts.quality,
    })
}

/// What the opening line names, so somebody starting the service can see that
/// it understood which accounts it is for.
pub(super) fn watching_label(watched: &[Watched]) -> String {
    let names: Vec<String> = watched
        .iter()
        .map(|w| snob_engine::app::target_label(w.name()))
        .collect();

    snob_engine::sentences::and_list(&names).unwrap_or_else(|| "nothing".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::watch::fixtures::watch_toml;
    use snob_engine::engine::watch::group_by_viewer;

    /// The opening line names what it watches as a sentence would.
    #[test]
    fn the_opening_line_names_what_it_watches() {
        let friend =
            |name: &str| Watched::consented(name.into(), snob_engine::engine::watch::Consent);
        assert_eq!(watching_label(&[]), "nothing");
        assert_eq!(watching_label(&[Watched::own()]), "your account");
        assert_eq!(
            watching_label(&[Watched::own(), friend("a"), friend("b")]),
            "your account, @a and @b"
        );
    }

    /// A stranger is asked unless the file records that somebody answered.
    ///
    /// `with_recorded_consent` is the only thing standing between `--target`
    /// and a scheduled run enumerating somebody else's lists: make its `None`
    /// arm hand back a `Watched::consented` and an unattended run reads a
    /// stranger on nobody's say-so. A test of `may_run_unattended` on values
    /// built by hand never reaches the function that decides which you get.
    #[test]
    fn a_stranger_is_asked_unless_the_file_says_somebody_answered() {
        let file = watch_toml(
            r#"
every = "6h"

[[account]]
target = "self"

[[account]]
target = "friend"
consent = { agreed_at = 1700 }

[[account]]
target = "acquaintance"
"#,
        );

        let asked = |name: &str, config: Option<&WatchConfig>| {
            let watched = watched_from(Some(name.to_string()), config, None);
            assert_eq!(watched.len(), 1);
            !watched[0].may_run_unattended()
        };

        assert!(
            !asked("friend", Some(&file)),
            "the file records an answer for them"
        );
        assert!(
            asked("stranger", Some(&file)),
            "nobody ever agreed to this one being read"
        );
        assert!(
            asked("acquaintance", Some(&file)),
            "listed is not the same as consented -- the answer is the `consent` table"
        );
        assert!(
            asked("friend", None),
            "with no file there is nowhere an answer could have been recorded"
        );

        // Instagram's spelling and the typed one need not agree in case.
        assert!(!asked("FRIEND", Some(&file)));

        // `self` on the command line is not the `[[account]] target = "self"`
        // line: that one is your own account, which needs nobody's permission,
        // and matching it would hand a stranger named `self` a consent.
        assert!(asked("self", Some(&file)));
    }

    /// The at sign a person types does not change which account is watched.
    ///
    /// Both spellings mean the same account and README.md says so without
    /// qualification.
    #[test]
    fn an_at_sign_does_not_change_which_account_is_watched() {
        let file = watch_toml(
            r#"
every = "6h"

[[account]]
target = "friend"
consent = { agreed_at = 1700 }
"#,
        );

        let typed = watched_from(Some("@friend".to_string()), Some(&file), None);
        assert_eq!(typed.len(), 1);
        assert_eq!(
            typed[0].name(),
            Some("friend"),
            "what reaches the profile endpoint is a username, and the sign is not part of one"
        );
        assert!(
            typed[0].may_run_unattended(),
            "the file records an answer for this account, however it was spelled"
        );

        // And from the other side: the file is documented as safe to hand-edit,
        // so the sign can be in it instead.
        let edited = watch_toml(
            r#"
every = "6h"

[[account]]
target = "@friend"
consent = { agreed_at = 1700 }
"#,
        );
        let listed = watched_from(None, Some(&edited), None);
        assert_eq!(
            listed.iter().map(|w| w.name()).collect::<Vec<_>>(),
            vec![Some("friend")]
        );
        assert!(listed[0].may_run_unattended());

        // `@self` is the same line as `self`, and it is your own account. Read
        // as a stranger it would send a scheduled run looking for confirmation
        // to read an account it owns.
        let own = watch_toml("every = \"6h\"\n\n[[account]]\ntarget = \"@self\"\n");
        let listed = watched_from(None, Some(&own), None);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name(), None, "your own account names nobody");
        assert!(listed[0].may_run_unattended());
    }

    /// Every account the file lists is watched, and a file that lists none
    /// means your own.
    #[test]
    fn the_accounts_watched_are_the_ones_the_file_names() {
        let file = watch_toml(
            r#"
every = "6h"

[[account]]
target = "self"

[[account]]
target = "friend"
consent = { agreed_at = 1700 }
"#,
        );

        let watched = watched_from(None, Some(&file), None);
        let names: Vec<Option<&str>> = watched.iter().map(|w| w.name()).collect();
        assert_eq!(names, [None, Some("friend")]);
        assert!(watched.iter().all(|w| w.may_run_unattended()));

        // A schedule with no `[[account]]` at all means the obvious thing.
        let bare = watch_toml("every = \"6h\"\n");
        let watched = watched_from(None, Some(&bare), None);
        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0].name(), None);
    }

    /// Every entry is read as the viewer the file names, and one that names
    /// none -- or a name typed on the command line -- as the account in use.
    #[test]
    fn each_entry_is_read_as_its_viewer_or_the_account_in_use() {
        let file = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "self"
viewer = 1

[[account]]
target = "friend"
consent = { agreed_at = 1700 }

[[account]]
target = "self"
viewer = 2
"#,
        );
        let in_use = Some(Pk::new(3));

        let watched = watched_from(None, Some(&file), in_use);
        let viewers: Vec<Option<Pk>> = watched.iter().map(Watched::viewer).collect();
        assert_eq!(viewers, [Some(Pk::new(1)), in_use, Some(Pk::new(2))]);

        let typed = watched_from(Some("friend".to_string()), Some(&file), in_use);
        assert_eq!(
            typed[0].viewer(),
            in_use,
            "a typed name is read as the account resolved"
        );
        assert!(typed[0].may_run_unattended());

        let bare = watch_toml("every = \"6h\"\n");
        assert_eq!(watched_from(None, Some(&bare), in_use)[0].viewer(), in_use);

        // Nobody signed in: nothing to fill in, and the file's own still count.
        let alone = watched_from(None, Some(&file), None);
        assert_eq!(alone[1].viewer(), None);
        assert_eq!(alone[0].viewer(), Some(Pk::new(1)));
    }

    /// Groups come in the order the file first names each viewer, and each
    /// keeps its entries in the file's order.
    #[test]
    fn groups_keep_the_order_of_the_file() {
        let file = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "one"
viewer = 2
consent = { agreed_at = 1700 }

[[account]]
target = "self"
viewer = 1

[[account]]
target = "self"
viewer = 2

[[account]]
target = "two"
consent = { agreed_at = 1700 }
"#,
        );

        let groups = group_by_viewer(&watched_from(None, Some(&file), Some(Pk::new(1))));
        let shape: Vec<(Option<Pk>, Vec<Option<&str>>)> = groups
            .iter()
            .map(|(viewer, entries)| (*viewer, entries.iter().map(Watched::name).collect()))
            .collect();
        assert_eq!(
            shape,
            [
                (Some(Pk::new(2)), vec![Some("one"), None]),
                (Some(Pk::new(1)), vec![None, Some("two")]),
            ],
            "the entry with no viewer joins the account in use rather than a group of its own"
        );
    }

    /// Reading somebody's posts asks for no consent, as `snob posts` does not:
    /// an entry that leaves their lists alone may run on a timer without one.
    /// One that also reads their lists still needs it.
    #[test]
    fn a_posts_only_entry_may_run_unattended_without_a_consent() {
        let file = watch_toml(
            r#"
every = "6h"

[[account]]
target = "school"
lists = false
[account.posts]

[[account]]
target = "stranger"
[account.posts]
"#,
        );
        let watched = watched_from(None, Some(&file), None);
        assert!(!watched[0].lists());
        assert!(watched[0].posts().is_some());
        assert!(watched[0].may_run_unattended());
        assert!(watched[1].lists());
        assert!(
            !watched[1].may_run_unattended(),
            "its lists are read too, and nobody agreed to that"
        );
    }

    /// A name typed is read as the viewer its entry names, and two viewers'
    /// entries for one account each keep their own consent.
    #[test]
    fn a_typed_name_is_read_as_its_entrys_viewer_with_its_own_consent() {
        let file = watch_toml(
            r#"
schema = 2
every = "6h"

[[account]]
target = "friend"
viewer = 7

[[account]]
target = "friend"
viewer = 8
consent = { agreed_at = 1700 }
"#,
        );
        let typed = watched_from(Some("friend".to_string()), Some(&file), Some(Pk::new(3)));
        assert_eq!(typed[0].viewer(), Some(Pk::new(7)), "its entry's viewer");

        let listed = watched_from(None, Some(&file), Some(Pk::new(3)));
        assert!(
            !listed[0].may_run_unattended(),
            "viewer 7 never recorded an answer"
        );
        assert!(listed[1].may_run_unattended(), "viewer 8 did");
    }

    /// A name typed on the command line watches what its entry says, the
    /// directory its posts are saved to included; with no entry, its lists.
    #[test]
    fn a_typed_name_watches_what_its_entry_says() {
        let dir = if cfg!(windows) {
            r"D:\school"
        } else {
            "/srv/school"
        };
        let file = watch_toml(&format!(
            "every = \"6h\"\n\n[[account]]\ntarget = \"school\"\nlists = false\n\
             [account.posts]\nsave_to = '{dir}'\nquality = \"compat\"\n\n\
             [[account]]\ntarget = \"self\"\n[account.posts]\n"
        ));

        let typed = watched_from(Some("@school".to_string()), Some(&file), None);
        assert!(!typed[0].lists());
        assert_eq!(
            typed[0].posts(),
            Some(&PostsWatch {
                save_to: Some(std::path::PathBuf::from(dir)),
                quality: Some(snob_store::config::VideoQuality::Compat),
            })
        );

        let own = watched_from(None, Some(&file), None);
        assert_eq!(own[1].name(), None);
        assert!(
            own[1].lists() && own[1].posts().is_some(),
            "self can watch both"
        );

        let unlisted = watched_from(Some("other".to_string()), Some(&file), None);
        assert!(unlisted[0].lists());
        assert_eq!(unlisted[0].posts(), None);
    }
}
