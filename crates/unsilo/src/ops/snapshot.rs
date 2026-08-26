//! Taking a snapshot, and taking one automatically before anything is written.

use crate::env::Env;
use crate::error::Result;
use crate::snapshot::manifest::Scope;
use crate::snapshot::write::{self, Options, Written};

/// The first snapshot of an installation, taken before Unsilo writes anything.
/// It is never rotated and never overwritten: it is what `off` returns to.
pub const BASELINE: &str = "baseline";

pub fn run(env: &Env, scope: Scope, name: &str, options: Options) -> Result<Written> {
    match scope {
        Scope::Claude => write::claude_snapshot(env, name, options),
        Scope::Store => write::store_snapshot(env, name, options),
    }
}

/// Captures the untouched installation the first time it is needed. Returns
/// `None` when it already exists, which is the normal case after the first run.
pub fn ensure_baseline(env: &Env) -> Result<Option<Written>> {
    let path = env.snapshots_dir().join(format!("{BASELINE}.tar.zst"));
    if path.exists() {
        return Ok(None);
    }
    write::claude_snapshot(env, BASELINE, Options::default()).map(Some)
}

/// A timestamped snapshot taken before a writing command. Rotated, unlike the
/// baseline.
pub fn auto(env: &Env, command: &str) -> Result<Written> {
    let name = format!("auto-{command}-{}", env.clock.now_ms());
    write::claude_snapshot(env, &name, Options::default())
}

/// Keeps the newest `keep` automatic snapshots. Named snapshots and the baseline
/// are never touched.
pub fn rotate_auto(env: &Env, keep: usize) -> Result<usize> {
    let dir = env.snapshots_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Ok(0) };
    let mut autos: Vec<camino::Utf8PathBuf> = entries
        .flatten()
        .filter_map(|e| camino::Utf8PathBuf::from_path_buf(e.path()).ok())
        .filter(|p| p.file_name().is_some_and(|n| n.starts_with("auto-")))
        .collect();
    // Names carry a millisecond timestamp, so lexical order is chronological.
    autos.sort();

    let guard = env.guard();
    let mut removed = 0;
    while autos.len() > keep {
        let oldest = autos.remove(0);
        crate::fsx::remove_file(&guard, &oldest)?;
        removed += 1;
    }
    Ok(removed)
}
