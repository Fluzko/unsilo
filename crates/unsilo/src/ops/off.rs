//! Turning Unsilo off: removing what it put outside its own store, and nothing
//! else.
//!
//! Two rules. Only paths the ledger recorded are candidates, and only those
//! whose bytes still hash to what was recorded are removed. Anything Claude has
//! since rewritten is kept and reported, because it now holds something we did
//! not put there.
//!
//! The store is never touched. Retention cleanup means it can be the last
//! remaining copy of a transcript, so removing it is destroying data, not
//! clearing a cache. `--purge` is the only path that does, and it demands a
//! store snapshot first.

use crate::env::Env;
use crate::error::{Error, Result};
use crate::fsx;
use crate::index::Index;
use crate::ledger::{Disposition, Ledger};
use crate::snapshot::manifest::Scope;
use crate::snapshot::read;
use camino::Utf8PathBuf;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub dry_run: bool,
    /// Also delete the store and the index. Destructive.
    pub purge: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: u32,
    pub removed: Vec<Utf8PathBuf>,
    pub kept: Vec<Utf8PathBuf>,
    pub missing: Vec<Utf8PathBuf>,
    pub store_transcripts: usize,
    pub purged: bool,
    pub dry_run: bool,
}

pub fn run(env: &Env, options: &Options) -> Result<Report> {
    let mut report = Report {
        schema: 1,
        removed: Vec::new(),
        kept: Vec::new(),
        missing: Vec::new(),
        store_transcripts: count_store(env),
        purged: false,
        dry_run: options.dry_run,
    };

    let guard = env.guard();
    // Scoped so the database connection is closed before anything considers
    // deleting the file behind it. Windows refuses to remove an open file, and a
    // purge that only works on unix is not a purge.
    {
        let index = Index::open(&env.index_path())?;
        let ledger = Ledger::new(&index);
        ledger.reconcile()?;

        for judged in ledger.judge()? {
            match judged.disposition {
                Disposition::Removable => {
                    if !options.dry_run {
                        fsx::remove_file(&guard, &judged.entry.path)?;
                        ledger.forget(&judged.entry.path)?;
                    }
                    report.removed.push(judged.entry.path);
                }
                Disposition::Modified => report.kept.push(judged.entry.path),
                Disposition::Missing => {
                    if !options.dry_run {
                        ledger.forget(&judged.entry.path)?;
                    }
                    report.missing.push(judged.entry.path);
                }
            }
        }
    }

    if options.purge {
        require_store_snapshot(env)?;
        if !options.dry_run {
            purge(env)?;
        }
        report.purged = true;
    }

    Ok(report)
}

/// The store can hold the only surviving copy of a transcript, so purging it
/// without a snapshot is unrecoverable. Refused rather than confirmed away.
fn require_store_snapshot(env: &Env) -> Result<()> {
    let dir = env.snapshots_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Err(Error::Usage(format!(
            "--purge needs a store snapshot: run `unsilo snapshot store --name <name>` \
             ({dir} does not exist)"
        )));
    };
    let has_store_snapshot = entries
        .flatten()
        .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter_map(|p| read::open(&p).ok())
        .any(|s| s.manifest.scope == Scope::Store && s.manifest.has_bodies);

    if has_store_snapshot {
        return Ok(());
    }
    Err(Error::Usage(
        "--purge needs a complete store snapshot: the store can be the last remaining \
         copy of a transcript. run `unsilo snapshot store --name <name>` first"
            .to_owned(),
    ))
}

fn purge(env: &Env) -> Result<()> {
    let guard = env.guard();
    let index = env.index_path();
    // The write-ahead log and shared-memory files sit beside the database and
    // would otherwise be left behind.
    let sidecars = ["index.sqlite-wal", "index.sqlite-shm"].map(|n| env.unsilo_home.join(n));

    guard.check(&env.store_dir())?;
    if env.store_dir().is_dir() {
        std::fs::remove_dir_all(env.store_dir()).map_err(|e| Error::io(env.store_dir(), e))?;
    }
    for path in std::iter::once(index).chain(sidecars) {
        fsx::remove_file(&guard, &path)?;
    }
    Ok(())
}

fn count_store(env: &Env) -> usize {
    std::fs::read_dir(env.store_dir().join("transcripts")).map_or(0, |rd| {
        rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")).count()
    })
}
