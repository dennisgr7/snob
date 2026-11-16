//! The card of a signed-in account (`snob_store::viewer`): kept from what a
//! login's check was answered, and from what the tab a later check of the
//! session went out from says ([`look_again`]); its picture fetched once per
//! picture. **Nothing here asks Instagram about the account**; the picture
//! is one download from the CDN, and only for a view that shows it.

use std::path::PathBuf;

use snob_core::model::printable;
use snob_ig::client::IgClient;
use snob_ig::page_values::Face;
use snob_store::paths::AccountPaths;
use snob_store::viewer::Card;

use crate::media::{MAX_SMALL_PICTURE, picture_type};

/// Keeps what an answer showed of `account`. What the answer left out stays
/// as it was kept. **Never an error**: the card is cosmetic, and a login does
/// not fail for it.
pub fn keep(account: &AccountPaths, face: Face) {
    let before = Card::load(account).unwrap_or_default();
    let said = |now: Option<String>, was: Option<String>| now.map(|text| printable(&text)).or(was);
    let card = Card {
        full_name: said(face.full_name, before.full_name),
        biography: said(face.biography, before.biography),
        picture: face.picture.or(before.picture),
        kept_picture: before.kept_picture,
        seen_at: Some(snob_core::clock::now()),
    };
    if let Err(e) = card.save(account) {
        tracing::debug!(error = %e, "the account's card could not be kept");
    }
}

/// What the account's tab says of it now, kept, and its picture fetched
/// when that is a new one: after a login or a check of the session, from
/// the browser it went out from, whose document says it **without a request
/// of its own**. Without a browser the card is left as it is, since there it
/// would be one, and only the picture it names is fetched. Returns whether a
/// new picture was kept.
pub async fn look_again(client: &IgClient, account: &AccountPaths) -> bool {
    if client.has_page() {
        match client.own_face().await {
            Ok(Some((_, face))) => keep(account, face),
            Ok(None) => {}
            Err(e) => tracing::debug!(error = %e, "the tab did not say who the account is"),
        }
    }
    let before = modified(&account.viewer_picture());
    picture(client, account).await;
    modified(&account.viewer_picture()) != before
}

fn modified(file: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(file).and_then(|m| m.modified()).ok()
}

/// The account's small picture on disk, fetched first when the one kept is
/// not the one its card last named. None when there is none to show.
pub async fn picture(client: &IgClient, account: &AccountPaths) -> Option<PathBuf> {
    let file = account.viewer_picture();
    let kept = || file.is_file().then(|| file.clone());
    let Some(mut card) = Card::load(account) else {
        return kept();
    };
    let Some(address) = card.picture.clone() else {
        return kept();
    };
    if file.is_file()
        && card
            .kept_picture
            .as_deref()
            .is_some_and(|from| same_picture(from, &address))
    {
        return Some(file);
    }
    match client.download_capped(&address, MAX_SMALL_PICTURE).await {
        Ok(bytes) if picture_type(&bytes).is_some() => {
            if let Err(e) = card.keep_picture(account, &address, &bytes) {
                tracing::debug!(error = %e, "the account's picture could not be kept");
            }
        }
        Ok(_) => tracing::debug!("what came for the account's picture is not a picture"),
        // Not the error's own words: they can hold the address.
        Err(_) => tracing::debug!("the account's picture could not be fetched"),
    }
    kept()
}

/// Whether two addresses name one picture: the same file on the CDN, whatever
/// the signature each answer puts after it.
fn same_picture(one: &str, other: &str) -> bool {
    let file = |address: &str| {
        url::Url::parse(address)
            .ok()
            .map(|url| url.path().to_string())
    };
    match (file(one), file(other)) {
        (Some(one), Some(other)) => one == other,
        _ => one == other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_the_same_under_another_signature() {
        let one = "https://cdn.example.invalid/v/t51/1_n.jpg?oh=aaa&oe=1";
        let again = "https://cdn.example.invalid/v/t51/1_n.jpg?oh=bbb&oe=2";
        let other = "https://cdn.example.invalid/v/t51/2_n.jpg?oh=aaa&oe=1";
        assert!(same_picture(one, again));
        assert!(!same_picture(one, other));
    }

    #[test]
    fn what_an_answer_leaves_out_stays_as_it_was_kept() {
        let root = tempfile::tempdir().unwrap();
        let account = snob_store::paths::AppPaths::rooted_at(root.path()).account(42.into());
        std::fs::create_dir_all(account.dir()).unwrap();

        keep(
            &account,
            Face {
                full_name: Some("Invented\u{202e} Name".into()),
                biography: Some("an invented bio".into()),
                picture: Some("https://cdn.example.invalid/1_n.jpg".into()),
            },
        );
        keep(
            &account,
            Face {
                full_name: Some("Another Name".into()),
                ..Face::default()
            },
        );

        let card = Card::load(&account).unwrap();
        assert_eq!(card.full_name.as_deref(), Some("Another Name"));
        assert_eq!(card.biography.as_deref(), Some("an invented bio"));
        assert_eq!(
            card.picture.as_deref(),
            Some("https://cdn.example.invalid/1_n.jpg")
        );
        assert!(card.seen_at.is_some());
    }

    #[test]
    fn another_persons_words_are_kept_printable() {
        let root = tempfile::tempdir().unwrap();
        let account = snob_store::paths::AppPaths::rooted_at(root.path()).account(42.into());
        std::fs::create_dir_all(account.dir()).unwrap();

        keep(
            &account,
            Face {
                full_name: Some("Invented\u{202e} Name\u{1b}[2K".into()),
                ..Face::default()
            },
        );

        let name = Card::load(&account).unwrap().full_name.unwrap();
        assert!(
            !name.contains('\u{202e}') && !name.contains('\u{1b}'),
            "{name:?}"
        );
    }
}
