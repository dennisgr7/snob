//! How much room files take, read from their metadata alone: **nothing is
//! opened**, a database least of all, so measuring changes nothing it
//! measures.

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

#[cfg(test)]
mod tests {
    use super::*;

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
