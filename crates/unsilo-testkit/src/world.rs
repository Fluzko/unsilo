//! Builds a complete, throwaway Claude installation in a temp directory.
//!
//! Fixture cwds are always written in posix form on every platform. The project
//! directory name is a pure string transform of that cwd, and its output is
//! alphanumerics and dashes, so the resulting tree is byte-identical on Linux,
//! macOS and Windows. That is what lets the same end-to-end assertions run
//! everywhere without per-platform branches.
//!
//! Accounts and organizations are named readably in tests and materialise as
//! deterministic uuids on disk, because that is the only shape Claude uses and
//! non-uuid directories at that level mean something else entirely.

use crate::digest::TreeDigest;
use crate::ids::uuid_for;
use camino::{Utf8Path, Utf8PathBuf};
use serde_json::json;
use std::collections::BTreeMap;
use unsilo::claude::slug::slug_lossy;
use unsilo::env::{Env, MapVars, RealProbe, VAR_HOME, VAR_UNSILO_HOME, VAR_UNSILO_USER_DATA};

const DEFAULT_TS: &str = "2026-08-01T10:00:00.000Z";

#[derive(Debug, Clone)]
pub struct SessionSpec {
    name: String,
    cwd: String,
    title: Option<String>,
    created: String,
    modified: String,
    messages: usize,
    branch: Option<String>,
    model: String,
    cli_version: String,
    sidechain: bool,
    subagents: usize,
    partial_last_line: bool,
    giant_record: Option<usize>,
    relocated_to: Option<String>,
    desktop_entry: bool,
    session_kind: Option<String>,
    also_in: Option<(String, usize)>,
}

impl SessionSpec {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            cwd: "/home/u/proj".to_owned(),
            title: None,
            created: DEFAULT_TS.to_owned(),
            modified: DEFAULT_TS.to_owned(),
            messages: 2,
            branch: None,
            model: "claude-opus-5".to_owned(),
            cli_version: "2.1.241".to_owned(),
            sidechain: false,
            subagents: 0,
            partial_last_line: false,
            giant_record: None,
            relocated_to: None,
            desktop_entry: true,
            session_kind: None,
            also_in: None,
        }
    }

    #[must_use]
    pub fn cwd(mut self, v: &str) -> Self {
        v.clone_into(&mut self.cwd);
        self
    }
    #[must_use]
    pub fn title(mut self, v: &str) -> Self {
        self.title = Some(v.to_owned());
        self
    }
    #[must_use]
    pub fn created(mut self, v: &str) -> Self {
        v.clone_into(&mut self.created);
        self
    }
    #[must_use]
    pub fn modified(mut self, v: &str) -> Self {
        v.clone_into(&mut self.modified);
        self
    }
    #[must_use]
    pub fn messages(mut self, n: usize) -> Self {
        self.messages = n;
        self
    }
    #[must_use]
    pub fn branch(mut self, v: &str) -> Self {
        self.branch = Some(v.to_owned());
        self
    }
    #[must_use]
    pub fn model(mut self, v: &str) -> Self {
        v.clone_into(&mut self.model);
        self
    }
    #[must_use]
    pub fn cli_version(mut self, v: &str) -> Self {
        v.clone_into(&mut self.cli_version);
        self
    }
    /// Subagent transcript: part of a conversation, never a conversation itself.
    #[must_use]
    pub fn sidechain(mut self) -> Self {
        self.sidechain = true;
        self
    }
    /// Nested `<sessionId>/subagents/*.jsonl`, which a naive glob overcounts.
    #[must_use]
    pub fn subagents(mut self, n: usize) -> Self {
        self.subagents = n;
        self
    }
    /// Leaves a half-written final line, as a concurrent append would.
    #[must_use]
    pub fn partial_last_line(mut self) -> Self {
        self.partial_last_line = true;
        self
    }
    /// One record large enough to force the tail window to grow.
    #[must_use]
    pub fn giant_record(mut self, bytes: usize) -> Self {
        self.giant_record = Some(bytes);
        self
    }
    #[must_use]
    pub fn relocated_to(mut self, v: &str) -> Self {
        self.relocated_to = Some(v.to_owned());
        self
    }
    /// A session born in the CLI, with no entry in the desktop index.
    #[must_use]
    pub fn cli_only(mut self) -> Self {
        self.desktop_entry = false;
        self
    }
    /// The same uuid in a second project dir, padded to `extra_messages` more
    /// turns. Claude leaves these behind when a project moves; the copies are
    /// not links and their contents differ.
    #[must_use]
    pub fn also_in(mut self, cwd: &str, extra_messages: usize) -> Self {
        self.also_in = Some((cwd.to_owned(), extra_messages));
        self
    }

    #[must_use]
    pub fn session_kind(mut self, v: &str) -> Self {
        self.session_kind = Some(v.to_owned());
        self
    }

    #[must_use]
    pub fn session_id(&self) -> String {
        uuid_for(&self.name)
    }

    #[must_use]
    pub fn host_id(&self) -> String {
        format!("local_{}", uuid_for(&format!("host:{}", self.name)))
    }
}

#[derive(Debug, Default)]
pub struct OrgBuilder {
    sessions: Vec<SessionSpec>,
    tombstones: Vec<String>,
}

impl OrgBuilder {
    pub fn session(&mut self, name: &str, f: impl FnOnce(SessionSpec) -> SessionSpec) -> &mut Self {
        self.sessions.push(f(SessionSpec::new(name)));
        self
    }

    /// A `deleted_<uuid>` marker: the user removed the session from this account.
    pub fn tombstone(&mut self, session_name: &str) -> &mut Self {
        self.tombstones.push(uuid_for(session_name));
        self
    }
}

#[derive(Debug)]
pub struct AccountBuilder {
    orgs: Vec<(String, String, OrgBuilder)>,
}

impl AccountBuilder {
    pub fn org(&mut self, id: &str, name: &str, f: impl FnOnce(&mut OrgBuilder)) -> &mut Self {
        let mut b = OrgBuilder::default();
        f(&mut b);
        self.orgs.push((id.to_owned(), name.to_owned(), b));
        self
    }
}

#[derive(Debug, Default)]
pub struct WorldBuilder {
    accounts: Vec<(String, String, AccountBuilder)>,
    active: Option<(String, String)>,
    hover_rest: bool,
    credentials_sentinel: Option<String>,
    orphaned: Vec<String>,
    sentinel_dirs: Vec<String>,
}

impl WorldBuilder {
    pub fn account(
        &mut self,
        id: &str,
        email: &str,
        f: impl FnOnce(&mut AccountBuilder),
    ) -> &mut Self {
        let mut b = AccountBuilder { orgs: Vec::new() };
        f(&mut b);
        self.accounts.push((id.to_owned(), email.to_owned(), b));
        self
    }

    pub fn active(&mut self, account: &str, org: &str) -> &mut Self {
        self.active = Some((account.to_owned(), org.to_owned()));
        self
    }

    /// Turns on the remote transcript backend flag, under which transcripts stop
    /// being plain files and Unsilo must refuse to write.
    pub fn hover_rest(&mut self, on: bool) -> &mut Self {
        self.hover_rest = on;
        self
    }

    /// Plants a canary in every credential file, so a snapshot test can prove
    /// the allow-list never let one through.
    pub fn credentials_with_sentinel(&mut self, token: &str) -> &mut Self {
        self.credentials_sentinel = Some(token.to_owned());
        self
    }

    /// A non-uuid directory sitting where account directories live. `skills-plugin`
    /// is a real one, and it nests differently underneath.
    pub fn sentinel_dir(&mut self, name: &str) -> &mut Self {
        self.sentinel_dirs.push(name.to_owned());
        self
    }

    /// A `<id>.orphaned-<ts>-<rand>.jsonl` left behind by Claude's own rename.
    pub fn orphaned(&mut self, name: &str, cwd: &str) -> &mut Self {
        self.orphaned.push(format!("{name}\u{0}{cwd}"));
        self
    }

    pub fn build(&mut self) -> World {
        World::materialize(self)
    }
}

#[derive(Debug)]
pub struct World {
    // Dropping this removes the whole tree, so it must outlive every path below.
    _root: tempfile::TempDir,
    pub root: Utf8PathBuf,
    pub home: Utf8PathBuf,
    pub config_dir: Utf8PathBuf,
    pub user_data: Utf8PathBuf,
    pub unsilo_home: Utf8PathBuf,
    sessions: BTreeMap<String, SessionSpec>,
}

/// Readable fixture name to the uuid it takes on disk.
#[must_use]
pub fn scope_uuid(name: &str) -> String {
    if crate::ids::looks_like_uuid(name) { name.to_owned() } else { uuid_for(name) }
}

impl World {
    #[must_use]
    pub fn builder() -> WorldBuilder {
        WorldBuilder::default()
    }

    fn materialize(b: &WorldBuilder) -> World {
        #[allow(clippy::expect_used)]
        let tmp = tempfile::tempdir().expect("tempdir");
        #[allow(clippy::expect_used)]
        let root = Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8 tempdir");

        let home = root.join("home");
        let config_dir = home.join(".claude");
        let user_data = root.join("userdata");
        let unsilo_home = root.join("unsilo");
        mkdir(&config_dir.join("projects"));
        mkdir(&config_dir.join("backups"));
        mkdir(&user_data.join("claude-code-sessions"));
        mkdir(&unsilo_home);

        let mut sessions = BTreeMap::new();

        for (acct, _email, ab) in &b.accounts {
            let acct = scope_uuid(acct);
            for (org, _org_name, ob) in &ab.orgs {
                let org = scope_uuid(org);
                let entry_dir = user_data.join("claude-code-sessions").join(&acct).join(&org);
                mkdir(&entry_dir);
                for s in &ob.sessions {
                    write_transcript(&config_dir, s);
                    if let Some((cwd, extra)) = &s.also_in {
                        let mut twin = s.clone();
                        twin.cwd.clone_from(cwd);
                        twin.messages += extra;
                        twin.also_in = None;
                        write_transcript(&config_dir, &twin);
                    }
                    if s.desktop_entry {
                        write_desktop_entry(&entry_dir, s);
                    }
                    sessions.insert(s.name.clone(), s.clone());
                }
                for t in &ob.tombstones {
                    write(&entry_dir.join(format!("deleted_{t}")), b"1787228048004");
                }
            }
        }

        for spec in &b.orphaned {
            let (name, cwd) = spec.split_once('\u{0}').unwrap_or((spec.as_str(), "/home/u/proj"));
            let dir = config_dir.join("projects").join(slug_lossy(cwd));
            mkdir(&dir);
            write(
                &dir.join(format!("{}.orphaned-1787228048004-abcd1234.jsonl", uuid_for(name))),
                b"{}\n",
            );
        }

        for name in &b.sentinel_dirs {
            for surface in ["claude-code-sessions", "local-agent-mode-sessions"] {
                let dir = user_data.join(surface).join(name).join(uuid_for("sentinel-org"));
                mkdir(&dir);
                write(&dir.join("scheduled-tasks.json"), b"{}");
            }
        }

        write_config_json(&home, b);
        if let Some(token) = &b.credentials_sentinel {
            write_credentials(&config_dir, token);
        }

        World { _root: tmp, root, home, config_dir, user_data, unsilo_home, sessions }
    }

    #[must_use]
    pub fn env(&self) -> Env {
        #[allow(clippy::expect_used)]
        Env::from_vars(&self.vars(), &RealProbe).expect("fixture env")
    }

    #[must_use]
    pub fn vars(&self) -> MapVars {
        MapVars::new()
            .with(VAR_HOME, self.home.as_str())
            .with(VAR_UNSILO_USER_DATA, self.user_data.as_str())
            .with(VAR_UNSILO_HOME, self.unsilo_home.as_str())
    }

    /// Environment for running the real binary in an end-to-end test.
    #[must_use]
    pub fn env_pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            (VAR_HOME, self.home.to_string()),
            (VAR_UNSILO_USER_DATA, self.user_data.to_string()),
            (VAR_UNSILO_HOME, self.unsilo_home.to_string()),
        ]
    }

    /// Digest of everything Unsilo could touch: Claude's tree and its own store.
    #[must_use]
    pub fn digest(&self) -> TreeDigest {
        TreeDigest::of(&self.root)
    }

    /// Everything Claude owns: the CLI config dir and the desktop's userData.
    /// Both are written to, so a comparison over one of them alone would miss
    /// half of what apply does.
    #[must_use]
    pub fn claude_digest(&self) -> TreeDigest {
        TreeDigest::of_many(&[("home", &self.home), ("userdata", &self.user_data)])
    }

    #[must_use]
    pub fn account_uuid(&self, name: &str) -> String {
        scope_uuid(name)
    }

    #[must_use]
    pub fn org_uuid(&self, name: &str) -> String {
        scope_uuid(name)
    }

    #[must_use]
    pub fn session_id(&self, name: &str) -> Option<String> {
        self.sessions.get(name).map(SessionSpec::session_id)
    }

    #[must_use]
    pub fn transcript_path(&self, name: &str) -> Option<Utf8PathBuf> {
        let s = self.sessions.get(name)?;
        Some(
            self.config_dir
                .join("projects")
                .join(slug_lossy(&s.cwd))
                .join(format!("{}.jsonl", s.session_id())),
        )
    }

    /// Claude's retention cleanup removing a transcript from its project dir.
    pub fn simulate_retention_cleanup(&self, name: &str) -> bool {
        self.transcript_path(name).is_some_and(|p| std::fs::remove_file(p).is_ok())
    }

    /// The desktop rewriting an index entry, which is what happens when a
    /// projected session is resumed under the account it was projected into.
    pub fn simulate_desktop_rewrite(&self, account: &str, org: &str, host_id: &str) -> bool {
        let p = self
            .user_data
            .join("claude-code-sessions")
            .join(scope_uuid(account))
            .join(scope_uuid(org))
            .join(format!("{host_id}.json"));
        let Ok(raw) = std::fs::read(&p) else { return false };
        let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&raw) else { return false };
        if let Some(o) = v.as_object_mut() {
            o.insert("lastActivityAt".to_owned(), json!(1_800_000_000_000_i64));
            o.insert("completedTurns".to_owned(), json!(99));
        }
        #[allow(clippy::expect_used)]
        let bytes = serde_json::to_vec(&v).expect("serialize");
        std::fs::write(&p, bytes).is_ok()
    }
}

fn mkdir(p: &Utf8Path) {
    let _ = std::fs::create_dir_all(p);
}

/// Claude writes its transcripts and index entries owner-only, so the fixtures
/// do too. Otherwise a restore that gets the permissions right looks like a
/// difference.
fn write(p: &Utf8Path, bytes: &[u8]) {
    if let Some(parent) = p.parent() {
        mkdir(parent);
    }
    let _ = std::fs::write(p, bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }
}

fn write_transcript(config_dir: &Utf8Path, s: &SessionSpec) {
    let dir = config_dir.join("projects").join(slug_lossy(&s.cwd));
    mkdir(&dir);
    let id = s.session_id();
    let mut out = String::new();

    let base = |ts: &str| {
        json!({
            "sessionId": id,
            "timestamp": ts,
            "cwd": s.cwd,
            "version": s.cli_version,
            "gitBranch": s.branch.clone().unwrap_or_default(),
            "isSidechain": s.sidechain,
            "userType": "external",
            "entrypoint": "cli",
        })
    };

    // Real transcripts open with queue bookkeeping that carries no cwd, so the
    // parser has to scan forward rather than trust the first record.
    push(
        &mut out,
        &json!({"type":"queue-operation","operation":"enqueue","sessionId":id,"timestamp":s.created}),
    );

    for i in 0..s.messages {
        let mut user = base(&s.created);
        merge(
            &mut user,
            &json!({
                "type": "user",
                "uuid": uuid_for(&format!("{}:u{i}", s.name)),
                "message": {"role":"user","content":[{"type":"text","text":format!("prompt {i} for {}", s.name)}]},
            }),
        );
        if let Some(kind) = &s.session_kind {
            merge(&mut user, &json!({ "sessionKind": kind }));
        }
        push(&mut out, &user);

        let mut asst = base(&s.modified);
        merge(
            &mut asst,
            &json!({
                "type": "assistant",
                "uuid": uuid_for(&format!("{}:a{i}", s.name)),
                "message": {"role":"assistant","content":[{"type":"text","text":format!("reply {i}")}]},
            }),
        );
        push(&mut out, &asst);
    }

    if let Some(bytes) = s.giant_record {
        let mut big = base(&s.modified);
        merge(
            &mut big,
            &json!({
                "type": "user",
                "isMeta": true,
                "toolUseResult": "x".repeat(bytes),
            }),
        );
        push(&mut out, &big);
    }

    if let Some(title) = &s.title {
        push(
            &mut out,
            &json!({"type":"custom-title","customTitle":title,"sessionId":id,"timestamp":s.modified}),
        );
    }
    if let Some(cwd) = &s.relocated_to {
        push(
            &mut out,
            &json!({"type":"relocated","relocatedCwd":cwd,"sessionId":id,"timestamp":s.modified}),
        );
    }

    if s.partial_last_line {
        out.push_str(r#"{"type":"user","sessi"#);
    }

    write(&dir.join(format!("{id}.jsonl")), out.as_bytes());

    for i in 0..s.subagents {
        let sub = dir.join(&id).join("subagents");
        mkdir(&sub);
        let sub_id = uuid_for(&format!("{}:sub{i}", s.name));
        let mut rec = base(&s.modified);
        merge(&mut rec, &json!({"type":"user","isSidechain":true,"uuid":sub_id}));
        let mut body = String::new();
        push(&mut body, &rec);
        write(&sub.join(format!("{sub_id}.jsonl")), body.as_bytes());
    }
}

fn push(out: &mut String, v: &serde_json::Value) {
    if let Ok(s) = serde_json::to_string(v) {
        out.push_str(&s);
        out.push('\n');
    }
}

fn merge(target: &mut serde_json::Value, extra: &serde_json::Value) {
    let (Some(t), Some(e)) = (target.as_object_mut(), extra.as_object()) else { return };
    for (k, v) in e {
        t.insert(k.clone(), v.clone());
    }
}

fn write_desktop_entry(dir: &Utf8Path, s: &SessionSpec) {
    let v = json!({
        "sessionId": s.host_id(),
        "cliSessionId": s.session_id(),
        "cwd": s.cwd,
        "originCwd": s.cwd,
        "sourceBranch": s.branch.clone().unwrap_or_else(|| "main".to_owned()),
        "createdAt": epoch_ms(&s.created),
        "lastActivityAt": epoch_ms(&s.modified),
        "lastFocusedAt": epoch_ms(&s.modified),
        "model": s.model,
        "effort": "xhigh",
        "isArchived": false,
        "title": s.title.clone().unwrap_or_else(|| format!("session {}", s.name)),
        "titleSource": "auto",
        "permissionMode": "auto",
        // Account-scoped payload: the bulk of the file, and what projection strips.
        "enabledMcpTools": {"local:example:tool-abc": true},
        "remoteMcpServersConfig": [{"uuid": uuid_for("mcp"), "name": "example", "tools": ["a", "b"]}],
        "completedTurns": 3,
    });
    if let Ok(bytes) = serde_json::to_vec(&v) {
        write(&dir.join(format!("{}.json", s.host_id())), &bytes);
    }
}

/// Keeps a desktop entry's timestamps in step with its transcript's, which is
/// what makes an entry usable as a dated sighting.
fn epoch_ms(iso: &str) -> i64 {
    unsilo::claude::time::iso_to_epoch_ms(iso).unwrap_or(0)
}

fn write_config_json(home: &Utf8Path, b: &WorldBuilder) {
    let active = b.active.clone().or_else(|| {
        let (acct, _, ab) = b.accounts.first()?;
        let (org, _, _) = ab.orgs.first()?;
        Some((acct.clone(), org.clone()))
    });

    let mut oauth = json!({});
    if let Some((acct, org)) = &active {
        let email = b
            .accounts
            .iter()
            .find(|(id, _, _)| id == acct)
            .map(|(_, e, _)| e.clone())
            .unwrap_or_default();
        let org_name = b
            .accounts
            .iter()
            .find(|(id, _, _)| id == acct)
            .and_then(|(_, _, ab)| ab.orgs.iter().find(|(id, _, _)| id == org))
            .map(|(_, n, _)| n.clone())
            .unwrap_or_default();
        oauth = json!({
            "accountUuid": scope_uuid(acct),
            "organizationUuid": scope_uuid(org),
            "emailAddress": email,
            "organizationName": org_name,
            "displayName": "Fixture",
        });
    }

    let v = json!({
        "oauthAccount": oauth,
        "cachedGrowthBookFeatures": {"tengu_hover_rest": b.hover_rest},
        "projects": {},
    });
    if let Ok(bytes) = serde_json::to_vec_pretty(&v) {
        write(&home.join(".claude.json"), &bytes);
    }
}

fn write_credentials(config_dir: &Utf8Path, token: &str) {
    let v = json!({"claudeAiOauth": {"accessToken": token, "refreshToken": token}});
    if let Ok(bytes) = serde_json::to_vec(&v) {
        write(&config_dir.join(".credentials.json"), &bytes);
    }
    write(&config_dir.join("sessions").join("1234.abcd.key"), token.as_bytes());
}
