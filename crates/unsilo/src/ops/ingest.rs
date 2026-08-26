//! Bringing the index up to date with what is on disk.
//!
//! Implicit in every command that needs it, and idempotent, so nobody has to
//! remember to run it. Writes only inside the store: transcripts stay where
//! Claude put them.

use crate::claude::identity::Identities;
use crate::claude::{desktop, transcript};
use crate::env::Env;
use crate::error::Result;
use crate::index::Index;
use crate::store::{Outcome, Store};
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub sessions: usize,
    pub subagents: usize,
    pub skipped: usize,
    pub desktop_entries: usize,
    pub tombstones: usize,
    /// Labels captured that would otherwise have rotated out of Claude's backups.
    pub identities_learned: usize,
    pub linked: usize,
    pub copies: usize,
    /// Sessions whose stored bytes and live bytes are different histories.
    pub diverged: Vec<String>,
    /// Sessions whose transcript is gone from its project dir but survives in
    /// the store.
    pub recovered_from_store: usize,
    pub unreadable: Vec<String>,
}

fn link_kind_name(kind: crate::env::LinkKind) -> &'static str {
    match kind {
        crate::env::LinkKind::Hard => "hardlink",
        crate::env::LinkKind::Copy => "copy",
    }
}

pub fn run(env: &Env, index: &Index) -> Result<Summary> {
    let mut summary = Summary::default();
    let now = env.clock.now_ms();
    let store = Store::new(env);

    index.clear_desktop()?;

    for dir in &env.config_dirs {
        let scan = transcript::scan(dir);
        summary.subagents += scan.subagents;
        summary.skipped += scan.skipped;
        for (path, err) in &scan.unreadable {
            summary.unreadable.push(format!("{path}: {err}"));
        }
        for meta in &scan.sessions {
            index.upsert_session(meta, now)?;
            summary.sessions += 1;

            let ingested = store.ingest(meta)?;
            match ingested.outcome {
                Outcome::Linked | Outcome::Refreshed => summary.linked += 1,
                Outcome::AlreadyLinked => {}
                Outcome::Diverged => summary.diverged.push(meta.session_id.clone()),
            }
            if ingested.kind == crate::env::LinkKind::Copy {
                summary.copies += 1;
            }
            index.record_store_link(
                &meta.session_id,
                &ingested.path,
                link_kind_name(ingested.kind),
                crate::fsx::file_id(&ingested.path),
            )?;
        }
    }

    for root in &env.user_data {
        let inventory = desktop::inventory(root);
        for entry in &inventory.entries {
            index.upsert_desktop_entry(entry)?;
            summary.desktop_entries += 1;
        }
        for tombstone in &inventory.tombstones {
            index.upsert_tombstone(tombstone)?;
            summary.tombstones += 1;
        }
        for (path, err) in &inventory.unreadable {
            summary.unreadable.push(format!("{path}: {err}"));
        }
    }

    // A transcript Claude's retention cleanup removed is still in the store, and
    // still belongs to the project dir it came from. Keeping its row is what
    // makes putting it back possible at all.
    let mut kept_from_store = Vec::new();
    if let Ok(entries) = std::fs::read_dir(store.transcripts_dir()) {
        for entry in entries.flatten() {
            let Ok(path) = camino::Utf8PathBuf::from_path_buf(entry.path()) else { continue };
            let Some(session_id) = path.file_stem() else { continue };
            if !crate::claude::transcript::is_session_uuid(session_id) {
                continue;
            }
            kept_from_store.push(session_id.to_owned());
            if index.origin_dir_of(session_id)?.is_some() {
                continue;
            }
            // Never indexed before: recover what we can from the stored copy.
            if let Ok(Some(mut meta)) = crate::claude::transcript::parse(&path) {
                if let Some(cwd) = meta.cwd.clone() {
                    let slug = crate::claude::slug::slug_lossy(&cwd);
                    if let Some(config) = env.config_dirs.first() {
                        meta.origin_dir = config.join("projects").join(slug);
                    }
                }
                index.upsert_session(&meta, now)?;
            }
        }
    }
    summary.recovered_from_store = index.forget_unseen(now, &kept_from_store)?;

    index.mark_projected_entries()?;

    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    summary.identities_learned = identities.learn_from(&env.home);
    if summary.identities_learned > 0 {
        identities.save(&path)?;
    }

    Ok(summary)
}

/// The identity map after ingest, which the filter needs to turn an email into
/// an account uuid.
pub fn identities(env: &Env) -> Result<Identities> {
    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    identities.learn_from(&env.home);
    Ok(identities)
}
