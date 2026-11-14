//! Session storage: the system keyring (Credential Manager, Keychain or
//! Secret Service), or a file where there is none.
//!
//! What this protects against:
//!
//! - **Other users of the machine**: yes. The keyring is per account, and the
//!   file is `0600` inside a `0700` directory.
//! - **A bare copy of the store, read elsewhere**: yes for the keyring and,
//!   on Windows, the file, both sealed under the user's login (DPAPI, the
//!   login keychain, the Secret Service collection).
//! - **The secret leaving this computer**: no. The Windows credential is
//!   written `CRED_PERSIST_LOCAL_MACHINE` and does not roam, but outside
//!   Windows the file is `Protection::Plain`, plain JSON at `0600`, a working
//!   session wherever it is copied; and a keychain carried off with the
//!   login password opens anywhere.
//! - **Code running as the user**: no, by any backend, as with every
//!   credential store on a desktop. So secondary entropy for
//!   `CryptProtectData` would buy nothing: it would live in the binary.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use zeroize::{Zeroize, Zeroizing};

use std::path::PathBuf;

use crate::paths::{AccountPaths, AppPaths, PathError};
use snob_core::secret::Secret;
use snob_core::session::{MAX_KEYRING_SECRET_BYTES, Session, keyring_bytes};

/// Installs the platform's credential store, once per process:
/// `set_default_store` replaces what is there, and a store that could not be
/// built (a server, a container, WSL) is not retried on every entry. That
/// failure is ordinary, and the file is what answers next.
fn use_the_platform_store() -> Result<(), SecretsError> {
    use std::sync::OnceLock;

    static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();

    let outcome = INSTALLED.get_or_init(|| {
        #[cfg(windows)]
        let store = windows_native_keyring_store::Store::new();
        #[cfg(target_os = "macos")]
        let store = apple_native_keyring_store::keychain::Store::new();
        #[cfg(all(unix, not(target_os = "macos")))]
        let store = zbus_secret_service_keyring_store::Store::new();

        match store {
            Ok(store) => {
                keyring_core::set_default_store(store);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    });

    outcome.clone().map_err(SecretsError::KeyringUnavailable)
}

const KEYRING_SERVICE: &str = "snob-ig";
const KEYRING_USER: &str = "session";
/// Separate keyring entry used only to check that writing works.
const KEYRING_PROBE_USER: &str = "write-probe";

/// What the credential store said when asked for one secret. "It holds
/// nothing" and "it cannot be reached" are different facts: on the second a
/// webhook report would go out unsigned, and the caller warns.
#[derive(Debug)]
pub enum Stored {
    Found(Secret),
    /// The store answered, and holds nothing under this name.
    Nothing,
    /// The store could not be opened. Whether anything is in it is unknown.
    Unreachable,
}

impl Stored {
    /// The secret, for callers that genuinely do not care why there is none.
    pub fn found(self) -> Option<Secret> {
        match self {
            Self::Found(secret) => Some(secret),
            Self::Nothing | Self::Unreachable => None,
        }
    }

    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::Unreachable)
    }
}

/// Everything kept in the keyring that is not one account's session.
/// [`SecretStore::delete_all`] walks `ALL`, so a secret added later is purged
/// without anybody remembering to. **No feature gate on any variant**: a
/// build without `snob mcp` still purges a key an earlier build stored. The
/// probe entry is not here: it never holds anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A token the monitor sends as a header to the user's webhook.
    WatchToken,
    /// The key the monitor signs the webhook body with.
    WatchSigningKey,
    /// The key a client presents to reach `snob mcp` over the network
    /// (`snob mcp key`). Not an account's: one server serves whichever
    /// account it is told to.
    McpKey,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::WatchToken, Kind::WatchSigningKey, Kind::McpKey];

    /// The keyring entry's user name. Stable: changing one strands what is
    /// stored under it, where `purge` no longer finds it either.
    pub fn entry_name(self) -> &'static str {
        match self {
            Self::WatchToken => "watch-token",
            Self::WatchSigningKey => "watch-signing-key",
            Self::McpKey => "mcp-key",
        }
    }
}

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("the system keyring is unavailable: {0}")]
    KeyringUnavailable(String),
    /// The keyring answered, and would not give the entry up: unlike
    /// [`SecretsError::KeyringUnavailable`], a session is still stored.
    #[error("the system keyring would not delete the stored session: {0}")]
    KeyringRefused(String),
    /// The keyring answered, and would not hand the entry over.
    #[error("the system keyring would not hand over the stored session: {0}")]
    KeyringUnreadable(String),
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the stored session is corrupt: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Paths(#[from] PathError),
    #[cfg(windows)]
    #[error("could not {operation} the session with DPAPI (code {code})")]
    Dpapi { operation: &'static str, code: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The operating system keyring.
    Keyring,
    /// A file in the data directory. On Windows, protected with DPAPI.
    File,
}

impl Backend {
    /// Stable token for machine-readable output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyring => "keyring",
            Self::File => "file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Protection {
    Dpapi,
    Plain,
}

#[derive(Serialize, Deserialize)]
struct StoredSession {
    protection: Protection,
    payload: String,
}

/// What is kept on this machine that is not one account's: the write check,
/// the monitor's secrets, and the removal of every secret at once.
///
/// One account's session is a [`SessionStore`], and [`SecretStore::session_of`]
/// is the only way to get one.
#[derive(Clone)]
pub struct SecretStore {
    backend: Backend,
    paths: AppPaths,
    /// Keyring service name. A field because tests have to point elsewhere:
    /// the keyring is the operating system's, and a test under the real name
    /// would wipe the session of whoever runs the suite.
    service: String,
}

impl SecretStore {
    pub fn new(paths: AppPaths, prefer_file: bool) -> Self {
        let backend = if prefer_file {
            Backend::File
        } else {
            Backend::Keyring
        };
        Self {
            backend,
            paths,
            service: KEYRING_SERVICE.to_string(),
        }
    }

    /// Points at a different set of keyring entries. **Tests and the
    /// sandbox**: forcing [`Backend::File`] is not enough, since this store
    /// reaches the keyring on every backend.
    #[doc(hidden)]
    #[must_use]
    pub fn with_service(mut self, service: &str) -> Self {
        self.service = service.to_string();
        self
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Checks that writing works, and reports **where**, before anything is
    /// asked of the user: a two-factor login must not be lost for want of a
    /// D-Bus. With no keyring it falls back to the file, and never silently:
    /// the caller says which backend it landed on.
    pub fn probe_writable(&self) -> Result<Backend, SecretsError> {
        if self.backend == Backend::Keyring {
            match self.probe_keyring() {
                Ok(()) => return Ok(Backend::Keyring),
                Err(e) => {
                    tracing::debug!(error = %e, "no usable keyring; trying the file instead");
                }
            }
        }

        self.probe_file()?;
        Ok(Backend::File)
    }

    fn probe_keyring(&self) -> Result<(), SecretsError> {
        let entry = self.probe_entry()?;
        entry
            .set_password("probe")
            .map_err(|e| SecretsError::KeyringUnavailable(e.to_string()))?;
        let _ = entry.delete_credential();
        Ok(())
    }

    fn probe_file(&self) -> Result<(), SecretsError> {
        self.paths.ensure_dirs()?;
        // Unlocked: nothing is kept at this name, and a lock's file would be.
        let probe = self.paths.data_dir().join(".write-probe");
        crate::paths::replace_private(&probe, &temporary_beside(&probe), b"probe")
            .map_err(write_failed)?;
        let _ = std::fs::remove_file(&probe);
        Ok(())
    }

    /// Fixes the backend to the one [`SecretStore::probe_writable`] found.
    #[must_use]
    pub fn using(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    /// One account's session, under the keyring entry `session.<pk>` or in
    /// the account's own directory.
    pub fn session_of(&self, account: &AccountPaths) -> SessionStore {
        self.session_at(Place::Account(account.clone()))
    }

    /// The single-account layout's session, under the entry `session` or in
    /// the data directory. What the migration reads, and what `delete_all`
    /// removes along with every account's.
    pub(crate) fn legacy_session(&self) -> SessionStore {
        self.session_at(Place::Legacy(self.paths.clone()))
    }

    fn session_at(&self, place: Place) -> SessionStore {
        SessionStore {
            backend: self.backend,
            service: self.service.clone(),
            place,
        }
    }

    /// Stores one of the secrets in [`Kind`]. **Keyring only**: the monitor
    /// runs without its token and the MCP server takes `SNOB_MCP_KEY`, so no
    /// second protected file is invented for them.
    pub fn save_secret(&self, kind: Kind, value: &Secret) -> Result<(), SecretsError> {
        self.entry_for(kind.entry_name())?
            .set_password(value.expose())
            .map_err(|e| SecretsError::KeyringRefused(e.to_string()))
    }

    /// Reads one back, if it is there.
    pub fn load_secret(&self, kind: Kind) -> Result<Stored, SecretsError> {
        match self.entry_for(kind.entry_name()) {
            Ok(entry) => match entry.get_password() {
                Ok(value) => Ok(Stored::Found(Secret::new(value))),
                Err(keyring_core::Error::NoEntry) => Ok(Stored::Nothing),
                Err(e) => Err(SecretsError::KeyringRefused(e.to_string())),
            },
            // Not an error — the tool works without one — but not "nothing is
            // stored" either.
            Err(e) => {
                tracing::debug!(error = %e, "there is no keyring to read from");
                Ok(Stored::Unreachable)
            }
        }
    }

    /// Removes one, for a `setup` that is being run again with no token this
    /// time. Silent about one that was not there.
    pub fn forget_secret(&self, kind: Kind) -> Result<(), SecretsError> {
        match self.entry_for(kind.entry_name()) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
                Err(e) => Err(SecretsError::KeyringRefused(e.to_string())),
            },
            Err(_) => Ok(()),
        }
    }

    /// Which of the secrets in [`Kind`] are on this machine, so that `purge`
    /// can name what it removes. The removal is `Kind::ALL` whatever this
    /// says.
    pub fn secrets_stored(&self) -> Vec<Kind> {
        Kind::ALL
            .into_iter()
            .filter(|kind| matches!(self.load_secret(*kind), Ok(Stored::Found(_))))
            .collect()
    }

    /// Whether a session is stored for any account in `accounts`, or for the
    /// single-account layout: the sessions [`Self::delete_all`] takes, asked
    /// first so `purge` can name them. A session too corrupt to parse counts.
    pub fn sessions_stored(&self, accounts: &[snob_core::Pk]) -> bool {
        self.legacy_session().something_is_stored()
            || accounts.iter().any(|pk| {
                self.session_of(&self.paths.account(*pk))
                    .something_is_stored()
            })
    }

    /// Removes every secret this tool has written: those in [`Kind`], the
    /// session of every account in `accounts` and the single-account
    /// layout's. `Ok` only if they all went. `snob logout` removes a session
    /// alone ([`SessionStore::delete`]): the monitor still needs its token.
    pub fn delete_all(&self, accounts: &[snob_core::Pk]) -> Result<(), SecretsError> {
        let places: Vec<Place> = accounts
            .iter()
            .map(|pk| Place::Account(self.paths.account(*pk)))
            .chain([Place::Legacy(self.paths.clone())])
            .collect();
        let entries: Vec<String> = Kind::ALL
            .iter()
            .map(|kind| kind.entry_name().to_string())
            .chain(places.iter().map(Place::entry_name))
            .collect();
        let files: Vec<PathBuf> = places.iter().map(Place::file).collect();
        remove(&self.service, &entries, &files)
    }

    /// A separate entry for the write check: probing over a real session
    /// would destroy it whenever the login that follows fails.
    fn probe_entry(&self) -> Result<keyring_core::Entry, SecretsError> {
        self.entry_for(KEYRING_PROBE_USER)
    }

    fn entry_for(&self, user: &str) -> Result<keyring_core::Entry, SecretsError> {
        entry_for(&self.service, user)
    }
}

/// Where one session is kept: its keyring entry and its file.
#[derive(Debug, Clone)]
enum Place {
    /// The single-account layout's: the entry `session` and `session.json` in
    /// the data directory.
    Legacy(AppPaths),
    /// One account's: the entry `session.<pk>` and `session.json` in its
    /// directory.
    Account(AccountPaths),
}

impl Place {
    /// The keyring entry's user name. Stable, as [`Kind::entry_name`] is.
    fn entry_name(&self) -> String {
        match self {
            Self::Legacy(_) => KEYRING_USER.to_string(),
            Self::Account(account) => format!("{KEYRING_USER}.{}", account.pk()),
        }
    }

    fn file(&self) -> PathBuf {
        match self {
            Self::Legacy(paths) => paths.legacy_session_file(),
            Self::Account(account) => account.session_file(),
        }
    }

    fn ensure_dirs(&self) -> Result<(), PathError> {
        match self {
            Self::Legacy(paths) => paths.ensure_dirs(),
            Self::Account(account) => account.ensure_dirs(),
        }
    }
}

/// One session: saved, read back and removed.
pub struct SessionStore {
    backend: Backend,
    service: String,
    place: Place,
}

impl SessionStore {
    pub fn backend(&self) -> Backend {
        self.backend
    }

    /// Where the session lives, for the user to read.
    pub fn describe(&self) -> String {
        match self.backend {
            Backend::Keyring => "system keyring".to_string(),
            Backend::File => format!("file {}", self.place.file().display()),
        }
    }

    /// The file path, when the backend is one.
    pub fn storage_path(&self) -> Option<String> {
        self.path_in(self.backend)
    }

    /// [`Self::storage_path`] for the backend a session was found in
    /// ([`Self::load_located`]), which is not always the store's own.
    pub fn path_in(&self, backend: Backend) -> Option<String> {
        match backend {
            Backend::Keyring => None,
            Backend::File => Some(self.place.file().display().to_string()),
        }
    }

    pub fn save(&self, session: &Session) -> Result<(), SecretsError> {
        self.save_in(self.backend, session)
    }

    /// [`Self::save`], to a backend named here rather than the store's own:
    /// the one [`Self::load_located`] found the session in.
    pub fn save_in(&self, backend: Backend, session: &Session) -> Result<(), SecretsError> {
        // `Zeroizing`: the serialized session is the cookie in plaintext.
        let json = Zeroizing::new(
            serde_json::to_string(session)
                .map_err(|e| SecretsError::Corrupt(format!("while serializing: {e}")))?,
        );

        match backend {
            Backend::Keyring => {
                // The Windows credential has a size ceiling; past it the
                // essentials are stored rather than nothing. In this arm
                // only: a file has no limit, and the essentials lack the
                // CSRF token.
                let json =
                    if keyring_bytes(&json) > MAX_KEYRING_SECRET_BYTES {
                        tracing::warn!(
                            bytes = keyring_bytes(&json),
                            "the session does not fit the keyring whole; storing only the \
                         essential fields"
                        );
                        Zeroizing::new(serde_json::to_string(&session.minimal()).map_err(|e| {
                            SecretsError::Corrupt(format!("while serializing: {e}"))
                        })?)
                    } else {
                        json
                    };

                self.entry()?
                    .set_password(&json)
                    .map_err(|e| SecretsError::KeyringUnavailable(e.to_string()))?;
                // `load` reads the keyring first, so a file left beside the
                // entry is a live cookie nothing reads any more.
                if let Err(e) = std::fs::remove_file(self.place.file())
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(
                        error = %e,
                        file = %self.place.file().display(),
                        "the session is in the keyring, but its older copy in the file could \
                         not be removed"
                    );
                }
            }
            Backend::File => {
                self.write_file(&json)?;
                // `load` reads the keyring **first**: an entry that will not
                // go is the session every later run picks up.
                if let Ok(entry) = self.entry()
                    && let Err(e) = entry.delete_credential()
                    && !matches!(e, keyring_core::Error::NoEntry)
                {
                    tracing::warn!(
                        error = %e,
                        "the session was written to the file, but the keyring would not give up \
                         its own copy; that copy is the one later runs will read"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Option<Session>, SecretsError> {
        Ok(self.load_located()?.map(|(session, _)| session))
    }

    /// The stored session, and the backend it was found in, for writing it
    /// back where it came from: after a login that fell back to the file,
    /// later runs are built without `--no-keyring`.
    pub fn load_located(&self) -> Result<Option<(Session, Backend)>, SecretsError> {
        let kept = match self.read_keyring() {
            // The keyring is there and refused: not "no session stored". The
            // file is tried next, out loud.
            Err(e @ SecretsError::KeyringUnreadable(_)) => {
                tracing::warn!(error = %e, "trying the file instead");
                None
            }
            read => read?,
        };
        self.located(kept)
    }

    /// [`Self::load_located`], except that a keyring which refuses is an
    /// error: the move to the per-account layout deletes what it read from.
    pub(crate) fn load_located_strictly(&self) -> Result<Option<(Session, Backend)>, SecretsError> {
        let kept = self.read_keyring()?;
        self.located(kept)
    }

    fn located(
        &self,
        kept: Option<Zeroizing<String>>,
    ) -> Result<Option<(Session, Backend)>, SecretsError> {
        // Both places, whatever the preferred backend: a session saved with
        // `--no-keyring` still shows up without the flag.
        if let Some(json) = kept {
            return parse_session(&json).map(|s| Some((s, Backend::Keyring)));
        }
        if let Some(json) = self.load_from_file()? {
            return parse_session(&json).map(|s| Some((s, Backend::File)));
        }
        Ok(None)
    }

    /// Whether there is a credential here at all. **Anything but a clean
    /// "nothing there" counts**: a session too corrupt to parse is still on
    /// the disk, and `logout` must not say there was none while deleting it.
    pub fn something_is_stored(&self) -> bool {
        !matches!(self.load(), Ok(None))
    }

    /// Removes the session from everywhere it can be; `Ok` only if it went.
    /// Every location is attempted even after one refuses. `NoEntry` and
    /// having no keyring at all are not refusals.
    pub fn delete(&self) -> Result<(), SecretsError> {
        remove(
            &self.service,
            &[self.place.entry_name()],
            &[self.place.file()],
        )
    }

    fn entry(&self) -> Result<keyring_core::Entry, SecretsError> {
        entry_for(&self.service, &self.place.entry_name())
    }

    /// The keyring's copy: `None` when there is no keyring or no entry, and
    /// [`SecretsError::KeyringUnreadable`] when there is one and it refused.
    fn read_keyring(&self) -> Result<Option<Zeroizing<String>>, SecretsError> {
        let entry = match self.entry() {
            Ok(e) => e,
            // No keyring at all: the session may still be in the file.
            Err(e) => {
                tracing::debug!(error = %e, "there is no keyring to read from");
                return Ok(None);
            }
        };
        match entry.get_password() {
            Ok(json) => Ok(Some(Zeroizing::new(json))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretsError::KeyringUnreadable(e.to_string())),
        }
    }

    fn load_from_file(&self) -> Result<Option<Zeroizing<String>>, SecretsError> {
        // Read and judged by the answer, never asked about first: a `logout`
        // elsewhere may remove the file in between.
        match read_file(&self.place.file()) {
            Err(SecretsError::Read { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(None)
            }
            read => read.map(Some),
        }
    }

    fn write_file(&self, json: &str) -> Result<(), SecretsError> {
        self.place.ensure_dirs()?;
        // Outside Windows both are the cookie in the clear.
        let mut stored = protect(json)?;
        let serialized = serde_json::to_vec_pretty(&stored).map(Zeroizing::new);
        stored.payload.zeroize();
        let serialized =
            serialized.map_err(|e| SecretsError::Corrupt(format!("while wrapping: {e}")))?;
        write_private(&self.place.file(), &serialized)
    }
}

/// Removes these keyring entries and these files, every one even after one
/// refuses; `Ok` only if they all went. Of two errors the file's wins: it
/// names a path somebody can delete by hand.
fn remove(service: &str, entries: &[String], files: &[PathBuf]) -> Result<(), SecretsError> {
    let mut keyring_refused = None;
    for name in entries {
        match entry_for(service, name) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                Err(e) => {
                    keyring_refused.get_or_insert(SecretsError::KeyringRefused(e.to_string()));
                }
            },
            Err(e) => tracing::debug!(error = %e, "there is no keyring to delete from"),
        }
    }

    // With each file, the temporary a save killed halfway left beside it:
    // outside Windows it holds the cookie as the file does.
    let mut file_refused = None;
    for file in files
        .iter()
        .flat_map(|file| [file.clone(), temporary_beside(file)])
    {
        let file = &file;
        if let Err(source) = std::fs::remove_file(file)
            && source.kind() != std::io::ErrorKind::NotFound
            && file_refused.is_none()
        {
            file_refused = Some(SecretsError::Write {
                path: file.display().to_string(),
                source,
            });
        }
    }

    match file_refused.or(keyring_refused) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Builds a keyring entry, installing the platform's store the first time.
///
/// **On Windows the credential is asked for local persistence**, which is why
/// this goes to `keyring-core` and a store directly: the default, Enterprise,
/// is visible "to logon sessions for this user on other computers", and the
/// cookie would roam. Checked on Windows 11 against a real Credential
/// Manager: persistence is a field of the record, not part of its key, so an
/// entry written as Enterprise is read unchanged and becomes local the next
/// time it is written. The target name is the store's default,
/// `{user}.{service}`, the one the `keyring` wrapper composes.
fn entry_for(service: &str, user: &str) -> Result<keyring_core::Entry, SecretsError> {
    use_the_platform_store()?;

    #[cfg(windows)]
    {
        let modifiers = std::collections::HashMap::from([("persistence", "Local")]);
        keyring_core::Entry::new_with_modifiers(service, user, &modifiers)
            .map_err(|e| SecretsError::KeyringUnavailable(e.to_string()))
    }
    #[cfg(not(windows))]
    {
        keyring_core::Entry::new(service, user)
            .map_err(|e| SecretsError::KeyringUnavailable(e.to_string()))
    }
}

/// Reads a stored session, clearing what it read: on Unix the payload is the
/// cookie in the clear.
fn read_file(path: &std::path::Path) -> Result<Zeroizing<String>, SecretsError> {
    use std::io::Read as _;

    let unread = |source| SecretsError::Read {
        path: path.display().to_string(),
        source,
    };
    let mut raw = Zeroizing::new(Vec::new());
    std::fs::File::open(path)
        .map_err(unread)?
        .take(MAX_SESSION_FILE_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(unread)?;
    if raw.len() as u64 > MAX_SESSION_FILE_BYTES {
        return Err(SecretsError::Corrupt(format!(
            "the session file is larger than any session is ({} KiB at most)",
            MAX_SESSION_FILE_BYTES / 1024
        )));
    }
    let mut stored: StoredSession = serde_json::from_slice(&raw).map_err(|e| {
        SecretsError::Corrupt(format!("the session file is not valid: {}", shape_of(&e)))
    })?;
    let session = unprotect(&stored);
    stored.payload.zeroize();
    session
}

fn parse_session(json: &str) -> Result<Session, SecretsError> {
    let session: Session = serde_json::from_str(json)
        .map_err(|e| SecretsError::Corrupt(format!("could not parse: {}", shape_of(&e))))?;
    session
        .check_schema()
        .map_err(|e| SecretsError::Corrupt(e.to_string()))?;
    held_to_shape(&session)?;
    Ok(session)
}

/// The most a session's file may weigh, read no further. A session is a
/// handful of cookies, none near a kilobyte, a username and a User-Agent of
/// at most 512 characters (`snob_core::session`): under 4 KiB as JSON, and
/// under 8 KiB once DPAPI has sealed it and base64 spelled the result. Eight
/// times that, so that nothing stored is refused and a file somebody swapped
/// in is not read whole into memory.
const MAX_SESSION_FILE_BYTES: u64 = 64 * 1024;

/// Holds a stored session to what a login can have stored, **naming the field
/// that fails and nothing of what it holds**, as [`shape_of`] does: the
/// sentence is printed, and handed to an agent.
///
/// A pasted `sessionid` and a User-Agent are checked on their way in; a
/// session read back was taken as it was, and every one of these goes into a
/// header: a line break in one is a header of somebody else's choosing.
fn held_to_shape(session: &Session) -> Result<(), SecretsError> {
    use snob_core::session::SessionError;

    let corrupt = |what: String| Err(SecretsError::Corrupt(what));
    // snob-core's own check, as a paste meets it.
    let checked = match Session::from_sessionid(
        session.sessionid.expose(),
        &session.user_agent,
        session.origin,
    ) {
        Ok(checked) => checked,
        Err(SessionError::EmptySessionId | SessionError::MalformedSessionId) => {
            return corrupt("its sessionid is not a cookie's value".into());
        }
        Err(_) => return corrupt("its User-Agent is not a browser's".into()),
    };
    if checked.ds_user_id != session.ds_user_id {
        return corrupt("its sessionid is another account's than the one it names".into());
    }

    let cookies = [
        ("sessionid", Some(session.sessionid.expose())),
        ("csrftoken", session.csrftoken.as_ref().map(Secret::expose)),
        ("mid", session.mid.as_deref()),
        ("ig_did", session.ig_did.as_deref()),
        ("datr", session.datr.as_deref()),
    ];
    for (name, value) in cookies {
        if value.is_some_and(|v| v.chars().any(|c| c.is_whitespace() || c.is_control())) {
            return corrupt(format!(
                "its {name} holds a character no cookie's value does"
            ));
        }
    }
    let lines = [
        ("User-Agent", Some(session.user_agent.as_str())),
        ("username", session.username.as_deref()),
    ];
    for (name, value) in lines {
        if value.is_some_and(|v| v.chars().any(char::is_control)) {
            return corrupt(format!("its {name} holds a control character"));
        }
    }
    Ok(())
}

/// What is wrong with stored JSON, **without serde's own sentence**: that one
/// quotes the value it tripped on, which here can be the cookie.
fn shape_of(e: &serde_json::Error) -> String {
    use serde_json::error::Category;

    let what = match e.classify() {
        Category::Io => "it could not be read",
        Category::Syntax => "it is not JSON",
        Category::Data => "it is not what this version stores",
        Category::Eof => "it ends early",
    };
    format!("{what} (line {}, column {})", e.line(), e.column())
}

/// Writes a session's file with restricted permissions and atomically, one
/// writer at a time (`paths::replace_private_alone`): a command and the
/// monitor may write back what the browser rotated at the same moment.
fn write_private(path: &std::path::Path, contents: &[u8]) -> Result<(), SecretsError> {
    crate::paths::replace_private_alone(path, &temporary_beside(path), contents)
        .map_err(write_failed)
}

/// The temporary a file here is written through: one name, in its directory.
fn temporary_beside(path: &std::path::Path) -> PathBuf {
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    dir.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ))
}

fn write_failed((path, source): (PathBuf, std::io::Error)) -> SecretsError {
    SecretsError::Write {
        path: path.display().to_string(),
        source,
    }
}

#[cfg(windows)]
fn protect(json: &str) -> Result<StoredSession, SecretsError> {
    let encrypted = dpapi::protect(json.as_bytes())?;
    Ok(StoredSession {
        protection: Protection::Dpapi,
        payload: b64(&encrypted),
    })
}

#[cfg(not(windows))]
fn protect(json: &str) -> Result<StoredSession, SecretsError> {
    // On Unix the protection is the file's 0600 permissions.
    Ok(StoredSession {
        protection: Protection::Plain,
        payload: json.to_string(),
    })
}

fn unprotect(stored: &StoredSession) -> Result<Zeroizing<String>, SecretsError> {
    match stored.protection {
        Protection::Plain => Ok(Zeroizing::new(stored.payload.clone())),
        Protection::Dpapi => {
            #[cfg(windows)]
            {
                let bytes = unb64(&stored.payload)?;
                String::from_utf8(dpapi::unprotect(&bytes)?)
                    .map(Zeroizing::new)
                    .map_err(|e| {
                        let reason = e.utf8_error();
                        e.into_bytes().zeroize();
                        SecretsError::Corrupt(format!("not valid UTF-8: {reason}"))
                    })
            }
            #[cfg(not(windows))]
            {
                Err(SecretsError::Corrupt(
                    "the session is DPAPI-protected and this system is not Windows".into(),
                ))
            }
        }
    }
}

#[cfg(windows)]
fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(windows)]
fn unb64(s: &str) -> Result<Vec<u8>, SecretsError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| SecretsError::Corrupt(format!("invalid base64: {e}")))
}

/// Encryption tied to the Windows user account. A file protected this way is
/// useless copied to another machine or another account.
#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use super::SecretsError;

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>, SecretsError> {
        transform(plain, true)
    }

    pub fn unprotect(encrypted: &[u8]) -> Result<Vec<u8>, SecretsError> {
        transform(encrypted, false)
    }

    fn transform(input: &[u8], encrypt: bool) -> Result<Vec<u8>, SecretsError> {
        let length = u32::try_from(input.len())
            .map_err(|_| SecretsError::Corrupt("it is too large to be a session".into()))?;
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: length,
            pbData: input.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        // SAFETY: both blobs are valid for the duration of the call; the output
        // buffer is allocated by Windows and freed with LocalFree below.
        let ok = unsafe {
            if encrypt {
                CryptProtectData(
                    &in_blob,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out_blob,
                )
            } else {
                CryptUnprotectData(
                    &in_blob,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out_blob,
                )
            }
        };

        if ok == 0 {
            // SAFETY: reads the calling thread's own last-error value and takes
            // no arguments. Read here rather than later because any further call
            // would replace it.
            let code = unsafe { GetLastError() };
            return Err(SecretsError::Dpapi {
                operation: if encrypt { "encrypt" } else { "decrypt" },
                code,
            });
        }

        if out_blob.pbData.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: on success Windows guarantees pbData is valid for cbData
        // bytes, and it was just checked not to be null.
        let output = unsafe {
            std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec()
        };
        // On the decrypt path this is the session in the clear, in memory
        // `LocalFree` hands back to the heap as it is: wiped first.
        //
        // SAFETY: same pointer and length the read above used, still owned by
        // this function and not yet freed.
        unsafe {
            std::ptr::write_bytes(out_blob.pbData, 0, out_blob.cbData as usize);
            LocalFree(out_blob.pbData as *mut core::ffi::c_void)
        };

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snob_core::session::SessionOrigin;

    const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
    const SID: &str = "71234567890%3AAbCdEfGhIjKl%3A20";
    /// The account `SID` belongs to.
    const ME: snob_core::Pk = snob_core::Pk::new(71_234_567_890);

    /// A keyring service name of its own for each test, for the reason
    /// `SecretStore::service` gives.
    fn test_service() -> String {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("snob-ig-test-{}-{n}", std::process::id())
    }

    /// Serializes the tests that reach the keyring.
    ///
    /// The credential store belongs to the operating system, and touching it
    /// from several threads at once is not reliable here: an entry written by
    /// one test came back missing to another, roughly one run in ten, in
    /// whichever test happened to be running at the time. Not a collision
    /// between the tests — each already has a service name of its own — so the
    /// race is below this code and cannot be fixed from here. Running them one
    /// at a time is the whole fix, and it costs milliseconds.
    ///
    /// The guard is handed back by `file_store` so a test that goes through it
    /// cannot forget to take it. One test does not go through it --
    /// `the_session_lands_where_the_probe_said_it_would` builds a
    /// keyring-backed store on purpose -- and it takes the lock by hand.
    ///
    /// **It does not reach across test binaries**, which a `static` cannot do,
    /// and `snob-cli` runs its own in parallel. That is why this is a reduction
    /// in a failure rate rather than a fix: what is left is one operating
    /// system credential store being written by two processes at once, which
    /// nothing in this repository can serialize.
    fn keyring_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn file_store() -> (
        tempfile::TempDir,
        SecretStore,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let held = keyring_lock();
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        (
            tmp,
            SecretStore::new(paths, true).with_service(&test_service()),
            held,
        )
    }

    /// The session of the account `SID` belongs to.
    fn mine(store: &SecretStore) -> SessionStore {
        store.session_of(&store.paths.account(ME))
    }

    /// The real service name must not appear in any test.
    #[test]
    fn tests_never_point_at_the_real_keyring() {
        let (_tmp, store, _keyring) = file_store();
        assert_ne!(store.service, KEYRING_SERVICE);
        assert!(store.service.starts_with("snob-ig-test-"));
    }

    /// Every kind has an entry name, and no two share one.
    ///
    /// Two kinds pointing at one entry would have the second silently overwrite
    /// the first — the signing key landing on top of the token, with nothing
    /// failing anywhere.
    #[test]
    fn every_kind_has_a_name_of_its_own() {
        // `ALL` is the list `purge` walks, and every test of it -- including
        // this one -- walks the same list, so shrinking `ALL` is invisible to
        // them: drop `WatchSigningKey` from it and `snob purge` leaves the
        // signing key in the user's keyring forever, with nothing failing.
        //
        // Two guards, because they catch opposite mistakes. The match is
        // exhaustive, so a variant added later stops this compiling until
        // somebody looks at `ALL`; the count catches a variant taken out of
        // `ALL` while the type keeps it.
        fn is_a_kind(kind: Kind) -> bool {
            match kind {
                Kind::WatchToken | Kind::WatchSigningKey | Kind::McpKey => true,
            }
        }
        assert!(Kind::ALL.into_iter().all(is_a_kind));
        assert_eq!(
            Kind::ALL.len(),
            3,
            "a kind left `ALL`, so `purge` no longer removes it"
        );

        let names: Vec<&str> = Kind::ALL.iter().map(|k| k.entry_name()).collect();
        let unique: std::collections::BTreeSet<_> = names.iter().collect();
        assert_eq!(
            names.len(),
            unique.len(),
            "two kinds share an entry: {names:?}"
        );
        assert!(!names.contains(&KEYRING_PROBE_USER));
        assert!(!names.contains(&KEYRING_USER));
    }

    /// The guard `snob purge` rests on. Its whole promise is that afterwards
    /// there is nothing of this tool left on the machine, and a webhook token
    /// forgotten in the keyring is precisely the failure it exists to prevent.
    ///
    /// Walking `Kind::ALL` rather than naming the two, so a secret added
    /// later is covered by this test the moment it joins the list — the same
    /// shape as the test that walks every `StopReason`.
    #[test]
    fn purging_takes_every_kind_of_secret_with_it() {
        let (_tmp, store, _keyring) = file_store();
        mine(&store)
            .save(&Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap())
            .unwrap();

        if !save_the_monitors_secrets(&store) {
            return;
        }

        store.delete_all(&[ME]).unwrap();

        assert!(
            mine(&store).load().unwrap().is_none(),
            "the session is still there"
        );
        for kind in Kind::ALL {
            assert!(
                store.load_secret(kind).unwrap().found().is_none(),
                "{kind:?} survived a purge"
            );
        }
    }

    /// Stores one secret for every kind. Returns false when
    /// there is no keyring to store them in, which `save_secret` documents and
    /// is not what any of these tests are about.
    fn save_the_monitors_secrets(store: &SecretStore) -> bool {
        for kind in Kind::ALL {
            if store.save_secret(kind, &Secret::new("a secret")).is_err() {
                return false;
            }
        }
        true
    }

    /// The keyring's size ceiling is the keyring's, not the file's.
    #[test]
    fn the_file_backend_does_not_shrink_to_fit_a_keyring() {
        let (_tmp, store, _keyring) = file_store();
        let store = store.using(Backend::File);

        let mut session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        session.csrftoken = Some(Secret::new("a-csrf-token"));
        // Long enough that the whole thing is past the keyring ceiling.
        session.ig_did = Some("x".repeat(MAX_KEYRING_SECRET_BYTES));

        mine(&store).save(&session).unwrap();

        let back = mine(&store).load().unwrap().expect("it was just saved");
        assert!(
            back.csrftoken.is_some(),
            "a file has no size ceiling, so nothing may be dropped to fit one"
        );
    }

    /// `snob logout` takes the session and nothing else, which is what its help
    /// says in those words; `SecretStore::delete_all` says why the split
    /// matters.
    #[test]
    fn logging_out_leaves_the_monitors_secrets_alone() {
        let (_tmp, store, _keyring) = file_store();
        mine(&store)
            .save(&Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap())
            .unwrap();

        if !save_the_monitors_secrets(&store) {
            return;
        }

        mine(&store).delete().unwrap();

        assert!(
            mine(&store).load().unwrap().is_none(),
            "the session should be gone"
        );
        for kind in Kind::ALL {
            assert!(
                store.load_secret(kind).unwrap().found().is_some(),
                "logout took {kind:?} with it"
            );
        }
    }

    #[test]
    fn a_stored_secret_reads_back_and_can_be_forgotten() {
        let (_tmp, store, _keyring) = file_store();
        if store
            .save_secret(Kind::WatchToken, &Secret::new("Bearer abc"))
            .is_err()
        {
            return; // no keyring on this machine; see `save_secret`
        }

        assert_eq!(
            store
                .load_secret(Kind::WatchToken)
                .unwrap()
                .found()
                .unwrap()
                .expose(),
            "Bearer abc"
        );
        store.forget_secret(Kind::WatchToken).unwrap();
        assert!(
            store
                .load_secret(Kind::WatchToken)
                .unwrap()
                .found()
                .is_none()
        );
        // Forgetting one that is not there is not an error: `setup` run again
        // with no token has to be able to clear whatever was there before.
        store.forget_secret(Kind::WatchToken).unwrap();
    }

    /// The MCP key is kept where the monitor's secrets are, under an entry
    /// of its own, and `snob mcp key revoke` takes it out again. On the
    /// file backend too, since it has no file form: `--no-keyring` is about
    /// where the session goes.
    #[test]
    fn the_mcp_key_is_saved_read_and_forgotten() {
        let (_tmp, store, _keyring) = file_store();
        let key = "snob_mcp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        if store.save_secret(Kind::McpKey, &Secret::new(key)).is_err() {
            return; // no keyring on this machine; see `save_secret`
        }

        assert_eq!(
            store
                .load_secret(Kind::McpKey)
                .unwrap()
                .found()
                .unwrap()
                .expose(),
            key
        );
        assert_eq!(store.secrets_stored(), vec![Kind::McpKey]);
        assert!(
            store
                .load_secret(Kind::WatchToken)
                .unwrap()
                .found()
                .is_none(),
            "the key is under its own entry, not the monitor's"
        );

        store.forget_secret(Kind::McpKey).unwrap();
        assert!(matches!(
            store.load_secret(Kind::McpKey).unwrap(),
            Stored::Nothing
        ));
        assert!(store.secrets_stored().is_empty());
    }

    /// A session is reported with the backend it was found in, so that it
    /// can be written back there.
    #[test]
    fn the_backend_a_session_was_found_in_is_reported() {
        let (_tmp, store, _keyring) = file_store();
        let store = mine(&store);
        assert!(store.load_located().unwrap().is_none());
        let original = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        store.save(&original).unwrap();
        let (found, backend) = store.load_located().unwrap().unwrap();
        assert_eq!(backend, Backend::File);
        assert_eq!(found.ds_user_id, original.ds_user_id);
    }

    #[test]
    fn file_round_trip() {
        let (_tmp, store, _keyring) = file_store();
        let original = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();

        assert_eq!(store.probe_writable().unwrap(), Backend::File);
        mine(&store).save(&original).unwrap();

        let recovered = mine(&store)
            .load()
            .unwrap()
            .expect("there should be a session");
        assert_eq!(recovered.sessionid, original.sessionid);
        assert_eq!(recovered.ds_user_id, original.ds_user_id);
        assert_eq!(recovered.user_agent, original.user_agent);
    }

    /// The promise `probe_writable` makes: whatever backend it names is where
    /// the session actually lands. A machine with no keyring — a server, a
    /// container, WSL — gets the file instead of a refusal, and the caller is
    /// told so rather than left to discover it.
    ///
    /// Which branch runs here depends on the machine: with a working keyring it
    /// proves the keyring path, without one it proves the fallback. The
    /// assertion is the same either way, which is the point — the answer and
    /// the destination cannot disagree.
    #[test]
    fn the_session_lands_where_the_probe_said_it_would() {
        // The lock, taken by hand because this test builds a keyring-backed
        // store rather than going through `file_store`.
        let _keyring = keyring_lock();
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let store = SecretStore::new(paths, false).with_service(&test_service());

        let landed = store.probe_writable().unwrap();
        let account = store.paths.account(ME);
        let store = store.using(landed).session_of(&account);

        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        store.save(&session).unwrap();

        assert_eq!(
            account.session_file().exists(),
            landed == Backend::File,
            "the file should exist exactly when the probe named the file"
        );
        let read_back = load_settled(&store);
        assert_eq!(
            read_back
                .expect("the session was just written and has to read back")
                .sessionid
                .expose(),
            session.sessionid.expose()
        );

        store.delete().unwrap();
    }

    /// The Windows credential stays on the machine it was created on.
    ///
    /// `entry_for` asks for local persistence rather than the store's roaming
    /// default, and this is what stops that being a comment: the modifier is a
    /// string, a typo in it is accepted by the type system, and nothing else
    /// in the suite would notice.
    ///
    /// Its own throwaway service name, like every other test that reaches the
    /// credential store, and it deletes what it wrote.
    #[cfg(windows)]
    #[test]
    fn the_windows_credential_is_local_to_this_machine() {
        let _keyring = keyring_lock();
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let store = SecretStore::new(paths, false).with_service(&test_service());

        let entry = store
            .entry_for("persistence-check")
            .expect("Windows always has a Credential Manager");
        entry.set_password("not a session").expect("it writes");

        let attributes = entry
            .get_attributes()
            .expect("the store says what it wrote");
        assert_eq!(
            attributes.get("persistence").map(String::as_str),
            Some("Local"),
            "the credential roams: {attributes:?}"
        );

        entry
            .delete_credential()
            .expect("it cleans up after itself");
    }

    /// Reads the session back, waiting out the credential store if it needs it.
    ///
    /// Not a retry bolted on to make a red test green. What it waits for was
    /// measured: this suite creates and deletes dozens of credentials in
    /// parallel, and two or three runs in fifty ended with `save` reporting the
    /// write to the Windows Credential Manager as successful and the read
    /// immediately after it answering `NoEntry` — the credential is not
    /// missing, it is not visible yet.
    ///
    /// The delay is the whole mechanism, which is why an immediate second
    /// attempt did not help: both landed inside the same window, microseconds
    /// apart. Adding any tracing to the path made it stop reproducing, which is
    /// the other reason to believe it is a timing window rather than logic.
    ///
    /// The real store is deliberately kept rather than faked. What this test is
    /// for is that the answer `probe_writable` gave and the place `save` put it
    /// cannot disagree, and against an in-memory double that proves nothing
    /// about the platform it is asserting.
    fn load_settled(store: &SessionStore) -> Option<Session> {
        for attempt in 0..5 {
            if let Some(session) = store.load().unwrap() {
                return Some(session);
            }
            std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
        }
        store.load().unwrap()
    }

    #[test]
    fn with_no_session_stored_it_returns_none() {
        let (_tmp, store, _keyring) = file_store();
        assert!(mine(&store).load().unwrap().is_none());
    }

    #[test]
    fn delete_removes_the_session() {
        let (_tmp, store, _keyring) = file_store();
        let store = mine(&store);
        let s = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        store.save(&s).unwrap();
        store.delete().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    /// Having nothing to delete is the result being asked for, not a refusal.
    ///
    /// `delete` reports what would not go, and the keyring's way of saying
    /// "there was nothing here" is `keyring_core::Error::NoEntry` — an answer,
    /// not a failure. Reading it as one would make every `logout` on a clean
    /// machine exit non-zero.
    #[test]
    fn nothing_stored_is_not_a_refusal() {
        let (_tmp, store, _keyring) = file_store();
        mine(&store).delete().unwrap();
    }

    /// One copy refusing must not spare the others, and must still be reported.
    ///
    /// The keyring branch cannot be driven from a test — reaching a fake store
    /// means depending on `keyring-core` directly, which `entry_for` documents
    /// as the thing not to do, and no test may touch the real one. So the shape
    /// is pinned through the filesystem, which the keyring branch shares: every
    /// location is attempted, and the refusal comes back at the end rather than
    /// short-circuiting, which would leave the copies after the failure alive.
    ///
    /// A directory standing where the file goes is how the refusal is arranged:
    /// `remove_file` fails on one on every platform, which a permission bit
    /// does not — Windows governs deletion by the file's read-only attribute
    /// and Unix by the parent's write bit. What is being tested is the
    /// reporting, not the reason the operating system said no.
    #[test]
    fn a_copy_that_will_not_go_is_reported_and_does_not_stop_the_others() {
        let (_tmp, store, _keyring) = file_store();
        let s = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        mine(&store).save(&s).unwrap();

        // The single-account layout's copy, which `delete_all` removes after
        // every account's. It is the one after the refusal, and it still has
        // to go.
        store.legacy_session().save(&s).unwrap();
        let previous = store.paths.legacy_session_file();

        let holding = store.paths.account(ME).session_file();
        std::fs::remove_file(&holding).unwrap();
        std::fs::create_dir(&holding).unwrap();

        let result = store.delete_all(&[ME]);

        assert!(
            matches!(result, Err(SecretsError::Write { .. })),
            "the refusal has to reach the caller, or purge claims the session is gone"
        );
        assert!(
            holding.exists(),
            "the test did not arrange what it meant to"
        );
        assert!(
            !previous.exists(),
            "the copy after the refusal was skipped, which is the failure the loop exists to avoid"
        );
    }

    /// A save killed between writing its temporary and moving it into place
    /// leaves the cookie in that temporary. Signing out takes it too.
    #[test]
    fn signing_out_removes_what_a_killed_save_left_beside_the_file() {
        let (_tmp, store, _keyring) = file_store();
        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        mine(&store).save(&session).unwrap();
        let left = temporary_beside(&store.paths.account(ME).session_file());
        std::fs::write(&left, b"what a killed save left").unwrap();

        mine(&store).delete().unwrap();
        assert!(!left.exists(), "the temporary outlived the sign-out");
        assert!(!store.paths.account(ME).session_file().exists());
    }

    /// What is read back from the store goes into headers as it is. A stored
    /// session with a line break in a cookie or in its User-Agent is refused,
    /// by the name of the field and with nothing of what it holds.
    #[test]
    fn a_stored_session_no_login_could_have_stored_is_refused_by_its_field() {
        let (_tmp, store, _keyring) = file_store();
        let whole = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        let refused = |session: &Session| {
            mine(&store).save(session).unwrap();
            mine(&store).load().unwrap_err().to_string()
        };

        let mut cookie = whole.clone();
        cookie.mid = Some("abc\r\nX-Injected: yes".to_string());
        let said = refused(&cookie);
        assert!(said.contains("mid"), "{said}");
        assert!(!said.contains("Injected"), "{said}");

        let mut agent = whole.clone();
        agent.user_agent = format!("{UA}\r\nCookie: stolen");
        let said = refused(&agent);
        assert!(said.contains("User-Agent"), "{said}");
        assert!(!said.contains("stolen"), "{said}");

        let mut another = whole.clone();
        another.ds_user_id = snob_core::Pk::new(1);
        let said = refused(&another);
        assert!(said.contains("another account's"), "{said}");
        assert!(!said.contains("AbCdEfGhIjKl"), "{said}");

        mine(&store).save(&whole).unwrap();
        assert!(mine(&store).load().unwrap().is_some());
    }

    /// A session's file is a few kilobytes. One far past that is not read
    /// whole into memory to find out it is no session.
    #[test]
    fn a_session_file_larger_than_any_session_is_refused_unread() {
        let (_tmp, store, _keyring) = file_store();
        let account = store.paths.account(ME);
        account.ensure_dirs().unwrap();
        let huge = vec![b' '; MAX_SESSION_FILE_BYTES as usize + 1];
        std::fs::write(account.session_file(), huge).unwrap();

        let said = mine(&store).load().unwrap_err().to_string();
        assert!(said.contains("larger than any session"), "{said}");
    }

    #[test]
    fn a_corrupt_file_gives_a_clear_error() {
        let (_tmp, store, _keyring) = file_store();
        let account = store.paths.account(ME);
        account.ensure_dirs().unwrap();
        std::fs::write(account.session_file(), b"this is not json").unwrap();
        assert!(matches!(mine(&store).load(), Err(SecretsError::Corrupt(_))));
    }

    /// serde's own sentence quotes the value it tripped on. A keyring entry or
    /// a file holding the cookie as a bare string, as somebody pasting one in
    /// by hand leaves it, is refused without the cookie in the error, which
    /// is printed and, for an agent, handed back as JSON.
    #[test]
    fn a_stored_session_that_does_not_parse_is_not_quoted_in_the_error() {
        let bare = format!("\"{SID}\"");

        let from_the_keyring = parse_session(&bare).unwrap_err().to_string();
        assert!(
            !from_the_keyring.contains("AbCdEfGhIjKl"),
            "{from_the_keyring}"
        );
        assert!(from_the_keyring.contains("line 1"), "{from_the_keyring}");

        let (_tmp, store, _keyring) = file_store();
        let account = store.paths.account(ME);
        account.ensure_dirs().unwrap();
        std::fs::write(account.session_file(), &bare).unwrap();
        let from_the_file = mine(&store).load().unwrap_err().to_string();
        assert!(!from_the_file.contains("AbCdEfGhIjKl"), "{from_the_file}");
    }

    #[test]
    fn the_backend_token_is_stable() {
        assert_eq!(Backend::Keyring.as_str(), "keyring");
        assert_eq!(Backend::File.as_str(), "file");
    }

    /// The header says what the backends do, and no more than that.
    ///
    /// That bullet is what a person reads before deciding whether to back up a
    /// profile or hand on a disk image, so it gets a test rather than a
    /// proofread. `include_str!` because the claim is the artifact under test;
    /// there is nothing else to call.
    ///
    /// It must not promise that every backend keeps the secret on this
    /// computer: outside Windows `protect` stores `Protection::Plain`, plain
    /// JSON at `0600` that is a working session on any machine it is copied
    /// to. And it must name the persistence the Windows credential really has,
    /// the local one `entry_for` asks for, not the roaming one: the two
    /// spellings differ by one word, and the wrong one reads as an answer.
    #[test]
    fn the_header_does_not_promise_more_than_the_backends_do() {
        let header = include_str!("secrets.rs")
            .lines()
            .take_while(|line| line.starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !header.contains("ties the secret to this user on this computer"),
            "the header promises the secret cannot travel, and one backend lets it"
        );
        assert!(
            header.contains("CRED_PERSIST_LOCAL_MACHINE"),
            "the Windows credential stays on this machine, and the header is where \
             that is read"
        );
        assert!(
            !header.contains("CRED_PERSIST_ENTERPRISE"),
            "the header still describes the roaming credential this stopped writing"
        );
        assert!(
            header.contains("Protection::Plain"),
            "the file fallback outside Windows is plain JSON, and the header is \
             where that is read"
        );
    }

    #[test]
    #[cfg(windows)]
    fn on_windows_the_file_does_not_hold_the_credential_in_the_clear() {
        let (_tmp, store, _keyring) = file_store();
        let s = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        mine(&store).save(&s).unwrap();
        let raw = std::fs::read_to_string(store.paths.account(ME).session_file()).unwrap();
        assert!(
            !raw.contains("AbCdEfGhIjKl"),
            "the sessionid appears in the clear in the file"
        );
    }

    #[test]
    #[cfg(unix)]
    fn on_unix_the_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, store, _keyring) = file_store();
        let s = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        mine(&store).save(&s).unwrap();
        let mode = std::fs::metadata(store.paths.account(ME).session_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "actual mode: {:o}", mode & 0o777);
    }

    /// Each account has an entry of its own, and none of them is the
    /// single-account layout's or one of the monitor's.
    #[test]
    fn every_account_has_an_entry_of_its_own() {
        let paths = AppPaths::rooted_at("/tmp/test");
        let a = Place::Account(paths.account(snob_core::Pk::new(1))).entry_name();
        let b = Place::Account(paths.account(snob_core::Pk::new(2))).entry_name();
        assert_eq!(a, "session.1");
        assert_ne!(a, b);
        assert_ne!(a, Place::Legacy(paths.clone()).entry_name());
        for kind in Kind::ALL {
            assert_ne!(a, kind.entry_name());
        }
    }

    /// Two accounts' sessions live side by side, and removing one leaves the
    /// other where it was.
    ///
    /// On whatever backend the probe lands on, like
    /// `the_session_lands_where_the_probe_said_it_would`: the keyring is where
    /// the two could collide, under one entry name, and a file store alone
    /// would not see it.
    #[test]
    fn two_accounts_keep_two_sessions() {
        let _keyring = keyring_lock();
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::rooted_at(tmp.path());
        let store = SecretStore::new(paths.clone(), false).with_service(&test_service());
        let landed = store.probe_writable().unwrap();
        let store = store.using(landed);

        let other = snob_core::Pk::new(81_234_567_890);
        let a = store.session_of(&paths.account(ME));
        let b = store.session_of(&paths.account(other));
        a.save(&Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap())
            .unwrap();
        b.save(
            &Session::from_sessionid("81234567890%3AZyXwVuTsRqPo%3A20", UA, SessionOrigin::Paste)
                .unwrap(),
        )
        .unwrap();

        assert_eq!(load_settled(&a).unwrap().ds_user_id, ME);
        assert_eq!(load_settled(&b).unwrap().ds_user_id, other);

        a.delete().unwrap();
        assert!(a.load().unwrap().is_none(), "the deleted session is back");
        assert_eq!(
            load_settled(&b)
                .expect("deleting one account took the other")
                .ds_user_id,
            other
        );

        b.delete().unwrap();
    }

    /// `purge` takes the single-account layout's session too, from every
    /// place it can be, beside every account's.
    #[test]
    fn purging_takes_the_single_account_session_with_it() {
        let (_tmp, store, _keyring) = file_store();
        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        store.legacy_session().save(&session).unwrap();
        mine(&store).save(&session).unwrap();

        store.delete_all(&[ME]).unwrap();

        assert!(!store.legacy_session().something_is_stored());
        assert!(!mine(&store).something_is_stored());
        for file in [
            store.paths.legacy_session_file(),
            store.paths.account(ME).session_file(),
        ] {
            assert!(!file.exists(), "{} survived a purge", file.display());
        }
    }

    /// What `purge` lists is what `delete_all` takes: the named accounts'
    /// sessions and the single-account layout's, and no other account's.
    #[test]
    fn a_stored_session_is_found_where_purge_will_look() {
        let (_tmp, store, _keyring) = file_store();
        let session = Session::from_sessionid(SID, UA, SessionOrigin::Paste).unwrap();
        let other = snob_core::Pk::new(81_234_567_890);
        assert!(!store.sessions_stored(&[ME, other]));

        mine(&store).save(&session).unwrap();
        assert!(store.sessions_stored(&[ME]));
        assert!(!store.sessions_stored(&[other]));

        store.delete_all(&[ME]).unwrap();
        store.legacy_session().save(&session).unwrap();
        assert!(store.sessions_stored(&[]));
        store.delete_all(&[]).unwrap();
    }
}
