//! Opening a snapshot: its manifest, and the bodies it carries.

use super::manifest::{MANIFEST_NAME, Manifest};
use crate::error::{Error, Result};
use camino::{Utf8Path, Utf8PathBuf};
use std::collections::BTreeMap;
use std::io::Read as _;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub path: Utf8PathBuf,
    pub manifest: Manifest,
}

/// Resolves a snapshot by name inside the store, or by an explicit path.
pub fn locate(snapshots_dir: &Utf8Path, name_or_path: &str) -> Utf8PathBuf {
    let direct = Utf8PathBuf::from(name_or_path);
    if direct.exists() {
        return direct;
    }
    snapshots_dir.join(format!("{name_or_path}.tar.zst"))
}

pub fn open(path: &Utf8Path) -> Result<Snapshot> {
    let manifest_bytes = read_member(path, MANIFEST_NAME)?.ok_or_else(|| {
        Error::Usage(format!(
            "{path} has no {MANIFEST_NAME}: it does not look like an unsilo snapshot"
        ))
    })?;
    let manifest: Manifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| Error::json(path, e))?;
    if manifest.schema > super::manifest::SCHEMA {
        return Err(Error::Usage(format!(
            "the snapshot uses schema {} and this version understands up to {}",
            manifest.schema,
            super::manifest::SCHEMA
        )));
    }
    Ok(Snapshot { path: path.to_owned(), manifest })
}

/// Every body in the archive, keyed by its path inside it.
///
/// A snapshot holds one machine's conversations, so it is read whole rather than
/// streamed twice; the archive is compressed and the caller already decided to
/// restore from it.
pub fn read_bodies(path: &Utf8Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut out = BTreeMap::new();
    for_each_member(path, |name, bytes| {
        if name != MANIFEST_NAME {
            out.insert(name.to_owned(), bytes.to_vec());
        }
        Ok(())
    })?;
    Ok(out)
}

fn read_member(path: &Utf8Path, wanted: &str) -> Result<Option<Vec<u8>>> {
    let mut found = None;
    for_each_member(path, |name, bytes| {
        if name == wanted {
            found = Some(bytes.to_vec());
        }
        Ok(())
    })?;
    Ok(found)
}

fn for_each_member(
    path: &Utf8Path,
    mut visit: impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<()> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let decoder = zstd::Decoder::new(file).map_err(|e| Error::io(path, e))?;
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|e| Error::io(path, e))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| Error::io(path, e))?;
        let name =
            entry.path().map_err(|e| Error::io(path, e))?.to_string_lossy().replace('\\', "/");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| Error::io(path, e))?;
        visit(&name, &bytes)?;
    }
    Ok(())
}
