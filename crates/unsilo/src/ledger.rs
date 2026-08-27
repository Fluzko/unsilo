//! The record of everything Unsilo wrote outside its own store.
//!
//! Written before the file it describes, not after. If the process dies between
//! the two, the next run finds a pending row and reconciles it. Without that
//! order a crash would leave a projected file that `off` could never find, which
//! is the one way this tool could quietly become impossible to uninstall.

use crate::error::Result;
use crate::fsx;
use crate::index::{Index, LedgerEntry};
use camino::Utf8Path;
use serde::Serialize;

pub const KIND_DESKTOP_ENTRY: &str = "desktop_entry";
/// An entry built from a CLI transcript rather than copied from another account.
pub const KIND_ADOPTED_ENTRY: &str = "adopted_entry";
pub const KIND_TRANSCRIPT_LINK: &str = "transcript_link";

pub const STATE_PENDING: &str = "pending";
pub const STATE_DONE: &str = "done";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Disposition {
    /// Present, unchanged since we wrote it, safe to remove.
    Removable,
    /// Changed after we wrote it, so it now holds something we did not put
    /// there. Kept, and reported.
    Modified,
    /// Already gone. Nothing to do but forget the row.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct Judged {
    pub entry: LedgerEntry,
    pub disposition: Disposition,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Reconciled {
    /// Pending rows whose file turned out to be there and correct.
    pub completed: usize,
    /// Pending rows with nothing on disk: the crash happened before the write.
    pub dropped: usize,
}

#[derive(Debug)]
pub struct Ledger<'a> {
    index: &'a Index,
}

impl<'a> Ledger<'a> {
    #[must_use]
    pub fn new(index: &'a Index) -> Self {
        Self { index }
    }

    /// Announce the write before making it.
    pub fn begin(
        &self,
        path: &Utf8Path,
        kind: &str,
        session_id: Option<&str>,
        host_id: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        self.index.ledger_begin(path, kind, session_id, host_id, now_ms)
    }

    /// Confirm it, recording what was written so `off` can tell later whether
    /// anyone else has touched it.
    pub fn commit(&self, path: &Utf8Path, bytes: &[u8]) -> Result<()> {
        self.index.ledger_commit(path, &fsx::hash_bytes(bytes), bytes.len() as u64)
    }

    pub fn forget(&self, path: &Utf8Path) -> Result<()> {
        self.index.ledger_forget(path)
    }

    pub fn entries(&self) -> Result<Vec<LedgerEntry>> {
        self.index.ledger_entries()
    }

    /// Resolves rows left pending by an interrupted run. A file that is there
    /// and matches becomes ours; one that never appeared is forgotten.
    pub fn reconcile(&self) -> Result<Reconciled> {
        let mut out = Reconciled::default();
        for entry in self.entries()? {
            if entry.state != STATE_PENDING {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&entry.path) {
                self.commit(&entry.path, &bytes)?;
                out.completed += 1;
            } else {
                self.forget(&entry.path)?;
                out.dropped += 1;
            }
        }
        Ok(out)
    }

    /// What `off` should do with each entry, decided before anything is removed.
    pub fn judge(&self) -> Result<Vec<Judged>> {
        self.entries()?
            .into_iter()
            .map(|entry| {
                let disposition = disposition_of(&entry);
                Ok(Judged { entry, disposition })
            })
            .collect()
    }
}

fn disposition_of(entry: &LedgerEntry) -> Disposition {
    let Ok(bytes) = std::fs::read(&entry.path) else { return Disposition::Missing };
    match (&entry.content_hash, entry.byte_len) {
        (Some(hash), Some(len)) => {
            let matches = i64::try_from(bytes.len()).unwrap_or(i64::MAX) == len
                && fsx::hash_bytes(&bytes) == *hash;
            // The real case: a projected entry that Claude rewrote when the
            // session was resumed under the account it was projected into.
            if matches { Disposition::Removable } else { Disposition::Modified }
        }
        // Never confirmed, so we cannot claim it. Leaving it is the safe half
        // of the mistake.
        _ => Disposition::Modified,
    }
}
