//! A test build's isolation, for whichever program runs the engine: every
//! file a run touches under one directory, its secrets in a file and on a
//! keyring service of that directory's own, and every client pointed at a
//! server of the test's. **Only a `testing` build has this module**
//! (`crates/snob-core/tests/sandbox.rs`).

use std::path::Path;

use snob_store::paths::AppPaths;
use snob_store::secrets::SecretStore;
use url::Url;

/// Where a sandboxed run keeps its files and its secrets.
#[cfg(feature = "testing")]
pub struct Sandbox {
    pub paths: AppPaths,
    /// The keyring service the run is confined to. **The store reaches the
    /// keyring whatever its backend**, so this name is what stands between a
    /// sandboxed login and the real session.
    pub service: String,
}

#[cfg(feature = "testing")]
impl Sandbox {
    /// Confines this process to `root`: every client is pointed at
    /// `instagram` when one is given, through the browser's page when
    /// `through_the_browser`. **Before any client is built, and once.**
    pub fn under(
        root: &Path,
        instagram: Option<Url>,
        through_the_browser: bool,
    ) -> anyhow::Result<Self> {
        if let Some(base) = instagram {
            snob_ig::client::point_every_client_at(base)
                .map_err(|already| anyhow::anyhow!("already pointed at {already}"))?;
        }
        if through_the_browser {
            snob_ig::client::page::use_the_page_off_instagram();
        }
        Ok(Self {
            paths: AppPaths::rooted_at(root),
            service: service_of(root),
        })
    }

    /// The secrets of this sandbox: in a file, on its own keyring service.
    pub fn secrets(&self) -> SecretStore {
        SecretStore::new(self.paths.clone(), true).with_service(&self.service)
    }
}

/// The keyring service a sandbox at `root` is confined to: the same for one
/// root in every process, and another for every other root.
fn service_of(root: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("snob-ig-sandbox-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One process writes the entry and another reads it back, so a name that
    /// changed between invocations would lose the session every time.
    #[test]
    fn the_service_is_the_same_answer_twice_and_differs_per_root() {
        let one = service_of(Path::new("/tmp/one"));
        let again = service_of(Path::new("/tmp/one"));
        let other = service_of(Path::new("/tmp/two"));

        assert_eq!(one, again, "the same root has to name the same entries");
        assert_ne!(
            one, other,
            "two sandboxes at once must not be able to delete each other's entries"
        );
    }

    /// The property the whole sandbox rests on: its service is never the one
    /// the real entries are under.
    #[test]
    fn a_sandbox_keeps_its_files_under_its_root_and_its_secrets_off_the_real_service() {
        let root = Path::new("/tmp/one");
        let sandbox = Sandbox::under(root, None, false).expect("nothing to point");

        assert!(sandbox.paths.data_dir().starts_with(root));
        assert_ne!(sandbox.service, "snob-ig");
        assert!(sandbox.service.starts_with("snob-ig-sandbox-"));
    }
}
