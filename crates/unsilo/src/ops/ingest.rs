//! Bringing the index up to date with what is on disk.
//!
//! Implicit in every command that needs it, and idempotent, so nobody has to
//! remember to run it. Writes only inside the store: transcripts stay where
//! Claude put them.

use crate::attribution::{self, Activity, Sighting};
use crate::claude::identity::{self, Identities};
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
    /// Moments at which some account was known to be signed in.
    pub sightings: usize,
    /// `(attributed, needing attribution)` over CLI-born conversations.
    pub attributed: (usize, usize),
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
    summary.sightings = record_sightings(env, index, now)?;
    summary.attributed = attribute(index)?;

    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    summary.identities_learned = identities.learn_from(&env.home);
    if summary.identities_learned > 0 {
        identities.save(&path)?;
    }

    Ok(summary)
}

/// Remembers which account was signed in, so a conversation that records none can
/// be placed later.
///
/// Three sources, all cheap. What is signed in now. What Claude's rotating config
/// backups still show, which reaches a little way back. And the timestamps of
/// every desktop entry, since an entry names its own account, which reaches back
/// as far as the desktop sessions go.
fn record_sightings(env: &Env, index: &Index, now: i64) -> Result<usize> {
    let mut sightings: Vec<Sighting> = Vec::new();

    if let Some(active) = identity::active(&env.home) {
        sightings.push(Sighting {
            account: active.account,
            org: active.org,
            at_ms: now,
            source: attribution::Source::Observed,
        });
    }
    sightings.extend(identity::sightings_from_backups(&env.home));

    for root in &env.user_data {
        for entry in desktop::inventory(root).entries {
            for at_ms in [entry.created_at_ms, entry.last_activity_ms].into_iter().flatten() {
                sightings.push(Sighting {
                    account: entry.scope.account.clone(),
                    org: entry.scope.org.clone(),
                    at_ms,
                    source: attribution::Source::Entry,
                });
            }
        }
    }

    for sighting in &sightings {
        index.record_sighting(sighting)?;
    }
    Ok(index.sightings()?.len())
}

/// Recomputed from scratch every run: sightings only ever accumulate, so an
/// inference can become possible, or become ambiguous, as more is known.
fn attribute(index: &Index) -> Result<(usize, usize)> {
    let sightings = index.sightings()?;
    index.clear_inferences()?;
    for (session_id, created_at_ms, modified_at_ms) in index.unattributed_sessions()? {
        let activity = Activity { created_at_ms, modified_at_ms };
        if let Some((account, org)) = attribution::infer(activity, &sightings) {
            index.set_inferred(&session_id, &account, &org)?;
        }
    }
    index.attribution_coverage()
}

/// The identity map after ingest, which the filter needs to turn an email into
/// an account uuid.
pub fn identities(env: &Env) -> Result<Identities> {
    let path = env.unsilo_home.join("identities.json");
    let mut identities = Identities::load(&path)?;
    identities.learn_from(&env.home);
    Ok(identities)
}
