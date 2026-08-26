//! Reading `projects/<slug>/<sessionId>.jsonl`, as observed in Claude Code CLI
//! 2.1.241.
//!
//! Two concerns are deliberately separated. **Identity** (which file is which
//! session, what its cwd is, whether Claude will show it) is copied exactly: if
//! Unsilo disagrees it lists sessions that cannot be opened, or hides ones that
//! can. **Presentation** (titles, prompts) is free to be better, and is.
//!
//! Transcripts are append only and are being written while we read them, so the
//! file is opened once, its length taken from that handle, and only that prefix
//! is ever read. A half written final line is expected, not an error.

use crate::claude::time::iso_to_epoch_ms;
use crate::error::{Error, Result};
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const WINDOW: u64 = 64 * 1024;
const MAX_WINDOW: u64 = 4 * 1024 * 1024;
const PROMPT_CHARS: usize = 300;

/// Why Claude keeps a transcript out of the resume picker. Unsilo hides the same
/// ones by default, but names the reason instead of dropping them silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    Sidechain,
    Team,
    Daemon,
}

impl Hidden {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Hidden::Sidechain => "isSidechain",
            Hidden::Team => "teamName",
            Hidden::Daemon => "sessionKind",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// From the file name, which is what Claude itself trusts.
    pub session_id: String,
    /// From the records. Kept apart so a mismatch is detectable rather than hidden.
    pub record_id: Option<String>,
    pub path: Utf8PathBuf,
    /// The directory the transcript was found in. Recorded, never recomputed:
    /// past 200 UTF-16 units the slug carries a hash we cannot reproduce.
    pub origin_dir: Utf8PathBuf,
    pub size: u64,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub cli_version: Option<String>,
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub created_at_ms: Option<i64>,
    pub modified_at_ms: Option<i64>,
    pub is_sidechain: bool,
    pub session_kind: Option<String>,
    pub team_name: Option<String>,
    pub entrypoint: Option<String>,
    /// The tail window never resolved a whole line. Metadata from it is absent,
    /// not wrong, and `doctor` surfaces the count.
    pub tail_unresolved: bool,
}

impl Meta {
    #[must_use]
    pub fn hidden_from_resume(&self) -> Option<Hidden> {
        if self.is_sidechain {
            return Some(Hidden::Sidechain);
        }
        if self.team_name.is_some() {
            return Some(Hidden::Team);
        }
        match self.session_kind.as_deref() {
            Some("daemon" | "daemon-worker") => Some(Hidden::Daemon),
            _ => None,
        }
    }

    /// Title, then first prompt, then nothing. Never the raw file name.
    #[must_use]
    pub fn display_title(&self) -> Option<&str> {
        self.title.as_deref().or(self.first_prompt.as_deref())
    }

    #[must_use]
    pub fn short_id(&self) -> &str {
        self.session_id.get(..8).unwrap_or(&self.session_id)
    }
}

#[must_use]
pub fn is_session_uuid(s: &str) -> bool {
    crate::claude::is_uuid(s)
}

#[derive(Debug, Clone)]
struct Windows {
    head: String,
    tail: String,
    unresolved: bool,
}

fn read_windows(f: &mut File, size: u64) -> Result<Windows> {
    let head = read_at(f, 0, size.min(WINDOW))?;
    if size <= WINDOW {
        return Ok(Windows { tail: head.clone(), head, unresolved: false });
    }
    let mut window = WINDOW;
    loop {
        let start = size.saturating_sub(window);
        let raw = read_at(f, start, size - start)?;
        // The window opens mid line except by accident, so the first fragment goes.
        let trimmed = raw.find('\n').map_or("", |i| raw.get(i + 1..).unwrap_or("")).to_owned();
        let resolved = !trimmed.trim().is_empty();
        if resolved || window >= MAX_WINDOW {
            return Ok(Windows { head, tail: trimmed, unresolved: !resolved });
        }
        window = (window * 8).min(MAX_WINDOW);
    }
}

fn read_at(f: &mut File, offset: u64, len: u64) -> Result<String> {
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    let mut buf = vec![0u8; len];
    f.seek(SeekFrom::Start(offset))?;
    f.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn records(chunk: &str) -> Vec<Value> {
    chunk
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect()
}

fn str_field(records: &[Value], key: &str) -> Option<String> {
    records.iter().find_map(|r| r.get(key)?.as_str().map(ToOwned::to_owned))
}

fn typed<'a>(records: &'a [Value], ty: &str) -> impl DoubleEndedIterator<Item = &'a Value> {
    let ty = ty.to_owned();
    records.iter().filter(move |r| r.get("type").and_then(Value::as_str) == Some(ty.as_str()))
}

/// Slash commands arrive as an XML-ish blob. Claude keeps the markup; showing
/// `/clear` instead is strictly more useful and changes nothing about identity.
fn tidy_prompt(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t.starts_with("<local-command-stdout>") || t.starts_with("<system-reminder>")
    {
        return None;
    }
    if let Some(rest) = t.strip_prefix("<command-name>") {
        let name = rest.split("</command-name>").next().unwrap_or("").trim();
        if name.is_empty() {
            return None;
        }
        let args = t
            .split("<command-args>")
            .nth(1)
            .and_then(|x| x.split("</command-args>").next())
            .unwrap_or("")
            .trim();
        return Some(if args.is_empty() { name.to_owned() } else { format!("{name} {args}") });
    }
    Some(t.to_owned())
}

fn prompt_text(record: &Value) -> Option<String> {
    let content = record.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return tidy_prompt(s);
    }
    let mut out = String::new();
    for part in content.as_array()? {
        match part.get("type").and_then(Value::as_str) {
            // A user record carrying a tool_result is machine traffic, not a prompt.
            Some("tool_result") => return None,
            Some("text") => out.push_str(part.get("text").and_then(Value::as_str).unwrap_or("")),
            _ => {}
        }
    }
    tidy_prompt(&out)
}

fn first_prompt(head: &[Value]) -> Option<String> {
    let candidates: Vec<String> = typed(head, "user")
        .filter(|r| r.get("isMeta").and_then(Value::as_bool) != Some(true))
        .filter(|r| r.get("isCompactSummary").and_then(Value::as_bool) != Some(true))
        .filter_map(prompt_text)
        .collect();
    // A session that opens with /clear or /login says more from its next prompt.
    candidates
        .iter()
        .find(|c| !c.starts_with('/'))
        .or_else(|| candidates.first())
        .map(|c| c.chars().take(PROMPT_CHARS).collect())
}

/// `Ok(None)` when the file is not a transcript Claude would consider: a name
/// that is not a session uuid, or an empty file.
pub fn parse(path: &Utf8Path) -> Result<Option<Meta>> {
    let Some(stem) = path.file_stem() else { return Ok(None) };
    if !is_session_uuid(stem) {
        return Ok(None);
    }
    let mut f = File::open(path).map_err(|e| Error::io(path, e))?;
    // Length comes from the open handle and bounds every read: a concurrent
    // append can only add bytes past it, never move the ones we are reading.
    let size = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if size == 0 {
        return Ok(None);
    }
    let w = read_windows(&mut f, size).map_err(|e| match e {
        Error::BareIo(e) => Error::io(path, e),
        other => other,
    })?;
    let (head, tail) = (records(&w.head), records(&w.tail));

    // Claude resolves the cwd as relocatedCwd from the tail, then the head cwd.
    let cwd = typed(&tail, "relocated")
        .next_back()
        .and_then(|r| r.get("relocatedCwd")?.as_str().map(ToOwned::to_owned))
        .or_else(|| str_field(&head, "cwd"))
        .or_else(|| str_field(&tail, "cwd"));

    let title = typed(&tail, "custom-title")
        .next_back()
        .and_then(|r| r.get("customTitle")?.as_str().map(ToOwned::to_owned))
        .filter(|t| !t.trim().is_empty());

    let timestamps = |recs: &[Value]| -> Vec<i64> {
        recs.iter().filter_map(|r| iso_to_epoch_ms(r.get("timestamp")?.as_str()?)).collect()
    };

    let (head_ts, tail_ts) = (timestamps(&head), timestamps(&tail));

    Ok(Some(Meta {
        session_id: stem.to_owned(),
        record_id: str_field(&head, "sessionId"),
        origin_dir: path.parent().unwrap_or(path).to_owned(),
        path: path.to_owned(),
        size,
        cwd,
        git_branch: str_field(&head, "gitBranch").filter(|s| !s.is_empty()),
        cli_version: str_field(&head, "version"),
        title,
        first_prompt: first_prompt(&head),
        created_at_ms: head_ts.first().copied(),
        modified_at_ms: tail_ts.last().or_else(|| head_ts.last()).copied(),
        is_sidechain: head
            .iter()
            .any(|r| r.get("isSidechain").and_then(Value::as_bool) == Some(true)),
        session_kind: str_field(&head, "sessionKind"),
        team_name: str_field(&head, "teamName").filter(|s| !s.is_empty()),
        entrypoint: str_field(&head, "entrypoint"),
        tail_unresolved: w.unresolved,
    }))
}

#[derive(Debug, Default)]
pub struct Scan {
    pub sessions: Vec<Meta>,
    pub project_dirs: usize,
    /// `.orphaned-*` and anything else Claude itself skips.
    pub skipped: usize,
    /// Nested `<sessionId>/subagents/*.jsonl`. Part of a conversation, never one.
    pub subagents: usize,
    pub unreadable: Vec<(Utf8PathBuf, String)>,
}

/// Walks one config dir's project directories. Never recurses: project dirs also
/// hold `memory/` and `tool-results/`, and a naive glob counts subagent
/// transcripts as conversations, roughly doubling the total.
pub fn scan(config_dir: &Utf8Path) -> Scan {
    let mut out = Scan::default();
    let projects = config_dir.join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else { return out };

    for pd in entries.flatten() {
        let Ok(dir) = Utf8PathBuf::from_path_buf(pd.path()) else { continue };
        if !pd.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        out.project_dirs += 1;
        let Ok(files) = std::fs::read_dir(&dir) else { continue };

        for fe in files.flatten() {
            let Ok(path) = Utf8PathBuf::from_path_buf(fe.path()) else { continue };
            let Ok(ft) = fe.file_type() else { continue };
            if ft.is_dir() {
                out.subagents += count_jsonl(&path.join("subagents"));
                continue;
            }
            if !ft.is_file() || path.extension() != Some("jsonl") {
                continue;
            }
            match parse(&path) {
                Ok(Some(m)) => out.sessions.push(m),
                Ok(None) => out.skipped += 1,
                Err(e) => out.unreadable.push((path, e.to_string())),
            }
        }
    }
    out
}

fn count_jsonl(dir: &Utf8Path) -> usize {
    std::fs::read_dir(dir).map_or(0, |rd| {
        rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")).count()
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn only_session_uuids_are_transcripts() {
        assert!(is_session_uuid("3db70634-c5f9-4099-95e4-710261df621f"));
        assert!(!is_session_uuid("3db70634-c5f9-4099-95e4-710261df621"));
        assert!(!is_session_uuid("3db70634_c5f9_4099_95e4_710261df621f"));
        assert!(!is_session_uuid("agent-3db70634"));
        // What Claude's own orphan rename produces.
        assert!(!is_session_uuid("3db70634-c5f9-4099-95e4-710261df621f.orphaned-1787-abcd"));
    }

    #[test]
    fn slash_commands_render_as_commands_not_markup() {
        assert_eq!(tidy_prompt("<command-name>/clear</command-name>").as_deref(), Some("/clear"));
        assert_eq!(
            tidy_prompt("<command-name>/loop</command-name><command-args>5m</command-args>")
                .as_deref(),
            Some("/loop 5m")
        );
    }

    #[test]
    fn injected_context_is_not_a_prompt() {
        assert_eq!(tidy_prompt("<system-reminder>ignore me</system-reminder>"), None);
        assert_eq!(tidy_prompt("<local-command-stdout>out</local-command-stdout>"), None);
        assert_eq!(tidy_prompt("   "), None);
    }

    #[test]
    fn a_tool_result_masquerading_as_a_user_turn_is_not_a_prompt() {
        let r = serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "content": "42"}]}
        });
        assert_eq!(prompt_text(&r), None);
    }

    #[test]
    fn the_first_real_prompt_wins_over_a_leading_slash_command() {
        let head = records(concat!(
            r#"{"type":"user","message":{"content":[{"type":"text","text":"<command-name>/clear</command-name>"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"text","text":"fix the auth bug"}]}}"#,
        ));
        assert_eq!(first_prompt(&head).as_deref(), Some("fix the auth bug"));
    }

    #[test]
    fn a_slash_command_is_still_better_than_nothing() {
        let head = records(
            r#"{"type":"user","message":{"content":[{"type":"text","text":"<command-name>/login</command-name>"}]}}"#,
        );
        assert_eq!(first_prompt(&head).as_deref(), Some("/login"));
    }

    #[test]
    fn meta_and_compact_summary_turns_are_skipped() {
        let head = records(concat!(
            r#"{"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"meta"}]}}"#,
            "\n",
            r#"{"type":"user","isCompactSummary":true,"message":{"content":[{"type":"text","text":"summary"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"text","text":"real"}]}}"#,
        ));
        assert_eq!(first_prompt(&head).as_deref(), Some("real"));
    }

    #[test]
    fn hidden_reasons_are_named_not_swallowed() {
        let base = Meta {
            session_id: "x".into(),
            record_id: None,
            path: "/x".into(),
            origin_dir: "/".into(),
            size: 1,
            cwd: None,
            git_branch: None,
            cli_version: None,
            title: None,
            first_prompt: None,
            created_at_ms: None,
            modified_at_ms: None,
            is_sidechain: false,
            session_kind: None,
            team_name: None,
            entrypoint: None,
            tail_unresolved: false,
        };
        assert_eq!(base.hidden_from_resume(), None);
        assert_eq!(
            Meta { is_sidechain: true, ..base.clone() }.hidden_from_resume(),
            Some(Hidden::Sidechain)
        );
        assert_eq!(
            Meta { team_name: Some("t".into()), ..base.clone() }.hidden_from_resume(),
            Some(Hidden::Team)
        );
        for kind in ["daemon", "daemon-worker"] {
            assert_eq!(
                Meta { session_kind: Some(kind.into()), ..base.clone() }.hidden_from_resume(),
                Some(Hidden::Daemon)
            );
        }
        assert_eq!(Meta { session_kind: Some("bg".into()), ..base }.hidden_from_resume(), None);
    }
}
