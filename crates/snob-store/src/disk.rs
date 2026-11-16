//! How much room files take, read from their metadata alone: **nothing is
//! opened**, a database least of all, so measuring changes nothing it
//! measures. And how large the disk holding them is, and how much of it is
//! free, as the system says ([`volume`]).

use std::path::Path;

/// What a set of files weighs: their lengths added up, not the blocks the
/// disk gave them, and how many there are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    pub files: u64,
}

impl Size {
    /// The two added together.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        Self {
            bytes: self.bytes.saturating_add(other.bytes),
            files: self.files.saturating_add(other.files),
        }
    }
}

impl std::ops::AddAssign for Size {
    fn add_assign(&mut self, other: Self) {
        *self = self.plus(other);
    }
}

/// What `path` weighs: a file alone, or every file under a directory. **A
/// link is never followed**, and neither counted: what it points at is not
/// snob's. What cannot be read counts nothing.
pub fn tree(path: &Path) -> Size {
    let Ok(found) = std::fs::symlink_metadata(path) else {
        return Size::default();
    };
    if found.is_file() {
        return Size {
            bytes: found.len(),
            files: 1,
        };
    }
    if !found.is_dir() {
        return Size::default();
    }
    let mut size = Size::default();
    let mut pending = vec![path.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file()
                && let Ok(meta) = entry.metadata()
            {
                size += Size {
                    bytes: meta.len(),
                    files: 1,
                };
            }
        }
    }
    size
}

/// The disk a path is on: how large it is, and how much of it this user
/// may still write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Volume {
    pub total: u64,
    pub free: u64,
}

/// The disk `path` is on, as the system says: `GetDiskFreeSpaceExW` on
/// Windows, `statvfs` elsewhere. `None` when it cannot be asked.
pub fn volume(path: &Path) -> Option<Volume> {
    let path = nearest_existing(path)?;
    asked(&path)
}

/// `path`, or the nearest directory above it that exists: the disk is asked
/// about a path that is there.
fn nearest_existing(path: &Path) -> Option<std::path::PathBuf> {
    path.ancestors()
        .find(|at| at.exists())
        .map(Path::to_path_buf)
}

#[cfg(windows)]
fn asked(path: &Path) -> Option<Volume> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total, mut all_free) = (0u64, 0u64, 0u64);
    // SAFETY: `wide` is a NUL-terminated wide string that outlives the call,
    // and the three out-parameters are this frame's own `u64`s.
    let done = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut all_free) };
    (done != 0).then_some(Volume { total, free })
}

#[cfg(unix)]
fn asked(path: &Path) -> Option<Volume> {
    use std::os::unix::ffi::OsStrExt;

    let named = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` is plain data, for which all zeroes is a valid value.
    let mut told: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `named` is NUL-terminated and outlives the call, and `told` is
    // this frame's own.
    if unsafe { libc::statvfs(named.as_ptr(), &mut told) } != 0 {
        return None;
    }
    #[allow(clippy::useless_conversion)] // the fields' widths differ by platform
    let block = u64::from(told.f_frsize);
    #[allow(clippy::useless_conversion)]
    let (blocks, available) = (u64::from(told.f_blocks), u64::from(told.f_bavail));
    Some(Volume {
        total: blocks.saturating_mul(block),
        free: available.saturating_mul(block),
    })
}

#[cfg(not(any(windows, unix)))]
fn asked(_: &Path) -> Option<Volume> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disk_a_directory_is_on_is_larger_than_what_is_free_on_it() {
        let root = tempfile::tempdir().unwrap();
        let volume = volume(&root.path().join("not/made/yet")).expect("the disk answers");
        assert!(volume.total > 0);
        assert!(volume.free <= volume.total, "{volume:?}");
    }

    #[test]
    fn a_tree_weighs_every_file_under_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("a/b")).unwrap();
        std::fs::write(root.path().join("one"), b"12345").unwrap();
        std::fs::write(root.path().join("a/b/two"), b"123").unwrap();

        assert_eq!(tree(root.path()), Size { bytes: 8, files: 2 });
        assert_eq!(tree(&root.path().join("one")), Size { bytes: 5, files: 1 });
        assert_eq!(tree(&root.path().join("missing")), Size::default());
    }
}
