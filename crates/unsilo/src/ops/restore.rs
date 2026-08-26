//! Putting a snapshot back.
//!
//! Append-only transcripts make conflict resolution exact rather than
//! heuristic. If what is on disk begins with exactly the bytes the snapshot
//! holds, it is the same conversation further along and is left alone. Only a
//! prefix that differs is a real divergence, and those stop the run.
//!
//! Restored files are Claude's own, not projections, so they are not recorded in
//! the ledger: `off` has no business removing something a restore put back.

use crate::env::Env;
use crate::error::{Error, Result};
use crate::fsx;
use crate::snapshot::manifest::{Entry, EntryKind};
use crate::snapshot::read::{self, Snapshot};
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing on disk. Put it back.
    Restore,
    /// Byte for byte what the snapshot holds.
    Identical,
    /// The local file starts with the snapshot's bytes, so it is the same
    /// history with more appended. Newer, and left alone.
    LocalIsNewer,
    /// The prefixes differ: two histories, and we have no basis to pick.
    Conflict,
}

#[derive(Debug, Clone, Serialize)]
pub struct Planned {
    pub archive_path: String,
    pub target: Utf8PathBuf,
    pub kind: EntryKind,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub dry_run: bool,
    /// Overwrite divergent files. Only ever on an explicit request.
    pub force: bool,
    pub skip_conflicts: bool,
    /// `OLD=NEW` prefix rewrites, for importing another machine's snapshot.
    pub rewrite_cwd: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: u32,
    pub snapshot: Utf8PathBuf,
    pub scope: crate::snapshot::Scope,
    pub planned: Vec<Planned>,
    pub restored: usize,
    pub skipped: usize,
    pub conflicts: usize,
    pub dry_run: bool,
}

impl Report {
    #[must_use]
    pub fn pending(&self) -> usize {
        self.planned.iter().filter(|p| p.verdict == Verdict::Restore).count()
    }
}

pub fn run(env: &Env, name_or_path: &str, options: &Options) -> Result<Report> {
    let path = read::locate(&env.snapshots_dir(), name_or_path);
    let snapshot = read::open(&path)?;
    if !snapshot.manifest.has_bodies {
        return Err(Error::Usage(format!("{path} is metadata only: there is nothing to restore")));
    }
    let bodies = read::read_bodies(&path)?;
    let guard = env.guard();

    let mut planned = Vec::new();
    for entry in &snapshot.manifest.entries {
        let target = retarget(env, &snapshot, entry, &options.rewrite_cwd);
        let verdict = judge(&target, entry)?;
        planned.push(Planned {
            archive_path: entry.archive_path.clone(),
            target,
            kind: entry.kind,
            verdict,
        });
    }

    let conflicts = planned.iter().filter(|p| p.verdict == Verdict::Conflict).count();
    if conflicts > 0 && !options.force && !options.skip_conflicts {
        return Err(Error::Usage(format!(
            "{conflicts} file(s) diverge from the snapshot. \
             use --skip-conflicts to leave them or --force to overwrite them"
        )));
    }

    let mut report = Report {
        schema: 1,
        snapshot: path.clone(),
        scope: snapshot.manifest.scope,
        restored: 0,
        skipped: 0,
        conflicts,
        dry_run: options.dry_run,
        planned,
    };

    for item in &report.planned {
        let write_it = match item.verdict {
            Verdict::Restore => true,
            Verdict::Conflict => options.force,
            Verdict::Identical | Verdict::LocalIsNewer => false,
        };
        if !write_it {
            report.skipped += 1;
            continue;
        }
        if options.dry_run {
            report.restored += 1;
            continue;
        }
        let Some(bytes) = bodies.get(&item.archive_path) else { continue };
        guard.check(&item.target)?;
        fsx::write_atomic(&guard, &item.target, bytes)?;
        report.restored += 1;
    }

    Ok(report)
}

/// Where a snapshotted file belongs on this machine. The recorded origin is
/// used as is unless a prefix rewrite applies, which is what makes importing
/// another machine's snapshot possible.
fn retarget(
    env: &Env,
    snapshot: &Snapshot,
    entry: &Entry,
    rewrites: &[(String, String)],
) -> Utf8PathBuf {
    let mut path = entry.origin.to_string();
    for (from, to) in rewrites {
        if path.starts_with(from.as_str()) {
            path = format!("{to}{}", path.get(from.len()..).unwrap_or_default());
            break;
        }
    }
    let candidate = Utf8PathBuf::from(path);
    if env.guard().check(&candidate).is_ok() {
        return candidate;
    }
    // The snapshot came from a different home. Re-root it under ours rather than
    // writing somewhere we have no claim to.
    reroot(env, snapshot, entry).unwrap_or(candidate)
}

fn reroot(env: &Env, snapshot: &Snapshot, entry: &Entry) -> Option<Utf8PathBuf> {
    let _ = snapshot;
    let rest = entry.archive_path.split_once('/')?.1;
    match entry.kind {
        EntryKind::Transcript | EntryKind::Subagent => {
            Some(env.config_dirs.first()?.join("projects").join(rest))
        }
        EntryKind::DesktopEntry | EntryKind::Tombstone => Some(env.user_data.first()?.join(rest)),
        EntryKind::StoreFile => Some(env.unsilo_home.join(rest)),
    }
}

fn judge(target: &Utf8Path, entry: &Entry) -> Result<Verdict> {
    let Ok(metadata) = std::fs::metadata(target) else { return Ok(Verdict::Restore) };
    let local_len = metadata.len();
    if local_len == entry.len && fsx::hash_prefix(target, entry.len)? == entry.sha256 {
        return Ok(Verdict::Identical);
    }
    if local_len > entry.len && fsx::hash_prefix(target, entry.len)? == entry.sha256 {
        return Ok(Verdict::LocalIsNewer);
    }
    Ok(Verdict::Conflict)
}
