//! Making the selected conversations visible under the account signed in now.
//!
//! Declarative, not additive: the filter describes the whole visible set, so
//! applying with narrower filters removes what no longer belongs as well as
//! adding what does. Running it twice changes nothing the second time.

use crate::claude::desktop::{self, Entry, Inventory, Scope, Surface};
use crate::claude::identity::{self, Active};
use crate::claude::layout::{Fingerprint, Version};
use crate::env::Env;
use crate::error::{Error, Result};
use crate::filter::Filter;
use crate::fsx;
use crate::index::Index;
use crate::ledger::{self, Disposition, Ledger};
use crate::ops::{ingest, snapshot};
use crate::store::Store;
use camino::Utf8PathBuf;
use serde::Serialize;
use std::collections::BTreeSet;

/// How many automatic snapshots to keep. Enough to step back through a few runs
/// without the store growing without bound.
const KEEP_AUTO_SNAPSHOTS: usize = 10;

/// Independent switches over one operation, so grouping them into a type would
/// only add a level of naming.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Options {
    pub dry_run: bool,
    /// Leave the account-scoped MCP payload in projected entries.
    pub keep_account_scoped: bool,
    /// Do not remove entries that fall outside the current filter.
    pub no_prune: bool,
    /// Build entries for conversations the desktop never knew about, so they
    /// appear in its list at all.
    pub adopt_cli_sessions: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Adoption {
    pub session_id: String,
    pub host_id: String,
    pub title: Option<String>,
    pub target: Utf8PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Projection {
    pub session_id: Option<String>,
    pub host_id: String,
    pub title: Option<String>,
    pub from: String,
    pub target: Utf8PathBuf,
    pub bytes: u64,
    pub stripped_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Relink {
    pub session_id: String,
    pub target: Utf8PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: u32,
    pub active: Active,
    pub selected: usize,
    pub projected: Vec<Projection>,
    /// Conversations the desktop never knew about, given an entry so it does.
    pub adopted: Vec<Adoption>,
    /// Conversations the desktop never knew about and still will not, because
    /// adoption was not asked for.
    pub adoptable: usize,
    pub relinked: Vec<Relink>,
    pub pruned: Vec<Utf8PathBuf>,
    /// Ledger entries Claude has since rewritten, which are kept.
    pub kept_modified: Vec<Utf8PathBuf>,
    pub already_visible: usize,
    pub baseline_created: bool,
    pub auto_snapshot: Option<Utf8PathBuf>,
    pub dry_run: bool,
}

impl Report {
    fn adoptions_push(&mut self, adoption: Adoption) {
        self.adopted.push(adoption);
    }

    #[must_use]
    pub fn changes(&self) -> usize {
        self.projected.len() + self.adopted.len() + self.relinked.len() + self.pruned.len()
    }
}

pub fn run(env: &Env, filter: &Filter, options: &Options) -> Result<Report> {
    let index = Index::open(&env.index_path())?;
    let ledger = Ledger::new(&index);
    // An interrupted run may have left a row without its file, or a file whose
    // row was never confirmed. Settle that before deciding anything else.
    ledger.reconcile()?;

    let summary = ingest::run(env, &index)?;
    guard_layout(env, &summary)?;

    let Some(active) = identity::active(&env.home) else {
        return Err(Error::Usage(
            "could not determine the active account from ~/.claude.json".to_owned(),
        ));
    };

    let identities = ingest::identities(env)?;
    let resolved = filter.resolve(&identities, env.clock.now_ms())?;
    let selected: BTreeSet<String> =
        index.query(&resolved)?.into_iter().map(|row| row.session_id).collect();

    let mut inventory = Inventory::default();
    for root in &env.user_data {
        let found = desktop::inventory(root);
        inventory.entries.extend(found.entries);
        inventory.tombstones.extend(found.tombstones);
    }

    let mut report = Report {
        schema: 1,
        active: active.clone(),
        selected: selected.len(),
        projected: Vec::new(),
        adopted: Vec::new(),
        adoptable: 0,
        relinked: Vec::new(),
        pruned: Vec::new(),
        kept_modified: Vec::new(),
        already_visible: 0,
        baseline_created: false,
        auto_snapshot: None,
        dry_run: options.dry_run,
    };

    plan_projections(env, &inventory, &active, &selected, options, &mut report);
    plan_adoptions(env, &index, &active, &selected, options, &mut report)?;
    plan_relinks(env, &index, &resolved, &selected, &mut report)?;
    if !options.no_prune {
        plan_prune(&ledger, &selected, &mut report)?;
        // The desktop may have created its own entry since we adopted one. Two
        // rows for one conversation would show it twice.
        report.pruned.extend(index.superseded_adoptions(&active.account, &active.org)?);
    }

    if options.dry_run {
        return Ok(report);
    }
    if report.changes() == 0 {
        return Ok(report);
    }

    // Nothing is written before there is a way back.
    report.baseline_created = snapshot::ensure_baseline(env)?.is_some();
    report.auto_snapshot = Some(snapshot::auto(env, "apply")?.path);

    execute(env, &ledger, &mut report)?;
    snapshot::rotate_auto(env, KEEP_AUTO_SNAPSHOTS)?;
    Ok(report)
}

/// Refuses to write into a layout we do not recognise. Reading it is fine;
/// writing blind is how one program corrupts another's data.
fn guard_layout(env: &Env, summary: &ingest::Summary) -> Result<()> {
    let _ = summary;
    let index = Index::open(&env.index_path())?;
    let mut fingerprint = Fingerprint {
        remote_backend: identity::remote_backend_enabled(&env.home, None),
        has_transcripts: index.count_sessions()? > 0,
        ..Fingerprint::default()
    };
    for dir in &env.config_dirs {
        for meta in crate::claude::transcript::scan(dir).sessions {
            if let Some(version) = meta.cli_version {
                *fingerprint.cli_versions.entry(version).or_insert(0) += 1;
            }
        }
    }
    fingerprint.newest_cli =
        fingerprint.cli_versions.keys().filter_map(|v| Version::parse(v)).max();

    let compat = fingerprint.compat();
    if compat.allows_writes() {
        return Ok(());
    }
    Err(Error::UnknownLayout(compat.reason().unwrap_or("unknown").to_owned()))
}

fn plan_projections(
    env: &Env,
    inventory: &Inventory,
    active: &Active,
    selected: &BTreeSet<String>,
    options: &Options,
    report: &mut Report,
) {
    let Some(user_data) = env.user_data.first() else { return };
    let target_dir = user_data.join(desktop::CODE_SESSIONS).join(&active.account).join(&active.org);

    let here: BTreeSet<&str> = inventory
        .entries
        .iter()
        .filter(|e| e.scope.account == active.account && e.scope.org == active.org)
        .map(|e| e.host_id.as_str())
        .collect();

    for entry in best_sources(inventory, selected) {
        if here.contains(entry.host_id.as_str()) {
            report.already_visible += 1;
            continue;
        }
        if inventory.is_tombstoned(&active.account, &active.org, entry) {
            // Deleted from this list on purpose. Putting it back is not our call.
            continue;
        }
        let projected = entry.projected(options.keep_account_scoped);
        let Ok(bytes) = serde_json::to_vec(&projected) else { continue };
        report.projected.push(Projection {
            session_id: entry.cli_session_id.clone(),
            host_id: entry.host_id.clone(),
            title: entry.title.clone(),
            from: format!("{}/{}", short(&entry.scope.account), short(&entry.scope.org)),
            target: target_dir.join(entry.file_name()),
            bytes: bytes.len() as u64,
            stripped_bytes: entry.size.saturating_sub(bytes.len() as u64),
        });
    }
}

/// One entry per session, preferring the most recently active, so a session that
/// exists in several accounts is projected from its liveliest copy.
fn best_sources<'a>(inventory: &'a Inventory, selected: &BTreeSet<String>) -> Vec<&'a Entry> {
    let mut candidates: Vec<&Entry> = inventory
        .entries
        .iter()
        .filter(|e| e.scope.surface == Surface::Code)
        .filter(|e| e.cli_session_id.as_ref().is_some_and(|id| selected.contains(id)))
        .collect();
    candidates.sort_by(|a, b| {
        b.last_activity_ms.cmp(&a.last_activity_ms).then_with(|| a.host_id.cmp(&b.host_id))
    });

    let mut seen = BTreeSet::new();
    candidates.into_iter().filter(|e| seen.insert(e.cli_session_id.clone())).collect()
}

/// A conversation with no entry anywhere is invisible in the desktop under every
/// account, so this is not about which account can see it but about whether the
/// desktop knows it exists at all.
fn plan_adoptions(
    env: &Env,
    index: &Index,
    active: &Active,
    selected: &BTreeSet<String>,
    options: &Options,
    report: &mut Report,
) -> Result<()> {
    let orphans: Vec<String> = index
        .sessions_without_any_entry()?
        .into_iter()
        .filter(|id| selected.contains(id))
        .collect();

    if !options.adopt_cli_sessions {
        // Reported even when not asked for, so the number is not a surprise the
        // first time someone passes the flag.
        report.adoptable = orphans.len();
        return Ok(());
    }
    let Some(user_data) = env.user_data.first() else { return Ok(()) };
    let target_dir = user_data.join(desktop::CODE_SESSIONS).join(&active.account).join(&active.org);

    for session_id in orphans {
        let Some(meta) = read_meta(index, &session_id)? else { continue };
        let host_id = crate::adopt::host_id_for(&session_id);
        let target = target_dir.join(format!("{host_id}.json"));
        if target.exists() {
            continue;
        }
        report.adoptions_push(Adoption {
            session_id,
            host_id,
            title: meta.display_title().map(ToOwned::to_owned),
            target,
        });
    }
    Ok(())
}

/// Re-reads the transcript, since an entry is built from what it says now rather
/// than from what the index remembered.
fn read_meta(index: &Index, session_id: &str) -> Result<Option<crate::claude::transcript::Meta>> {
    let Some(dir) = index.origin_dir_of(session_id)? else { return Ok(None) };
    let path = dir.join(format!("{session_id}.jsonl"));
    if !path.exists() {
        return Ok(None);
    }
    crate::claude::transcript::parse(&path)
}

fn plan_relinks(
    env: &Env,
    index: &Index,
    resolved: &crate::filter::Resolved,
    selected: &BTreeSet<String>,
    report: &mut Report,
) -> Result<()> {
    let store = Store::new(env);
    for row in index.query(resolved)? {
        if !selected.contains(&row.session_id) {
            continue;
        }
        let origin = Utf8PathBuf::from(&row.origin_dir);
        let target = origin.join(format!("{}.jsonl", row.session_id));
        // Claude's retention cleanup removed it; the store kept the inode alive.
        if target.exists() || !store.holds(&row.session_id) {
            continue;
        }
        report.relinked.push(Relink { session_id: row.session_id, target });
    }
    Ok(())
}

fn plan_prune(ledger: &Ledger, selected: &BTreeSet<String>, report: &mut Report) -> Result<()> {
    for judged in ledger.judge()? {
        if judged.entry.kind != ledger::KIND_DESKTOP_ENTRY {
            continue;
        }
        let still_wanted = judged.entry.session_id.as_ref().is_some_and(|id| selected.contains(id));
        if still_wanted {
            continue;
        }
        match judged.disposition {
            Disposition::Removable => report.pruned.push(judged.entry.path),
            // Claude rewrote it after we put it there, so it is no longer ours.
            Disposition::Modified => report.kept_modified.push(judged.entry.path),
            Disposition::Missing => {}
        }
    }
    Ok(())
}

fn execute(env: &Env, ledger: &Ledger, report: &mut Report) -> Result<()> {
    let guard = env.guard();
    let store = Store::new(env);
    let now = env.clock.now_ms();

    for projection in &report.projected {
        let Some(entry) = source_entry(env, &projection.host_id) else { continue };
        let bytes = serde_json::to_vec(&entry).map_err(|e| Error::json(&projection.target, e))?;
        // Row first, file second: a crash between them leaves something the next
        // run can resolve, never a file `off` cannot find.
        ledger.begin(
            &projection.target,
            ledger::KIND_DESKTOP_ENTRY,
            projection.session_id.as_deref(),
            Some(&projection.host_id),
            now,
        )?;
        env.failpoints.hit("apply.after_ledger_pending")?;
        fsx::write_atomic(&guard, &projection.target, &bytes)?;
        ledger.commit(&projection.target, &bytes)?;
    }

    for relink in &report.relinked {
        let Some(dir) = relink.target.parent() else { continue };
        // Deliberately not recorded in the ledger. This is Claude's own
        // transcript put back where Claude expects it, the same category as a
        // restore. Removing it on `off` would delete a conversation the user can
        // see, which is the opposite of turning a tool off.
        store.restore_into(&relink.session_id, dir)?;
    }

    for adoption in &report.adopted {
        let Some(meta) = read_meta(&index_for(env)?, &adoption.session_id)? else { continue };
        let entry = crate::adopt::entry_for(&meta, now);
        let bytes = serde_json::to_vec(&entry).map_err(|e| Error::json(&adoption.target, e))?;
        ledger.begin(
            &adoption.target,
            ledger::KIND_ADOPTED_ENTRY,
            Some(&adoption.session_id),
            Some(&adoption.host_id),
            now,
        )?;
        env.failpoints.hit("apply.after_ledger_pending")?;
        fsx::write_atomic(&guard, &adoption.target, &bytes)?;
        ledger.commit(&adoption.target, &bytes)?;
    }

    for path in &report.pruned {
        fsx::remove_file(&guard, path)?;
        ledger.forget(path)?;
    }
    Ok(())
}

fn index_for(env: &Env) -> Result<Index> {
    Index::open(&env.index_path())
}

/// Re-reads the entry at execution time so what is written is what is on disk
/// now, not what was on disk when the plan was made.
fn source_entry(env: &Env, host_id: &str) -> Option<serde_json::Value> {
    for root in &env.user_data {
        let inventory = desktop::inventory(root);
        if let Some(entry) = inventory.entries.iter().find(|e| e.host_id == host_id) {
            return Some(entry.projected(false));
        }
    }
    None
}

fn short(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

/// Present so the scope type stays exercised from this module's tests.
#[allow(dead_code)]
fn _scope(account: &str, org: &str) -> Scope {
    Scope { account: account.to_owned(), org: org.to_owned(), surface: Surface::Code }
}
