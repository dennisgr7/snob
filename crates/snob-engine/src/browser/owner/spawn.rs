//! Starting the owner of the browsers, apart from the command that needed it.
//!
//! **It keeps the command's environment, for its whole life**, and for the
//! owner that matters beyond itself: `PATH`, and with it which browser is found
//! on Linux; the locale and time zone the browser reports; the desktop session
//! Chromium picks its cookie keyring from.

use std::ffi::OsString;
use std::io;
use std::path::Path;

/// Past this, the log starts again, as the next owner starts.
const LOG_LIMIT: u64 = 1024 * 1024;

/// The owner just started, as far as the command that started it can tell.
pub(super) use crate::system::detach::Started;

/// Starts `program` with `args` as the owner, logging to `log`.
pub(super) fn spawn(program: &Path, args: &[OsString], log: &Path) -> io::Result<Started> {
    if std::fs::metadata(log).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(log, log.with_extension("log.old"));
    }
    crate::system::detach::spawn(program, args, None, log)
}
