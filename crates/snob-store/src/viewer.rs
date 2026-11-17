//! What a signed-in account shows of itself, kept beside its session so a
//! view can name it without asking Instagram: its name, its bio, and its
//! small picture, kept in the account's cache (`cache`) since it can be
//! fetched again. **Cosmetic, and asked for once only**: right after a
//! login, the account's own profile (`snob_engine::viewer::read_after_login`);
//! otherwise whatever an answer already paid for carried. It goes with the
//! session on a sign-out.

use std::io::Read as _;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use snob_core::Epoch;

use crate::paths::{self, AccountPaths};

/// The most the card's file is read: it holds three short strings.
const MAX_CARD_BYTES: u64 = 64 * 1024;

/// An account's card. **Its strings are another person's words**: whoever
/// saves one passes them through `snob_core::model::printable` first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub biography: Option<String>,
    /// The address of its small picture when it was last seen. It expires.
    #[serde(default)]
    pub picture: Option<String>,
    /// The address the picture kept in [`AccountPaths::viewer_picture`] came
    /// from: none while no picture is kept.
    #[serde(default)]
    pub kept_picture: Option<String>,
    /// When an answer last said any of this.
    #[serde(default)]
    pub seen_at: Option<Epoch>,
}

impl AccountPaths {
    /// The account's card ([`Card`]).
    pub fn viewer_card(&self) -> PathBuf {
        self.dir().join("viewer.json")
    }

    /// Its small picture, as the CDN served it: the cache's picture of the
    /// account itself.
    pub fn viewer_picture(&self) -> PathBuf {
        self.cached_picture(self.pk())
    }
}

impl Card {
    /// The card kept for `account`: none when there is none, or when the file
    /// does not read as one.
    pub fn load(account: &AccountPaths) -> Option<Self> {
        let mut text = String::new();
        std::fs::File::open(account.viewer_card())
            .ok()?
            .take(MAX_CARD_BYTES)
            .read_to_string(&mut text)
            .ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Keeps this card for `account`, whose directory is there already.
    pub fn save(&self, account: &AccountPaths) -> std::io::Result<()> {
        let contents = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        replace(account.viewer_card(), &contents)
    }

    /// Keeps `picture` as the account's, fetched from `address`.
    pub fn keep_picture(
        &mut self,
        account: &AccountPaths,
        address: &str,
        picture: &[u8],
    ) -> std::io::Result<()> {
        if !crate::cache::keep_picture(account, account.pk(), picture)? {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        self.kept_picture = Some(address.to_string());
        self.save(account)
    }

    /// Removes the card and the picture of `account`, and what a killed save
    /// of either left.
    pub fn forget(account: &AccountPaths) {
        for file in [account.viewer_card(), account.viewer_picture()] {
            let temporary = temporary_of(&file);
            let mut lock = temporary.clone().into_os_string();
            lock.push(".lock");
            for path in [file, temporary, PathBuf::from(lock)] {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Where a file of the card's is written before it is moved into place.
fn temporary_of(file: &std::path::Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(file.file_name().unwrap_or_default());
    name.push(".tmp");
    file.with_file_name(name)
}

/// Two processes may save at once: a login in a terminal and the app.
fn replace(file: PathBuf, contents: &[u8]) -> std::io::Result<()> {
    paths::replace_private_alone(&file, &temporary_of(&file), contents).map_err(|(_, e)| e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::AppPaths;
    use snob_core::Pk;

    fn account(root: &std::path::Path) -> AccountPaths {
        let account = AppPaths::rooted_at(root).account(Pk::new(42));
        std::fs::create_dir_all(account.dir()).unwrap();
        account
    }

    #[test]
    fn a_card_saved_is_the_card_read_back() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path());
        assert_eq!(Card::load(&account), None);

        let mut card = Card {
            full_name: Some("Invented Name".into()),
            biography: Some("an invented bio".into()),
            picture: Some("https://example.invalid/a.jpg".into()),
            kept_picture: None,
            seen_at: Some(Epoch::new(1_790_000_000)),
        };
        card.save(&account).unwrap();
        assert_eq!(Card::load(&account), Some(card.clone()));

        card.keep_picture(&account, "https://example.invalid/a.jpg", b"\xff\xd8\xff")
            .unwrap();
        assert_eq!(
            std::fs::read(account.viewer_picture()).unwrap(),
            b"\xff\xd8\xff"
        );
        assert_eq!(
            Card::load(&account).unwrap().kept_picture.as_deref(),
            Some("https://example.invalid/a.jpg")
        );
    }

    #[test]
    fn forgetting_a_card_leaves_none_of_its_files() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path());
        let mut card = Card::default();
        card.keep_picture(&account, "https://example.invalid/a.jpg", b"\xff\xd8\xff")
            .unwrap();

        Card::forget(&account);

        assert_eq!(crate::disk::tree(&account.dir()).files, 0);
    }

    #[test]
    fn a_file_that_is_not_a_card_reads_as_none() {
        let root = tempfile::tempdir().unwrap();
        let account = account(root.path());
        std::fs::write(account.viewer_card(), b"not json").unwrap();
        assert_eq!(Card::load(&account), None);
    }
}
