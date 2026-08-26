//! Read-only diagnosis. Answers the original question before anything is written:
//! how many conversations exist, how many are invisible under the account that is
//! signed in, and whether it is safe to do anything about it.

use crate::claude::desktop::{self, Inventory};
use crate::claude::identity::{self, Active, Identities};
use crate::claude::layout::{Compat, Fingerprint, Version};
use crate::claude::transcript::{self, Hidden};
use crate::env::Env;
use crate::error::Result;
use camino::Utf8PathBuf;
use serde::Serialize;
use std::collections::BTreeMap;

const DEFAULT_CLEANUP_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct ConfigDirReport {
    pub path: Utf8PathBuf,
    pub conversations: usize,
    pub hidden: BTreeMap<String, usize>,
    pub subagents: usize,
    pub skipped: usize,
    pub project_dirs: usize,
    pub bytes: u64,
    pub unreadable: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrgReport {
    pub uuid: String,
    pub name: Option<String>,
    pub entries: usize,
    pub tombstones: usize,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountReport {
    pub uuid: String,
    pub email: Option<String>,
    pub is_active: bool,
    pub orgs: Vec<OrgReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreReport {
    pub path: Utf8PathBuf,
    pub exists: bool,
    /// `None` when it could not be determined, which is not the same as `false`.
    pub hardlinks_viable: Option<bool>,
    pub transcripts: usize,
    pub ledger_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionReport {
    pub cleanup_period_days: i64,
    pub from_settings: bool,
    pub at_risk: usize,
    pub at_risk_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warn,
    Blocker,
}

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: u32,
    pub unsilo_version: &'static str,
    pub home: Utf8PathBuf,
    pub config_dirs: Vec<ConfigDirReport>,
    pub user_data: Vec<Utf8PathBuf>,
    pub cli_versions: Vec<(String, usize)>,
    pub remote_backend: Option<bool>,
    pub writes_allowed: bool,
    pub compat_reason: Option<String>,
    pub accounts: Vec<AccountReport>,
    pub active: Option<Active>,
    /// Desktop entries that exist somewhere but not under the active account.
    /// This is the number the whole tool exists for.
    pub invisible_under_active: usize,
    pub unresolved_accounts: Vec<String>,
    pub linked_entries: usize,
    pub total_entries: usize,
    pub tail_unresolved: usize,
    /// Session uuids found in more than one project directory, with their dirs.
    pub duplicate_locations: Vec<(String, Vec<String>)>,
    pub store: StoreReport,
    pub retention: RetentionReport,
    pub problems: Vec<Problem>,
}

impl Report {
    #[must_use]
    pub fn conversations(&self) -> usize {
        self.config_dirs.iter().map(|c| c.conversations).sum()
    }

    #[must_use]
    pub fn subagents(&self) -> usize {
        self.config_dirs.iter().map(|c| c.subagents).sum()
    }

    #[must_use]
    pub fn has_blockers(&self) -> bool {
        self.problems.iter().any(|p| p.severity == Severity::Blocker)
    }

    #[must_use]
    pub fn has_warnings(&self) -> bool {
        self.problems.iter().any(|p| p.severity != Severity::Info)
    }
}

#[derive(Debug, Default)]
struct Transcripts {
    dirs: Vec<ConfigDirReport>,
    versions: BTreeMap<String, usize>,
    ids: std::collections::BTreeSet<String>,
    locations: BTreeMap<String, Vec<String>>,
    tail_unresolved: usize,
    at_risk: usize,
    at_risk_bytes: u64,
    problems: Vec<Problem>,
}

fn scan_transcripts(env: &Env, cleanup_days: i64) -> Transcripts {
    let mut out = Transcripts::default();
    let now = env.clock.now_ms();

    for dir in &env.config_dirs {
        let scan = transcript::scan(dir);
        let mut hidden: BTreeMap<String, usize> = BTreeMap::new();
        let mut conversations = 0;
        let mut bytes = 0;

        for meta in &scan.sessions {
            bytes += meta.size;
            out.ids.insert(meta.session_id.clone());
            out.locations
                .entry(meta.session_id.clone())
                .or_default()
                .push(meta.origin_dir.to_string());
            if meta.tail_unresolved {
                out.tail_unresolved += 1;
            }
            if let Some(version) = &meta.cli_version {
                *out.versions.entry(version.clone()).or_insert(0) += 1;
            }
            match meta.hidden_from_resume() {
                Some(reason) => *hidden.entry(reason.as_str().to_owned()).or_insert(0) += 1,
                None => conversations += 1,
            }
            if let Some(modified) = meta.modified_at_ms {
                if crate::claude::time::days_between(modified, now) > cleanup_days {
                    out.at_risk += 1;
                    out.at_risk_bytes += meta.size;
                }
            }
        }

        for (path, err) in &scan.unreadable {
            out.problems.push(Problem {
                severity: Severity::Warn,
                message: format!("could not read {path}: {err}"),
            });
        }

        out.dirs.push(ConfigDirReport {
            path: dir.clone(),
            conversations,
            hidden,
            subagents: scan.subagents,
            skipped: scan.skipped,
            project_dirs: scan.project_dirs,
            bytes,
            unreadable: scan.unreadable.iter().map(|(p, _)| p.to_string()).collect(),
        });
    }
    out
}

fn collect_inventory(env: &Env, problems: &mut Vec<Problem>) -> Inventory {
    let mut inventory = Inventory::default();
    for root in &env.user_data {
        let inv = desktop::inventory(root);
        for (path, err) in &inv.unreadable {
            problems.push(Problem {
                severity: Severity::Warn,
                message: format!("unreadable desktop entry {path}: {err}"),
            });
        }
        inventory.entries.extend(inv.entries);
        inventory.tombstones.extend(inv.tombstones);
        for (account, orgs) in inv.scopes {
            inventory.scopes.entry(account).or_default().extend(orgs);
        }
    }
    inventory
}

pub fn run(env: &Env) -> Result<Report> {
    let cleanup = cleanup_period(env);
    let scanned = scan_transcripts(env, cleanup.0);
    let mut problems = scanned.problems.clone();
    let inventory = collect_inventory(env, &mut problems);

    let fingerprint = Fingerprint {
        remote_backend: identity::remote_backend_enabled(&env.home, None),
        newest_cli: scanned.versions.keys().filter_map(|v| Version::parse(v)).max(),
        cli_versions: scanned.versions.clone(),
        has_transcripts: scanned.dirs.iter().any(|c| c.conversations > 0),
        has_desktop_index: !inventory.entries.is_empty(),
        surfaces: Vec::new(),
    };

    let active = identity::active(&env.home);
    let mut identities = Identities::load(&env.unsilo_home.join("identities.json"))?;
    identities.learn_from(&env.home);

    let accounts = build_accounts(&inventory, &identities, active.as_ref());
    let invisible_under_active =
        active.as_ref().map_or(0, |a| inventory.missing_from(&a.account, &a.org).len());

    let duplicate_locations = duplicate_locations(&scanned, &mut problems);

    let linked_entries = inventory
        .entries
        .iter()
        .filter(|e| e.cli_session_id.as_ref().is_some_and(|id| scanned.ids.contains(id)))
        .count();

    let compat = fingerprint.compat();
    push_environment_problems(env, &compat, &mut problems);
    match &compat {
        Compat::Refuses(reason) => problems
            .push(Problem { severity: Severity::Blocker, message: format!("read only: {reason}") }),
        Compat::Unverified(reason) => {
            problems.push(Problem { severity: Severity::Warn, message: reason.clone() });
        }
        Compat::Known => {}
    }

    let unresolved = identities.unresolved(inventory.scopes.keys().map(String::as_str));
    if !unresolved.is_empty() {
        problems.push(Problem {
            severity: Severity::Info,
            message: format!(
                "{} account(s) without an email: only the account signed in right now can be \
                 resolved locally, label the others by hand in identities.json",
                unresolved.len()
            ),
        });
    }

    let store = store_report(env);
    if store.hardlinks_viable == Some(false) {
        problems.push(Problem {
            severity: Severity::Warn,
            message: "the store is on another volume: copies will be used, and they fall \
                      behind as a session grows"
                .to_owned(),
        });
    }

    Ok(Report {
        schema: 1,
        unsilo_version: crate::VERSION,
        home: env.home.clone(),
        config_dirs: scanned.dirs,
        user_data: env.user_data.clone(),
        cli_versions: fingerprint
            .versions_by_frequency()
            .into_iter()
            .map(|(v, n)| (v.to_owned(), n))
            .collect(),
        remote_backend: fingerprint.remote_backend,
        writes_allowed: compat.allows_writes(),
        compat_reason: compat.reason().map(ToOwned::to_owned),
        accounts,
        active,
        invisible_under_active,
        unresolved_accounts: unresolved,
        linked_entries,
        total_entries: inventory.entries.len(),
        tail_unresolved: scanned.tail_unresolved,
        duplicate_locations,
        store,
        retention: RetentionReport {
            cleanup_period_days: cleanup.0,
            from_settings: cleanup.1,
            at_risk: scanned.at_risk,
            at_risk_bytes: scanned.at_risk_bytes,
        },
        problems,
    })
}

fn push_environment_problems(env: &Env, _compat: &Compat, problems: &mut Vec<Problem>) {
    if env.config_dirs.is_empty() {
        problems.push(Problem {
            severity: Severity::Blocker,
            message: "no Claude config dir with a projects/ directory was found".to_owned(),
        });
    }
    if env.user_data.is_empty() {
        problems.push(Problem {
            severity: Severity::Warn,
            message: "no desktop userData found; only the cli side is visible".to_owned(),
        });
    }
}

fn duplicate_locations(
    scanned: &Transcripts,
    problems: &mut Vec<Problem>,
) -> Vec<(String, Vec<String>)> {
    let duplicates: Vec<(String, Vec<String>)> = scanned
        .locations
        .iter()
        .filter(|(_, dirs)| dirs.len() > 1)
        .map(|(id, dirs)| {
            let mut dirs = dirs.clone();
            dirs.sort();
            (id.clone(), dirs)
        })
        .collect();
    if !duplicates.is_empty() {
        problems.push(Problem {
            severity: Severity::Info,
            message: format!(
                "{} session(s) exist in more than one project dir; the largest copy is used, \
                 which in an append-only transcript is the most complete history",
                duplicates.len()
            ),
        });
    }
    duplicates
}

fn build_accounts(
    inventory: &Inventory,
    identities: &Identities,
    active: Option<&Active>,
) -> Vec<AccountReport> {
    inventory
        .scopes
        .iter()
        .map(|(account, orgs)| {
            let mut seen: Vec<&String> = orgs.iter().collect();
            seen.sort();
            seen.dedup();
            AccountReport {
                uuid: account.clone(),
                email: identities.email(account).map(ToOwned::to_owned),
                is_active: active.is_some_and(|a| &a.account == account),
                orgs: seen
                    .into_iter()
                    .map(|org| OrgReport {
                        uuid: org.clone(),
                        name: identities.org_name(org).map(ToOwned::to_owned),
                        entries: inventory.entries_in(account, org).len(),
                        tombstones: inventory
                            .tombstones
                            .iter()
                            .filter(|t| &t.scope.account == account && &t.scope.org == org)
                            .count(),
                        is_active: active.is_some_and(|a| &a.account == account && &a.org == org),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// `(days, came_from_settings)`. Claude defaults to 30 when unset.
fn cleanup_period(env: &Env) -> (i64, bool) {
    for dir in &env.config_dirs {
        let Ok(bytes) = std::fs::read(dir.join("settings.json")) else { continue };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
        if let Some(days) = value.get("cleanupPeriodDays").and_then(serde_json::Value::as_i64) {
            return (days, true);
        }
    }
    (DEFAULT_CLEANUP_DAYS, false)
}

fn store_report(env: &Env) -> StoreReport {
    // The index may not exist yet on a first run, and doctor never creates one.
    let ledger_entries = if env.index_path().exists() {
        crate::index::Index::open(&env.index_path())
            .and_then(|index| Ok(crate::ledger::Ledger::new(&index).entries()?.len()))
            .unwrap_or(0)
    } else {
        0
    };
    let transcripts = std::fs::read_dir(env.store_dir().join("transcripts")).map_or(0, |rd| {
        rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")).count()
    });
    StoreReport {
        path: env.unsilo_home.clone(),
        exists: env.unsilo_home.is_dir(),
        hardlinks_viable: hardlinks_viable(env),
        transcripts,
        ledger_entries,
    }
}

/// Hard links cannot cross filesystems, so the store has to live on the same one
/// as the transcripts. Checked against the nearest existing ancestor, since the
/// store directory may not have been created yet.
fn hardlinks_viable(env: &Env) -> Option<bool> {
    let config = env.config_dirs.first()?;
    let mut candidate = env.unsilo_home.clone();
    while !candidate.exists() {
        candidate = candidate.parent()?.to_owned();
    }
    same_volume(config, &candidate)
}

/// Hard links cannot cross filesystems. Derived from file identity so there is
/// one platform-specific implementation rather than one per question asked.
fn same_volume(a: &camino::Utf8Path, b: &camino::Utf8Path) -> Option<bool> {
    Some(crate::fsx::file_id(a)?.0 == crate::fsx::file_id(b)?.0)
}

/// Present so the enum is exercised even where a variant is only produced by a
/// platform-specific path.
#[allow(dead_code)]
fn _hidden_variants() -> [Hidden; 3] {
    [Hidden::Sidechain, Hidden::Team, Hidden::Daemon]
}
