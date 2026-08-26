//! Reading the desktop app's session index, as observed in Claude Desktop
//! 1.34493.1.
//!
//! ```text
//! <userData>/claude-code-sessions/<accountUuid>/<orgUuid>/local_<hostId>.json
//! <userData>/claude-code-sessions/<accountUuid>/<orgUuid>/deleted_<uuid>
//! <userData>/local-agent-mode-sessions/<accountUuid>/<orgUuid>/...
//! ```
//!
//! The app builds that path from the account and organization it is signed in as
//! and reads only that directory, which is why switching accounts empties the
//! list while every transcript stays on disk. `cliSessionId` is the bridge back
//! to `projects/<slug>/<id>.jsonl`.

use crate::error::{Error, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CODE_SESSIONS: &str = "claude-code-sessions";
pub const AGENT_SESSIONS: &str = "local-agent-mode-sessions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Surface {
    Code,
    Cowork,
}

impl Surface {
    #[must_use]
    pub fn dir_name(self) -> &'static str {
        match self {
            Surface::Code => CODE_SESSIONS,
            Surface::Cowork => AGENT_SESSIONS,
        }
    }
}

/// Where a session index entry lives. Two accounts can hold an entry for the same
/// session, which is the whole point of projecting one into the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Scope {
    pub account: String,
    pub org: String,
    pub surface: Surface,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: Utf8PathBuf,
    pub scope: Scope,
    /// `local_<uuid>`, the desktop's own id.
    pub host_id: String,
    /// The transcript this entry points at, when it has one.
    pub cli_session_id: Option<String>,
    /// Earlier transcripts this session has been through. Claude tracks these as
    /// `unarchivedCliSessionId` and `priorCliSessionIds`, and a tombstone can
    /// name any link in that chain rather than the current one.
    pub prior_session_ids: Vec<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub created_at_ms: Option<i64>,
    pub last_activity_ms: Option<i64>,
    pub is_archived: bool,
    pub size: u64,
    /// Bytes taken by the account scoped MCP payload, which projection drops.
    pub account_scoped_bytes: u64,
    raw: Value,
}

impl Entry {
    /// The entry as it should be written into another account's directory.
    ///
    /// `remoteMcpServersConfig` and `enabledMcpTools` are roughly four fifths of
    /// the file and describe servers and tool grants belonging to the account it
    /// came from. Dropping them keeps one account's MCP entitlements from
    /// appearing under another; the desktop lists and opens the session either
    /// way, and re-resolves them on resume.
    #[must_use]
    pub fn projected(&self, keep_account_scoped: bool) -> Value {
        let mut v = self.raw.clone();
        if !keep_account_scoped {
            if let Some(o) = v.as_object_mut() {
                o.remove("remoteMcpServersConfig");
                o.remove("enabledMcpTools");
            }
        }
        v
    }

    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.json", self.host_id)
    }

    /// Whether this entry is, or ever was, the given transcript.
    #[must_use]
    pub fn names_session(&self, session_id: &str) -> bool {
        self.cli_session_id.as_deref() == Some(session_id)
            || self.prior_session_ids.iter().any(|id| id == session_id)
    }
}

/// A session the user deleted from one account's list. The transcript survives:
/// deleting in the desktop removes the index entry only.
///
/// The id is a `cliSessionId`, never a host id. Claude's own recovery scan
/// collects these from `deleted_*` and compares them against the `cliSessionId`
/// of each candidate transcript, and it collects them only from the directory of
/// the account currently signed in, which is where the per-directory scope comes
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub scope: Scope,
    pub id: String,
    pub deleted_at_ms: Option<i64>,
}

#[derive(Debug, Default, Clone)]
pub struct Inventory {
    pub entries: Vec<Entry>,
    pub tombstones: Vec<Tombstone>,
    /// Every account seen, mapped to the organizations found under it.
    pub scopes: BTreeMap<String, Vec<String>>,
    pub unreadable: Vec<(Utf8PathBuf, String)>,
}

impl Inventory {
    #[must_use]
    pub fn entries_in(&self, account: &str, org: &str) -> Vec<&Entry> {
        self.entries.iter().filter(|e| e.scope.account == account && e.scope.org == org).collect()
    }

    /// Entries that exist somewhere but not under the given account and org.
    #[must_use]
    pub fn missing_from(&self, account: &str, org: &str) -> Vec<&Entry> {
        let present: std::collections::BTreeSet<&str> = self
            .entries
            .iter()
            .filter(|e| e.scope.account == account && e.scope.org == org)
            .map(|e| e.host_id.as_str())
            .collect();
        self.entries
            .iter()
            .filter(|e| !(e.scope.account == account && e.scope.org == org))
            .filter(|e| !present.contains(e.host_id.as_str()))
            .collect()
    }

    /// Whether the user deleted this session from that account's list.
    ///
    /// Matched against the whole chain of transcript ids the session has been
    /// through, since a tombstone written before an archive round trip names the
    /// id that was current then.
    #[must_use]
    pub fn is_tombstoned(&self, account: &str, org: &str, entry: &Entry) -> bool {
        self.tombstones
            .iter()
            .any(|t| t.scope.account == account && t.scope.org == org && entry.names_session(&t.id))
    }

    fn merge(&mut self, other: Inventory) {
        self.entries.extend(other.entries);
        self.tombstones.extend(other.tombstones);
        self.unreadable.extend(other.unreadable);
        for (account, orgs) in other.scopes {
            let slot = self.scopes.entry(account).or_default();
            for org in orgs {
                if !slot.contains(&org) {
                    slot.push(org);
                }
            }
        }
    }
}

pub fn inventory(user_data: &Utf8Path) -> Inventory {
    let mut inv = Inventory::default();
    for surface in [Surface::Code, Surface::Cowork] {
        inv.merge(scan_surface(user_data, surface));
    }
    inv
}

fn scan_surface(user_data: &Utf8Path, surface: Surface) -> Inventory {
    let mut inv = Inventory::default();
    let root = user_data.join(surface.dir_name());
    let Ok(accounts) = std::fs::read_dir(&root) else { return inv };

    for acct in accounts.flatten() {
        if !acct.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let account = acct.file_name().to_string_lossy().into_owned();
        // Sentinel directories live alongside the account ones. `skills-plugin`
        // is real, sits at this level, and nests differently underneath.
        if !crate::claude::is_uuid(&account) {
            continue;
        }
        let Ok(orgs) = std::fs::read_dir(acct.path()) else { continue };

        for org_entry in orgs.flatten() {
            if !org_entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let org = org_entry.file_name().to_string_lossy().into_owned();
            if !crate::claude::is_uuid(&org) {
                continue;
            }
            let slot = inv.scopes.entry(account.clone()).or_default();
            if !slot.contains(&org) {
                slot.push(org.clone());
            }
            let scope = Scope { account: account.clone(), org: org.clone(), surface };
            let Ok(files) = std::fs::read_dir(org_entry.path()) else { continue };

            for fe in files.flatten() {
                let Ok(path) = Utf8PathBuf::from_path_buf(fe.path()) else { continue };
                let Some(name) = path.file_name() else { continue };

                if let Some(id) = name.strip_prefix("deleted_") {
                    inv.tombstones.push(Tombstone {
                        scope: scope.clone(),
                        id: id.to_owned(),
                        deleted_at_ms: std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|s| s.trim().parse().ok()),
                    });
                } else if name.starts_with("local_") && path.extension() == Some("json") {
                    match read_entry(&path, &scope) {
                        Ok(Some(e)) => inv.entries.push(e),
                        Ok(None) => {}
                        Err(e) => inv.unreadable.push((path, e.to_string())),
                    }
                }
            }
        }
    }
    inv
}

fn read_entry(path: &Utf8Path, scope: &Scope) -> Result<Option<Entry>> {
    let raw = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let value: Value = serde_json::from_slice(&raw).map_err(|e| Error::json(path, e))?;
    let Some(host_id) = value.get("sessionId").and_then(Value::as_str) else { return Ok(None) };

    let account_scoped_bytes = ["remoteMcpServersConfig", "enabledMcpTools"]
        .iter()
        .filter_map(|k| value.get(*k))
        .filter_map(|v| serde_json::to_vec(v).ok())
        .map(|b| b.len() as u64)
        .sum();

    let text = |k: &str| value.get(k).and_then(Value::as_str).map(ToOwned::to_owned);

    let mut prior_session_ids: Vec<String> = Vec::new();
    if let Some(id) = text("unarchivedCliSessionId") {
        prior_session_ids.push(id);
    }
    if let Some(list) = value.get("priorCliSessionIds").and_then(Value::as_array) {
        prior_session_ids.extend(list.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)));
    }
    prior_session_ids.sort();
    prior_session_ids.dedup();

    Ok(Some(Entry {
        path: path.to_owned(),
        scope: scope.clone(),
        host_id: host_id.to_owned(),
        cli_session_id: text("cliSessionId"),
        prior_session_ids,
        title: text("title").filter(|t| !t.trim().is_empty()),
        cwd: text("cwd"),
        model: text("model"),
        created_at_ms: value.get("createdAt").and_then(Value::as_i64),
        last_activity_ms: value.get("lastActivityAt").and_then(Value::as_i64),
        is_archived: value.get("isArchived").and_then(Value::as_bool).unwrap_or(false),
        size: raw.len() as u64,
        account_scoped_bytes,
        raw: value,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn entry(host: &str, cli: Option<&str>) -> Entry {
        Entry {
            path: "/x".into(),
            scope: Scope { account: "a".into(), org: "o".into(), surface: Surface::Code },
            host_id: host.to_owned(),
            cli_session_id: cli.map(ToOwned::to_owned),
            prior_session_ids: Vec::new(),
            title: None,
            cwd: None,
            model: None,
            created_at_ms: None,
            last_activity_ms: None,
            is_archived: false,
            size: 0,
            account_scoped_bytes: 0,
            raw: serde_json::json!({
                "sessionId": host,
                "title": "t",
                "remoteMcpServersConfig": [{"uuid": "m"}],
                "enabledMcpTools": {"x": true},
            }),
        }
    }

    #[test]
    fn projection_drops_the_account_scoped_payload_by_default() {
        let projected = entry("local_x", None).projected(false);
        assert!(projected.get("remoteMcpServersConfig").is_none());
        assert!(projected.get("enabledMcpTools").is_none());
        assert_eq!(projected.get("title").and_then(Value::as_str), Some("t"));
        assert_eq!(projected.get("sessionId").and_then(Value::as_str), Some("local_x"));
    }

    #[test]
    fn projection_can_keep_it_when_asked() {
        let projected = entry("local_x", None).projected(true);
        assert!(projected.get("remoteMcpServersConfig").is_some());
    }

    #[test]
    fn a_tombstone_is_keyed_by_transcript_id_not_by_host_id() {
        let mut inv = Inventory::default();
        let scope = Scope { account: "a".into(), org: "o".into(), surface: Surface::Code };
        inv.tombstones.push(Tombstone {
            scope: scope.clone(),
            id: "cli-1".into(),
            deleted_at_ms: None,
        });
        inv.tombstones.push(Tombstone { scope, id: "host-2".into(), deleted_at_ms: None });

        assert!(inv.is_tombstoned("a", "o", &entry("local_zzz", Some("cli-1"))));
        // A host id that happens to match a tombstone is not a deletion.
        assert!(!inv.is_tombstoned("a", "o", &entry("local_host-2", Some("cli-9"))));
        assert!(!inv.is_tombstoned("a", "o", &entry("local_other", Some("cli-9"))));
        assert!(
            !inv.is_tombstoned("b", "o", &entry("local_zzz", Some("cli-1"))),
            "scoped per directory, exactly as Claude scopes it to the signed in account"
        );
    }

    #[test]
    fn a_tombstone_naming_an_earlier_transcript_still_counts() {
        let mut inv = Inventory::default();
        inv.tombstones.push(Tombstone {
            scope: Scope { account: "a".into(), org: "o".into(), surface: Surface::Code },
            id: "cli-old".into(),
            deleted_at_ms: None,
        });
        let mut moved = entry("local_x", Some("cli-new"));
        moved.prior_session_ids = vec!["cli-old".to_owned()];

        assert!(inv.is_tombstoned("a", "o", &moved), "the session was deleted under its old id");
    }

    #[test]
    fn missing_from_ignores_entries_the_target_already_has() {
        let mut inv = Inventory::default();
        let here = Scope { account: "a".into(), org: "o".into(), surface: Surface::Code };
        let there = Scope { account: "b".into(), org: "p".into(), surface: Surface::Code };

        inv.entries.push(Entry { scope: here.clone(), ..entry("local_1", None) });
        inv.entries.push(Entry { scope: there.clone(), ..entry("local_1", None) });
        inv.entries.push(Entry { scope: there, ..entry("local_2", None) });

        let missing = inv.missing_from("a", "o");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].host_id, "local_2");
        assert_eq!(inv.entries_in("a", "o").len(), 1);
    }
}
