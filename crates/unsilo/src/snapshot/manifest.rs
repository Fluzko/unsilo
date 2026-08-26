//! What a snapshot contains and where each file came from.
//!
//! The manifest is the part that makes a snapshot restorable onto a machine
//! whose paths differ, so it records the original location of every file rather
//! than relying on the archive layout to encode it.

use crate::claude::identity::Active;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA: u32 = 1;
pub const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Claude's own state: transcripts and desktop index entries.
    Claude,
    /// Unsilo's state: the store, the index and the ledger.
    Store,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Transcript,
    Subagent,
    DesktopEntry,
    Tombstone,
    StoreFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Path inside the archive.
    pub archive_path: String,
    /// Where the file lived when it was captured.
    pub origin: Utf8PathBuf,
    pub kind: EntryKind,
    /// Bytes captured. A transcript keeps growing, so this pins which prefix the
    /// hash describes.
    pub len: u64,
    pub sha256: String,
    /// For transcripts: the session it belongs to.
    pub session_id: Option<String>,
    /// For desktop entries and tombstones: which list they belonged to.
    pub account: Option<String>,
    pub org: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub scope: Scope,
    pub created_at_ms: i64,
    pub unsilo_version: String,
    /// The account signed in when the snapshot was taken, for context on restore.
    pub active: Option<Active>,
    /// uuid to email and uuid to organization name, so a restored snapshot can
    /// still say who a session belonged to.
    pub accounts: BTreeMap<String, String>,
    pub orgs: BTreeMap<String, String>,
    /// Whether file bodies are in the archive or only their hashes.
    pub has_bodies: bool,
    pub entries: Vec<Entry>,
}

impl Manifest {
    #[must_use]
    pub fn count(&self, kind: EntryKind) -> usize {
        self.entries.iter().filter(|e| e.kind == kind).count()
    }

    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.len).sum()
    }

    #[must_use]
    pub fn find(&self, archive_path: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.archive_path == archive_path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn entry(kind: EntryKind, len: u64) -> Entry {
        Entry {
            archive_path: format!("{kind:?}/{len}"),
            origin: "/x".into(),
            kind,
            len,
            sha256: "abc".to_owned(),
            session_id: None,
            account: None,
            org: None,
        }
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let manifest = Manifest {
            schema: SCHEMA,
            scope: Scope::Claude,
            created_at_ms: 1,
            unsilo_version: "0.1.0".to_owned(),
            active: None,
            accounts: BTreeMap::new(),
            orgs: BTreeMap::new(),
            has_bodies: true,
            entries: vec![entry(EntryKind::Transcript, 10)],
        };
        let text = serde_json::to_string(&manifest).unwrap();
        let back: Manifest = serde_json::from_str(&text).unwrap();
        assert_eq!(back.entries, manifest.entries);
        assert_eq!(back.scope, Scope::Claude);
    }

    #[test]
    fn counts_and_sizes_come_from_the_entries() {
        let manifest = Manifest {
            schema: SCHEMA,
            scope: Scope::Claude,
            created_at_ms: 0,
            unsilo_version: String::new(),
            active: None,
            accounts: BTreeMap::new(),
            orgs: BTreeMap::new(),
            has_bodies: false,
            entries: vec![
                entry(EntryKind::Transcript, 10),
                entry(EntryKind::Transcript, 20),
                entry(EntryKind::DesktopEntry, 5),
            ],
        };
        assert_eq!(manifest.count(EntryKind::Transcript), 2);
        assert_eq!(manifest.count(EntryKind::Tombstone), 0);
        assert_eq!(manifest.total_bytes(), 35);
    }
}
