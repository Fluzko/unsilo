//! Building a desktop index entry from a CLI transcript.
//!
//! The desktop lists a conversation only if it has an entry, and it writes one
//! only for the sessions it created itself. A conversation started from the
//! terminal therefore never appears there, whatever account is signed in. On a
//! real machine that is 126 of 132 conversations.
//!
//! Everything an entry needs is already in the transcript, so the entry can be
//! built rather than waited for. This is more invasive than copying one that
//! already exists, which is why it takes an explicit flag: it tells the desktop
//! about a conversation the desktop never knew.

use crate::claude::transcript::Meta;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Marks the entry as ours and as built rather than copied. Informational: the
/// mechanism that recognises these is the ledger, not this field, so the desktop
/// ignoring an unknown key costs nothing.
pub const ORIGIN_FIELD: &str = "unsiloOrigin";
pub const ORIGIN_CLI: &str = "cli";

/// A host id derived from the transcript id, so adopting the same conversation
/// twice produces the same entry instead of a second one.
#[must_use]
pub fn host_id_for(session_id: &str) -> String {
    let digest = Sha256::digest(format!("unsilo-adopt:{session_id}").as_bytes());
    let hex: String = digest.iter().take(16).fold(String::new(), |mut acc, byte| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{byte:02x}");
        acc
    });
    let part = |range: std::ops::Range<usize>| hex.get(range).unwrap_or_default();
    format!(
        "local_{}-{}-{}-{}-{}",
        part(0..8),
        part(8..12),
        part(12..16),
        part(16..20),
        part(20..32)
    )
}

/// The entry as the desktop would have written it, from what the transcript
/// already says.
///
/// Deliberately omits everything account-scoped. `remoteMcpServersConfig` and
/// `enabledMcpTools` belong to whichever account resolves them, and inventing
/// them here would attach one account's entitlements to a conversation that
/// never had any.
#[must_use]
pub fn entry_for(meta: &Meta, now_ms: i64) -> Value {
    let created = meta.created_at_ms.or(meta.modified_at_ms).unwrap_or(now_ms);
    let modified = meta.modified_at_ms.or(meta.created_at_ms).unwrap_or(now_ms);
    let cwd = meta.cwd.clone().unwrap_or_else(|| meta.origin_dir.to_string());
    let title = meta
        .display_title()
        .map_or_else(|| "(untitled)".to_owned(), |t| t.chars().take(120).collect::<String>());

    let mut entry = json!({
        "sessionId": host_id_for(&meta.session_id),
        "cliSessionId": meta.session_id,
        "cwd": cwd,
        "originCwd": cwd,
        "createdAt": created,
        "lastActivityAt": modified,
        "lastFocusedAt": modified,
        // Never claim the user wrote this title, since it may be the first prompt.
        "titleSource": "auto",
        "title": title,
        "isArchived": false,
        ORIGIN_FIELD: ORIGIN_CLI,
    });
    if let (Some(object), Some(branch)) = (entry.as_object_mut(), meta.git_branch.as_ref()) {
        object.insert("sourceBranch".to_owned(), json!(branch));
    }
    if let (Some(object), Some(model)) = (entry.as_object_mut(), meta.model.as_ref()) {
        object.insert("model".to_owned(), json!(model));
    }
    if let (Some(object), Some(effort)) = (entry.as_object_mut(), meta.effort.as_ref()) {
        object.insert("effort".to_owned(), json!(effort));
    }
    entry
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            session_id: "3db70634-c5f9-4099-95e4-710261df621f".to_owned(),
            record_id: None,
            path: "/h/.claude/projects/-h-p/3db70634.jsonl".into(),
            origin_dir: "/h/.claude/projects/-h-p".into(),
            size: 10,
            cwd: Some("/h/p".to_owned()),
            git_branch: Some("main".to_owned()),
            cli_version: Some("2.1.241".to_owned()),
            model: Some("claude-opus-5".to_owned()),
            effort: Some("xhigh".to_owned()),
            title: Some("Fix auth".to_owned()),
            first_prompt: Some("please fix auth".to_owned()),
            created_at_ms: Some(1_000),
            modified_at_ms: Some(2_000),
            is_sidechain: false,
            session_kind: None,
            team_name: None,
            entrypoint: None,
            tail_unresolved: false,
        }
    }

    #[test]
    fn the_host_id_is_derived_so_adopting_twice_is_the_same_entry() {
        let first = host_id_for("abc");
        assert_eq!(first, host_id_for("abc"));
        assert_ne!(first, host_id_for("abd"));
        assert!(first.starts_with("local_"));
        assert_eq!(first.len(), "local_".len() + 36);
    }

    #[test]
    fn the_entry_points_back_at_the_transcript() {
        let entry = entry_for(&meta(), 9_999);
        assert_eq!(entry["cliSessionId"], meta().session_id);
        assert_eq!(entry["sessionId"], host_id_for(&meta().session_id));
        assert_eq!(entry["cwd"], "/h/p");
        assert_eq!(entry["title"], "Fix auth");
        assert_eq!(entry["createdAt"], 1_000);
        assert_eq!(entry["lastActivityAt"], 2_000);
        assert_eq!(entry["sourceBranch"], "main");
        assert_eq!(entry["model"], "claude-opus-5");
        assert_eq!(entry[ORIGIN_FIELD], ORIGIN_CLI);
    }

    #[test]
    fn nothing_account_scoped_is_invented() {
        let entry = entry_for(&meta(), 0);
        assert!(entry.get("remoteMcpServersConfig").is_none());
        assert!(entry.get("enabledMcpTools").is_none());
    }

    #[test]
    fn a_transcript_with_no_reply_yet_still_produces_an_entry() {
        let bare = Meta { model: None, effort: None, git_branch: None, ..meta() };
        let entry = entry_for(&bare, 0);
        assert!(entry.get("model").is_none(), "absent rather than guessed");
        assert!(entry.get("effort").is_none());
        assert!(entry.get("sourceBranch").is_none());
        assert_eq!(entry["title"], "Fix auth");
    }

    #[test]
    fn a_missing_title_falls_back_and_never_to_a_uuid() {
        let untitled = Meta { title: None, first_prompt: None, ..meta() };
        assert_eq!(entry_for(&untitled, 0)["title"], "(untitled)");

        let prompted = Meta { title: None, ..meta() };
        assert_eq!(entry_for(&prompted, 0)["title"], "please fix auth");
    }

    #[test]
    fn missing_timestamps_fall_back_to_now_rather_than_to_zero() {
        let undated = Meta { created_at_ms: None, modified_at_ms: None, ..meta() };
        let entry = entry_for(&undated, 4_242);
        assert_eq!(entry["createdAt"], 4_242);
        assert_eq!(entry["lastActivityAt"], 4_242);
    }

    #[test]
    fn a_long_title_is_trimmed_so_the_entry_stays_small() {
        let long = Meta { title: Some("x".repeat(500)), ..meta() };
        assert_eq!(entry_for(&long, 0)["title"].as_str().unwrap().chars().count(), 120);
    }
}
