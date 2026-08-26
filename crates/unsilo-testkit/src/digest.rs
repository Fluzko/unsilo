//! Content plus link identity for a whole tree.
//!
//! Comparing bytes alone is not enough: a leaked hard link changes no file
//! content, so `link_groups` records which paths share an inode. That is what
//! makes the apply-then-off identity test meaningful.

use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDigest {
    pub sha256: String,
    pub len: u64,
    /// Unix permission bits. `None` elsewhere, since Windows has no equivalent.
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeDigest {
    pub files: BTreeMap<String, FileDigest>,
    pub link_groups: Vec<BTreeSet<String>>,
}

impl TreeDigest {
    pub fn of(root: &Utf8Path) -> Self {
        Self::of_many(&[("", root)])
    }

    /// Several roots as one digest, each prefixed so their paths cannot collide.
    /// Claude's state lives in two separate trees, and a comparison that covers
    /// only one of them would miss exactly the writes this tool makes.
    pub fn of_many(roots: &[(&str, &Utf8Path)]) -> Self {
        let mut files = BTreeMap::new();
        let mut by_id: BTreeMap<(u64, u64), BTreeSet<String>> = BTreeMap::new();
        for (label, root) in roots {
            let mut part = BTreeMap::new();
            walk(root, root, &mut part, &mut by_id);
            for (path, digest) in part {
                let key = if label.is_empty() { path } else { format!("{label}/{path}") };
                files.insert(key, digest);
            }
        }
        let link_groups = by_id.into_values().filter(|g| g.len() > 1).collect();
        Self { files, link_groups }
    }

    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        self.files.keys().map(String::as_str).collect()
    }

    /// Paths present in `other` but not in `self`. Reads better in a failure
    /// message than a raw inequality between two large maps.
    #[must_use]
    pub fn added_in(&self, other: &TreeDigest) -> Vec<String> {
        other.files.keys().filter(|k| !self.files.contains_key(*k)).cloned().collect()
    }

    #[must_use]
    pub fn removed_in(&self, other: &TreeDigest) -> Vec<String> {
        self.files.keys().filter(|k| !other.files.contains_key(*k)).cloned().collect()
    }

    #[must_use]
    pub fn changed_in(&self, other: &TreeDigest) -> Vec<String> {
        self.files
            .iter()
            .filter(|(k, v)| other.files.get(*k).is_some_and(|o| o != *v))
            .map(|(k, _)| k.clone())
            .collect()
    }
}

fn walk(
    root: &Utf8Path,
    dir: &Utf8Path,
    files: &mut BTreeMap<String, FileDigest>,
    by_id: &mut BTreeMap<(u64, u64), BTreeSet<String>>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(path) = Utf8PathBuf::from_path_buf(e.path()) else { continue };
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(root, &path, files, by_id);
            continue;
        }
        if !ft.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let rel = path.strip_prefix(root).unwrap_or(&path).as_str().replace('\\', "/");
        if let Some(id) = file_id(&path) {
            by_id.entry(id).or_default().insert(rel.clone());
        }
        files.insert(
            rel,
            FileDigest {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                len: bytes.len() as u64,
                mode: mode_of(&path),
            },
        );
    }
}

fn file_id(path: &Utf8Path) -> Option<(u64, u64)> {
    unsilo::fsx::file_id(path)
}

#[cfg(unix)]
fn mode_of(path: &Utf8Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(std::fs::metadata(path).ok()?.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_path: &Utf8Path) -> Option<u32> {
    None
}
