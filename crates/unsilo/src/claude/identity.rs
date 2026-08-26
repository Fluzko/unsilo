//! Putting readable names on account and organization uuids.
//!
//! `~/.claude.json` carries `oauthAccount` for the account that is signed in
//! **right now** and nothing about the others. Its rotating backups under
//! `.claude/backups/` still hold whoever was signed in before, which recovers
//! some labels for free.
//!
//! That recovery is best effort and was measured failing: on a real machine the
//! five surviving backups had all rotated past an account switch within half an
//! hour, so an email known minutes earlier was already gone. Labels are
//! therefore persisted the first time they are seen, never re-derived, and can
//! be set by hand for accounts that were never active while Unsilo ran.

use crate::error::{Error, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const BACKUP_PREFIX: &str = ".claude.json.backup.";
/// Enough to catch a recent switch without walking an unbounded directory.
const MAX_BACKUPS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Read out of Claude's own config while that account was active.
    Learned,
    /// Written by the user. Never overwritten by learning.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Active {
    pub account: String,
    pub org: String,
    pub email: Option<String>,
    pub org_name: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Identities {
    /// account uuid -> email
    pub accounts: BTreeMap<String, Label>,
    /// org uuid -> display name
    pub orgs: BTreeMap<String, Label>,
}

impl Identities {
    pub fn load(path: &Utf8Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| Error::json(path, e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::io(path, e)),
        }
    }

    pub fn save(&self, path: &Utf8Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| Error::json(path, e))?;
        // Same directory, then rename: a partial write must never replace the
        // labels, since some of them cannot be recovered a second time.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).map_err(|e| Error::io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))
    }

    #[must_use]
    pub fn email(&self, account: &str) -> Option<&str> {
        self.accounts.get(account).map(|l| l.name.as_str())
    }

    #[must_use]
    pub fn org_name(&self, org: &str) -> Option<&str> {
        self.orgs.get(org).map(|l| l.name.as_str())
    }

    /// Accounts seen in the desktop index that still have no label. `doctor`
    /// lists these so the user knows which ones to name by hand.
    #[must_use]
    pub fn unresolved<'a>(&self, seen: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut out: Vec<String> = seen
            .into_iter()
            .filter(|a| !self.accounts.contains_key(*a))
            .map(ToOwned::to_owned)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    pub fn set_manual_account(&mut self, account: &str, email: &str) {
        self.accounts
            .insert(account.to_owned(), Label { name: email.to_owned(), source: Source::Manual });
    }

    fn learn_account(&mut self, account: &str, email: &str) {
        if email.is_empty() {
            return;
        }
        // Manual labels win, and an already learned one is not re-litigated:
        // the first sighting is the one we could still verify.
        self.accounts
            .entry(account.to_owned())
            .or_insert_with(|| Label { name: email.to_owned(), source: Source::Learned });
    }

    fn learn_org(&mut self, org: &str, name: &str) {
        if name.is_empty() {
            return;
        }
        self.orgs
            .entry(org.to_owned())
            .or_insert_with(|| Label { name: name.to_owned(), source: Source::Learned });
    }

    /// Folds in whatever the current config and its backups can still tell us.
    /// Returns how many new labels were added, so `apply` can report having
    /// captured something that would otherwise have rotated away.
    pub fn learn_from(&mut self, home: &Utf8Path) -> usize {
        let before = self.accounts.len() + self.orgs.len();
        for path in config_candidates(home) {
            let Some(oauth) = read_oauth(&path) else { continue };
            if let (Some(account), Some(email)) = (oauth.account.as_deref(), oauth.email.as_deref())
            {
                self.learn_account(account, email);
            }
            if let (Some(org), Some(name)) = (oauth.org.as_deref(), oauth.org_name.as_deref()) {
                self.learn_org(org, name);
            }
        }
        self.accounts.len() + self.orgs.len() - before
    }
}

#[derive(Debug, Default)]
struct OauthAccount {
    account: Option<String>,
    org: Option<String>,
    email: Option<String>,
    org_name: Option<String>,
}

/// The live config first, then backups newest to oldest. The live file is
/// rewritten constantly and can be caught mid write, so a parse failure falls
/// through instead of aborting.
fn config_candidates(home: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut out = vec![home.join(".claude.json")];
    if let Ok(rd) = std::fs::read_dir(home.join(".claude").join("backups")) {
        let mut backups: Vec<Utf8PathBuf> = rd
            .flatten()
            .filter_map(|e| Utf8PathBuf::from_path_buf(e.path()).ok())
            .filter(|p| p.file_name().is_some_and(|n| n.starts_with(BACKUP_PREFIX)))
            .collect();
        // Names end in a millisecond timestamp, so lexical order is chronological.
        backups.sort();
        backups.reverse();
        backups.truncate(MAX_BACKUPS);
        out.extend(backups);
    }
    out
}

fn read_oauth(path: &Utf8Path) -> Option<OauthAccount> {
    let bytes = std::fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let oauth = value.get("oauthAccount")?;
    let text = |k: &str| oauth.get(k).and_then(Value::as_str).map(ToOwned::to_owned);
    let out = OauthAccount {
        account: text("accountUuid"),
        org: text("organizationUuid"),
        email: text("emailAddress"),
        org_name: text("organizationName"),
    };
    out.account.as_ref()?;
    Some(out)
}

/// The account and organization the desktop is currently reading its session
/// list from, which is the directory `apply` projects into.
#[must_use]
pub fn active(home: &Utf8Path) -> Option<Active> {
    config_candidates(home).into_iter().find_map(|path| {
        let oauth = read_oauth(&path)?;
        Some(Active {
            account: oauth.account?,
            org: oauth.org?,
            email: oauth.email.filter(|s| !s.is_empty()),
            org_name: oauth.org_name.filter(|s| !s.is_empty()),
        })
    })
}

/// Whether Claude is storing transcripts through the remote backend instead of
/// as plain files. If it ever is, hard links have nothing to point at and the
/// writing commands must refuse rather than corrupt something.
#[must_use]
pub fn remote_backend_enabled(home: &Utf8Path, env_value: Option<&str>) -> Option<bool> {
    if let Some(v) = env_value {
        let v = v.to_ascii_lowercase();
        if matches!(v.as_str(), "1" | "true" | "pinned" | "on") {
            return Some(true);
        }
        if matches!(v.as_str(), "0" | "false" | "off") {
            return Some(false);
        }
    }
    let bytes = std::fs::read(home.join(".claude.json")).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    match value.get("cachedGrowthBookFeatures")? {
        Value::Object(map) => map.get("tengu_hover_rest")?.as_bool(),
        Value::Array(items) => items.iter().find_map(|item| {
            (item.get("key")?.as_str()? == "tengu_hover_rest")
                .then(|| item.get("value")?.as_bool())?
        }),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_manual_label_is_never_overwritten_by_learning() {
        let mut ids = Identities::default();
        ids.set_manual_account("acct", "chosen@example.com");
        ids.learn_account("acct", "learned@example.com");
        assert_eq!(ids.email("acct"), Some("chosen@example.com"));
        assert_eq!(ids.accounts["acct"].source, Source::Manual);
    }

    #[test]
    fn the_first_sighting_wins_over_later_ones() {
        let mut ids = Identities::default();
        ids.learn_account("acct", "first@example.com");
        ids.learn_account("acct", "second@example.com");
        assert_eq!(ids.email("acct"), Some("first@example.com"));
    }

    #[test]
    fn empty_labels_are_not_recorded() {
        let mut ids = Identities::default();
        ids.learn_account("acct", "");
        ids.learn_org("org", "");
        assert!(ids.accounts.is_empty());
        assert!(ids.orgs.is_empty());
    }

    #[test]
    fn unresolved_names_the_accounts_that_still_need_one() {
        let mut ids = Identities::default();
        ids.learn_account("known", "k@example.com");
        assert_eq!(ids.unresolved(["known", "other", "other"]), vec!["other".to_owned()]);
    }

    #[test]
    fn the_remote_backend_env_override_beats_the_cached_flag() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            br#"{"cachedGrowthBookFeatures":{"tengu_hover_rest":false}}"#,
        )
        .unwrap();
        assert_eq!(remote_backend_enabled(&home, None), Some(false));
        assert_eq!(remote_backend_enabled(&home, Some("pinned")), Some(true));
        assert_eq!(remote_backend_enabled(&home, Some("garbage")), Some(false));
    }

    #[test]
    fn the_flag_is_read_from_either_cache_shape() {
        let dir = tempfile::tempdir().unwrap();
        let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            home.join(".claude.json"),
            br#"{"cachedGrowthBookFeatures":[{"key":"tengu_hover_rest","value":true}]}"#,
        )
        .unwrap();
        assert_eq!(remote_backend_enabled(&home, None), Some(true));
    }

    #[test]
    fn identities_round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = Utf8PathBuf::from_path_buf(dir.path().join("identities.json")).unwrap();
        assert_eq!(Identities::load(&path).unwrap(), Identities::default());

        let mut ids = Identities::default();
        ids.set_manual_account("a", "a@example.com");
        ids.learn_org("o", "Acme");
        ids.save(&path).unwrap();

        assert_eq!(Identities::load(&path).unwrap(), ids);
    }
}
