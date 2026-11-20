//! One file from Instagram's CDN, by an address already known: what `snob
//! fetch` saves, an agent's `snob_fetch` and the app's own.
//!
//! **Only files on the CDN, over HTTPS** (`allowlist::refused_asset`), every
//! redirect hop held to the same rule by the client; no credential goes with
//! it. Saved into a directory, **a file there is never written over**.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use snob_ig::client::CdnClient;
use snob_ig::error::IgError;

use crate::posts::MAX_POST_BYTES;

/// `address` read as an address on the CDN, or the refusal saying what it is
/// instead.
pub fn checked(address: &str) -> Result<url::Url> {
    let url = url::Url::parse(address).map_err(|e| anyhow!("that is not an address: {e}"))?;
    let site = snob_ig::client::site();
    if let Some(what) = snob_ig::allowlist::refused_asset(site.as_str(), url.as_str()) {
        return Err(anyhow!(
            "only files on Instagram's CDN (cdninstagram.com, fbcdn.net) are fetched, over \
             HTTPS; this is {what}"
        ));
    }
    Ok(url)
}

/// The User-Agent to send: `given`, else the one `account_ua` reads off the
/// account, else the installed browser's.
pub fn user_agent(
    given: Option<String>,
    account_ua: impl FnOnce() -> Result<Option<String>>,
) -> Result<String> {
    if let Some(given) = given.filter(|ua| !ua.trim().is_empty()) {
        return Ok(given);
    }
    account_ua()?
        .filter(|ua| !ua.trim().is_empty())
        .or_else(|| crate::browser::detect().map(|browser| browser.user_agent()))
        .ok_or_else(|| {
            anyhow!(
                "there is no User-Agent to send: nobody is signed in and no browser was found. \
                 Give one with --user-agent"
            )
        })
}

/// A client for `url` sending `user_agent`, which holds the address to the
/// same rule again, and every redirect hop after it.
pub fn client(user_agent: &str, url: &url::Url) -> Result<CdnClient> {
    let cdn = CdnClient::new(
        user_agent,
        snob_ig::client::site(),
        crate::system::interrupt::install(),
    )?;
    cdn.check_downloadable(url)?;
    Ok(cdn)
}

/// The file, saved into `dir` under the address's own name and the
/// extension its bytes say, as every other download is named, **next to
/// whatever is there, never over it**.
pub async fn into_dir(cdn: &CdnClient, url: &url::Url, dir: &Path) -> Result<PathBuf> {
    let (stem, from_address) = name_of(url);
    let part = dir.join(format!("{stem}.{}.part", std::process::id()));
    let mut file = crate::files::create_new(&part)?;
    let downloaded = match cdn
        .download_to(url.as_str(), MAX_POST_BYTES, &mut file)
        .await
    {
        Ok(downloaded) => downloaded,
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err(explained(e));
        }
    };
    let synced = file.sync_all();
    drop(file);
    let placed = synced
        .map_err(|e| anyhow!("could not finish writing {}: {e}", part.display()))
        .and_then(|()| {
            let extension = extension_for(&downloaded.head, from_address.as_deref());
            crate::files::default_path(dir, &stem, extension)
        })
        .and_then(|name| {
            std::fs::rename(&part, &name)
                .map_err(|e| anyhow!("could not move into place {}: {e}", name.display()))?;
            Ok(name)
        });
    if placed.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    placed
}

/// The file, replacing `path`: the person named it, so replacing what is
/// there is their call, but only with a file that arrived whole.
pub async fn to_path(cdn: &CdnClient, url: &url::Url, path: &Path) -> Result<()> {
    let mut file = crate::files::Replacing::open(path)?;
    cdn.download_to(url.as_str(), MAX_POST_BYTES, file.file())
        .await
        .map_err(explained)?;
    file.commit()
}

/// The file, streamed to standard output.
pub async fn to_stdout(cdn: &CdnClient, url: &url::Url) -> Result<()> {
    let mut sink = crate::files::StdoutSink::default();
    let streamed = cdn
        .download_to(url.as_str(), MAX_POST_BYTES, &mut sink)
        .await;
    sink.finish(streamed.map_err(explained))
}

/// The CDN's refusal said in its own terms: a signed address that has
/// expired, rather than a picture that "could not be downloaded".
fn explained(error: IgError) -> anyhow::Error {
    match error {
        IgError::Unexpected {
            status: 403 | 410, ..
        } => anyhow!(
            "the CDN refused the address; a signed address expires after a while, so ask for \
             the post or the listing again for a fresh one"
        ),
        other => other.into(),
    }
}

/// The address's own file name, kept to what is safe to create anywhere: the
/// stem, and the extension it claims, which is only a fallback.
fn name_of(url: &url::Url) -> (String, Option<String>) {
    let last = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let (stem, extension) = match last.rsplit_once('.') {
        Some((stem, extension)) => (stem, Some(extension)),
        None => (last, None),
    };
    let keep = |text: &str, most: usize| -> String {
        text.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(most)
            .collect()
    };
    let stem = match keep(stem, 64) {
        stem if stem.is_empty() => "download".to_string(),
        stem => stem,
    };
    let extension = extension
        .map(|extension| keep(extension, 8).to_ascii_lowercase())
        .filter(|extension| !extension.is_empty());
    (stem, extension)
}

/// The extension a fetched file is saved under: what its bytes are, as
/// every other download is named, since the CDN converts (`stp=dst-jpg`) and
/// the address is no guide; failing that, the address's own, only when it is
/// one of the media the CDN serves; and otherwise `bin`, so a file that is
/// none of them is not saved under a name a system would run.
fn extension_for(head: &[u8], from_address: Option<&str>) -> &'static str {
    const MEDIA: [&str; 9] = [
        "jpg", "jpeg", "png", "webp", "heic", "gif", "mp4", "webm", "m4a",
    ];
    let jpeg = head.starts_with(&[0xFF, 0xD8, 0xFF]);
    let sniffed = crate::media::extension_of(head);
    if sniffed != "jpg" || jpeg {
        return sniffed;
    }
    from_address
        .and_then(|claimed| MEDIA.iter().find(|media| **media == claimed))
        .copied()
        .unwrap_or("bin")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_name_is_the_addresses_own_and_nothing_else() {
        let name = |address: &str| name_of(&url::Url::parse(address).unwrap());
        assert_eq!(
            name("https://scontent.cdninstagram.com/v/t51/12345_n.jpg?oh=1&oe=2"),
            ("12345_n".to_string(), Some("jpg".to_string()))
        );
        assert_eq!(
            name("https://video.fbcdn.net/o1/v/AbC-dEf.MP4?efg=x"),
            ("AbC-dEf".to_string(), Some("mp4".to_string()))
        );
        assert_eq!(
            name("https://video.fbcdn.net/"),
            ("download".to_string(), None)
        );
        let (sneaky, _) = name("https://scontent.cdninstagram.com/..%2F..%2Fpasswd");
        assert!(
            !sneaky.contains('/') && !sneaky.contains('\\') && !sneaky.starts_with('.'),
            "{sneaky}"
        );
    }

    /// The bytes decide, the address only vouches for media, and anything
    /// else is `bin`, whatever the address says it is.
    #[test]
    fn the_extension_is_read_from_the_bytes_first() {
        let mp4 = b"\x00\x00\x00\x18ftypisom";
        assert_eq!(extension_for(mp4, Some("jpg")), "mp4");
        assert_eq!(
            extension_for(b"\xff\xd8\xff\xe0 a picture", Some("webp")),
            "jpg"
        );
        assert_eq!(extension_for(b"\x89PNG\r\n", None), "png");
        assert_eq!(extension_for(b"\x1aE\xdf\xa3 webm", Some("webm")), "webm");
        assert_eq!(extension_for(b"MZ\x90\x00", Some("exe")), "bin");
        assert_eq!(extension_for(b"function(){}", Some("js")), "bin");
        assert_eq!(extension_for(b"#!/bin/sh", None), "bin");
    }

    #[test]
    fn an_address_off_the_cdn_is_refused_before_anything_is_sent() {
        assert!(checked("https://scontent.cdninstagram.com/v/t51/1_n.jpg").is_ok());
        assert!(checked("http://scontent.cdninstagram.com/v/t51/1_n.jpg").is_err());
        assert!(checked("https://www.instagram.com/api/v1/users/").is_err());
        assert!(checked("https://example.com/a.jpg").is_err());
        assert!(checked("not an address").is_err());
    }
}
