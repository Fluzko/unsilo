//! The catalogue: what exists, where it lives, and which account can see it.
//!
//! Rebuilding it from disk is always possible, so nothing here is authoritative.
//! The ledger is the exception, and it is what `off` reverts from.

pub mod schema;

use crate::claude::desktop::{Entry, Surface, Tombstone};
use crate::claude::transcript::Meta;
use crate::error::{Error, Result};

use crate::filter::Resolved;
use camino::{Utf8Path, Utf8PathBuf};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::Serialize;
use std::fmt::Write as _;

/// One thing Unsilo wrote outside its own store, and enough to undo it.
#[derive(Debug, Clone, Serialize)]
pub struct LedgerEntry {
    pub path: Utf8PathBuf,
    pub session_id: Option<String>,
    pub host_id: Option<String>,
    pub kind: String,
    pub content_hash: Option<String>,
    pub byte_len: Option<i64>,
    pub state: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub session_id: String,
    pub cwd: Option<String>,
    pub project_slug: Option<String>,
    pub origin_dir: String,
    pub git_branch: Option<String>,
    pub title: Option<String>,
    pub first_prompt: Option<String>,
    pub created_at_ms: Option<i64>,
    pub modified_at_ms: Option<i64>,
    pub size_bytes: i64,
    pub hidden_reason: Option<String>,
    /// Every account and organization whose list this session appears in.
    pub scopes: Vec<Scope>,
    /// The account this conversation probably belongs to, when it has no entry
    /// to say so outright. Never presented as a fact.
    pub inferred_account: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Scope {
    pub account: String,
    pub org: String,
    pub surface: String,
    pub host_id: String,
    pub model: Option<String>,
    pub is_archived: bool,
}

impl Row {
    /// Title, then first prompt. Never the file name.
    #[must_use]
    pub fn display_title(&self) -> Option<&str> {
        self.title.as_deref().or(self.first_prompt.as_deref())
    }

    #[must_use]
    pub fn short_id(&self) -> &str {
        self.session_id.get(..8).unwrap_or(&self.session_id)
    }
}

/// A session id with the timestamps needed to place it in time.
pub type UnattributedSession = (String, Option<i64>, Option<i64>);

#[derive(Debug)]
pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(path: &Utf8Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
        }
        let conn = Connection::open(path)?;
        Self::prepare(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> Result<Self> {
        // WAL keeps `find` readable while another process is mid apply.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let index = Self { conn };
        index.migrate()?;
        Ok(index)
    }

    fn migrate(&self) -> Result<()> {
        let current: u32 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, migration) in schema::MIGRATIONS.iter().enumerate() {
            let version = u32::try_from(i + 1).unwrap_or(u32::MAX);
            if version <= current {
                continue;
            }
            self.conn.execute_batch(migration)?;
            self.conn.pragma_update(None, "user_version", version)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn version(&self) -> u32 {
        self.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap_or(0)
    }

    /// Clears what a rescan rebuilds from the desktop side. Sessions are not
    /// cleared here: a transcript removed by retention cleanup still exists in
    /// the store, and dropping its row would lose the directory it belongs in,
    /// which is the one thing needed to put it back.
    pub fn clear_desktop(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM desktop_entry; DELETE FROM tombstone;")
            .map_err(Error::from)
    }

    /// Forgets sessions that were not seen in this scan and are not held by the
    /// store, so the index does not accumulate rows for things that are gone for
    /// good.
    pub fn forget_unseen(&self, seen_at_ms: i64, kept: &[String]) -> Result<usize> {
        let placeholders: Vec<String> = (0..kept.len()).map(|i| format!("?{}", i + 2)).collect();
        let clause = if kept.is_empty() {
            String::new()
        } else {
            format!(" AND session_id NOT IN ({})", placeholders.join(", "))
        };
        let mut binds: Vec<rusqlite::types::Value> = vec![seen_at_ms.into()];
        binds.extend(kept.iter().map(|k| rusqlite::types::Value::from(k.clone())));

        for table in ["session_location", "session_fts", "session"] {
            let sql = format!(
                "DELETE FROM {table} WHERE session_id IN (
                    SELECT session_id FROM session WHERE seen_at_ms < ?1{clause}
                 )"
            );
            self.conn.execute(&sql, params_from_iter(binds.iter()))?;
        }
        Ok(usize::try_from(self.conn.changes()).unwrap_or(0))
    }

    pub fn record_sighting(&self, sighting: &crate::attribution::Sighting) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO account_sighting (account_uuid, org_uuid, at_ms, source)
             VALUES (?1,?2,?3,?4)",
            params![sighting.account, sighting.org, sighting.at_ms, sighting.source.as_str()],
        )?;
        Ok(())
    }

    pub fn sightings(&self) -> Result<Vec<crate::attribution::Sighting>> {
        use crate::attribution::{Sighting, Source};
        let mut stmt = self.conn.prepare(
            "SELECT account_uuid, org_uuid, at_ms, source FROM account_sighting ORDER BY at_ms",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Sighting {
                    account: r.get(0)?,
                    org: r.get(1)?,
                    at_ms: r.get(2)?,
                    source: Source::parse(&r.get::<_, String>(3)?).unwrap_or(Source::Observed),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Sessions with no desktop entry anywhere, which are the only ones an
    /// inference is needed for.
    pub fn unattributed_sessions(&self) -> Result<Vec<UnattributedSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.session_id, s.created_at_ms, s.modified_at_ms FROM session s
             WHERE NOT EXISTS (SELECT 1 FROM desktop_entry d WHERE d.session_id = s.session_id)",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_inferred(&self, session_id: &str, account: &str, org: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE session SET inferred_account = ?2, inferred_org = NULLIF(?3, '')
             WHERE session_id = ?1",
            params![session_id, account, org],
        )?;
        Ok(())
    }

    pub fn clear_inferences(&self) -> Result<()> {
        self.conn.execute("UPDATE session SET inferred_account = NULL, inferred_org = NULL", [])?;
        Ok(())
    }

    /// `(attributed, total)` over the sessions that need an inference at all.
    pub fn attribution_coverage(&self) -> Result<(usize, usize)> {
        let row: (i64, i64) = self.conn.query_row(
            "SELECT
                COUNT(*) FILTER (WHERE s.inferred_account IS NOT NULL),
                COUNT(*)
             FROM session s
             WHERE NOT EXISTS (SELECT 1 FROM desktop_entry d WHERE d.session_id = s.session_id)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((usize::try_from(row.0).unwrap_or(0), usize::try_from(row.1).unwrap_or(0)))
    }

    pub fn origin_dir_of(&self, session_id: &str) -> Result<Option<Utf8PathBuf>> {
        let dir: Option<String> = self
            .conn
            .query_row(
                "SELECT origin_dir FROM session WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(dir.map(Utf8PathBuf::from))
    }

    pub fn upsert_session(&self, meta: &Meta, seen_at_ms: i64) -> Result<()> {
        let slug = meta.origin_dir.file_name();
        self.conn.execute(
            "INSERT INTO session (
                    session_id, record_id, cwd, project_slug, origin_dir, git_branch,
                    cli_version, title, first_prompt, created_at_ms, modified_at_ms,
                    size_bytes, hidden_reason, seen_at_ms
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(session_id) DO UPDATE SET
                    cwd = excluded.cwd,
                    origin_dir = excluded.origin_dir,
                    git_branch = excluded.git_branch,
                    cli_version = excluded.cli_version,
                    -- A title that scrolled out of the tail window would come back
                    -- as NULL; keep the one we already saw.
                    title = COALESCE(excluded.title, session.title),
                    first_prompt = COALESCE(excluded.first_prompt, session.first_prompt),
                    modified_at_ms = excluded.modified_at_ms,
                    size_bytes = excluded.size_bytes,
                    hidden_reason = excluded.hidden_reason,
                    seen_at_ms = excluded.seen_at_ms",
            params![
                meta.session_id,
                meta.record_id,
                meta.cwd,
                slug,
                meta.origin_dir.as_str(),
                meta.git_branch,
                meta.cli_version,
                meta.title,
                meta.first_prompt,
                meta.created_at_ms,
                meta.modified_at_ms,
                i64::try_from(meta.size).unwrap_or(i64::MAX),
                meta.hidden_from_resume().map(crate::claude::transcript::Hidden::as_str),
                seen_at_ms,
            ],
        )?;
        self.conn.execute(
            "INSERT OR REPLACE INTO session_location
                 (session_id, origin_dir, project_slug, size_bytes, modified_at_ms, cwd)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                meta.session_id,
                meta.origin_dir.as_str(),
                slug,
                i64::try_from(meta.size).unwrap_or(i64::MAX),
                meta.modified_at_ms,
                meta.cwd,
            ],
        )?;
        self.promote_primary_location(&meta.session_id)?;
        self.reindex_text(&meta.session_id)?;
        Ok(())
    }

    /// Picks which copy `session` points at when a uuid exists in several project
    /// directories. Transcripts are append only, so the largest is the most
    /// complete history; ties fall back to the newest and then to the path, so
    /// the choice never depends on directory iteration order.
    fn promote_primary_location(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE session SET
                    origin_dir = winner.origin_dir,
                    project_slug = winner.project_slug,
                    size_bytes = winner.size_bytes
                 FROM (
                    SELECT origin_dir, project_slug, size_bytes FROM session_location
                    WHERE session_id = ?1
                    ORDER BY size_bytes DESC, modified_at_ms DESC, origin_dir ASC
                    LIMIT 1
                 ) AS winner
                 WHERE session.session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Sessions whose uuid was found in more than one project directory, largest
    /// copy first.
    pub fn duplicate_locations(&self) -> Result<Vec<(String, Vec<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, origin_dir FROM session_location
                 WHERE session_id IN (
                    SELECT session_id FROM session_location
                    GROUP BY session_id HAVING COUNT(*) > 1
                 ) ORDER BY session_id, size_bytes DESC, origin_dir ASC",
        )?;
        let pairs = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut out: Vec<(String, Vec<String>)> = Vec::new();
        for (session_id, dir) in pairs {
            match out.last_mut() {
                Some((id, dirs)) if *id == session_id => dirs.push(dir),
                _ => out.push((session_id, vec![dir])),
            }
        }
        Ok(out)
    }

    pub fn record_store_link(
        &self,
        session_id: &str,
        store_path: &Utf8Path,
        link_kind: &str,
        file_id: Option<(u64, u64)>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE session SET store_path = ?2, link_kind = ?3, vol_id = ?4, file_id = ?5
                 WHERE session_id = ?1",
            params![
                session_id,
                store_path.as_str(),
                link_kind,
                file_id.map(|(v, _)| i64::from_ne_bytes(v.to_ne_bytes())),
                file_id.map(|(_, f)| i64::from_ne_bytes(f.to_ne_bytes())),
            ],
        )?;
        Ok(())
    }

    pub fn upsert_desktop_entry(&self, entry: &Entry) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO desktop_entry (
                    host_id, account_uuid, org_uuid, surface, session_id, title, cwd,
                    model, created_at_ms, last_activity_ms, is_archived, path
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                entry.host_id,
                entry.scope.account,
                entry.scope.org,
                surface_name(entry.scope.surface),
                entry.cli_session_id,
                entry.title,
                entry.cwd,
                entry.model,
                entry.created_at_ms,
                entry.last_activity_ms,
                i32::from(entry.is_archived),
                entry.path.as_str(),
            ],
        )?;
        Ok(())
    }

    /// Flags the entries Unsilo itself wrote, by matching the ledger on path.
    pub fn mark_projected_entries(&self) -> Result<usize> {
        let changed = self.conn.execute(
            "UPDATE desktop_entry SET projected = 1
             WHERE path IN (SELECT path FROM ledger WHERE kind = 'desktop_entry')",
            [],
        )?;
        Ok(changed)
    }

    pub fn upsert_tombstone(&self, tombstone: &Tombstone) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tombstone
                 (account_uuid, org_uuid, surface, id, deleted_at_ms) VALUES (?1,?2,?3,?4,?5)",
            params![
                tombstone.scope.account,
                tombstone.scope.org,
                surface_name(tombstone.scope.surface),
                tombstone.id,
                tombstone.deleted_at_ms,
            ],
        )?;
        Ok(())
    }

    fn reindex_text(&self, session_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM session_fts WHERE session_id = ?1", params![session_id])?;
        self.conn.execute(
            "INSERT INTO session_fts (session_id, title, first_prompt, body)
                 SELECT session_id, COALESCE(title,''), COALESCE(first_prompt,''), ''
                 FROM session WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    pub fn count_sessions(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Every session matching the filter, newest first unless told otherwise.
    ///
    /// The SQL is assembled from a fixed set of fragments with bound parameters;
    /// no user text ever reaches the statement text.
    pub fn query(&self, filter: &Resolved) -> Result<Vec<Row>> {
        let (sql, binds) = build_query(filter);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(binds.iter()), |r| {
                Ok(Row {
                    session_id: r.get(0)?,
                    cwd: r.get(1)?,
                    project_slug: r.get(2)?,
                    origin_dir: r.get(3)?,
                    git_branch: r.get(4)?,
                    title: r.get(5)?,
                    first_prompt: r.get(6)?,
                    created_at_ms: r.get(7)?,
                    modified_at_ms: r.get(8)?,
                    size_bytes: r.get(9)?,
                    hidden_reason: r.get(10)?,
                    inferred_account: r.get(11)?,
                    scopes: Vec::new(),
                })
            })?
            .collect::<std::result::Result<Vec<Row>, _>>()?;

        rows.into_iter()
            .map(|mut row| {
                row.scopes = self.scopes_of(&row.session_id)?;
                Ok(row)
            })
            .collect()
    }

    fn scopes_of(&self, session_id: &str) -> Result<Vec<Scope>> {
        let mut stmt = self.conn.prepare(
            "SELECT account_uuid, org_uuid, surface, host_id, model, is_archived
             FROM desktop_entry WHERE session_id = ?1 ORDER BY account_uuid, org_uuid",
        )?;
        let scopes = stmt
            .query_map(params![session_id], |r| {
                Ok(Scope {
                    account: r.get(0)?,
                    org: r.get(1)?,
                    surface: r.get(2)?,
                    host_id: r.get(3)?,
                    model: r.get(4)?,
                    is_archived: r.get::<_, i32>(5)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<Scope>, _>>()?;
        Ok(scopes)
    }

    pub fn ledger_begin(
        &self,
        path: &Utf8Path,
        kind: &str,
        session_id: Option<&str>,
        host_id: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ledger
             (path, session_id, host_id, kind, content_hash, byte_len, state, created_at_ms)
             VALUES (?1,?2,?3,?4,NULL,NULL,'pending',?5)",
            params![path.as_str(), session_id, host_id, kind, now_ms],
        )?;
        Ok(())
    }

    pub fn ledger_commit(&self, path: &Utf8Path, hash: &str, byte_len: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE ledger SET content_hash = ?2, byte_len = ?3, state = 'done' WHERE path = ?1",
            params![path.as_str(), hash, i64::try_from(byte_len).unwrap_or(i64::MAX)],
        )?;
        Ok(())
    }

    pub fn ledger_forget(&self, path: &Utf8Path) -> Result<()> {
        self.conn.execute("DELETE FROM ledger WHERE path = ?1", params![path.as_str()])?;
        Ok(())
    }

    pub fn ledger_entries(&self) -> Result<Vec<LedgerEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, session_id, host_id, kind, content_hash, byte_len, state, created_at_ms
             FROM ledger ORDER BY created_at_ms, path",
        )?;
        let entries = stmt
            .query_map([], |r| {
                Ok(LedgerEntry {
                    path: Utf8PathBuf::from(r.get::<_, String>(0)?),
                    session_id: r.get(1)?,
                    host_id: r.get(2)?,
                    kind: r.get(3)?,
                    content_hash: r.get(4)?,
                    byte_len: r.get(5)?,
                    state: r.get(6)?,
                    created_at_ms: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    pub fn store_path_of(&self, session_id: &str) -> Result<Option<Utf8PathBuf>> {
        let path: Option<String> = self
            .conn
            .query_row(
                "SELECT store_path FROM session WHERE session_id = ?1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(path.map(Utf8PathBuf::from))
    }
}

/// Everything that narrows by where a conversation can be seen: account,
/// organization, surface, model, archived.
fn push_scope_predicates(
    filter: &Resolved,
    where_sql: &mut Vec<String>,
    bind: &mut impl FnMut(rusqlite::types::Value) -> usize,
) {
    if !filter.accounts.is_empty() {
        // Match where a session came from, not where Unsilo made it visible. An
        // inference has no desktop row at all, so it is a separate branch.
        let confirmed = in_clause("d.account_uuid", &filter.accounts, bind)
            .map(|c| format!("({c} AND d.projected = 0)"));
        let inferred = if filter.confirmed_only {
            None
        } else {
            in_clause("s.inferred_account", &filter.accounts, bind)
        };
        let branches: Vec<String> = [confirmed, inferred].into_iter().flatten().collect();
        if !branches.is_empty() {
            where_sql.push(format!("({})", branches.join(" OR ")));
        }
    }
    if let Some(clause) = in_clause("d.org_uuid", &filter.orgs, bind) {
        where_sql.push(clause);
    }
    if let Some(clause) = in_clause(
        "d.surface",
        &filter.surfaces.iter().map(|s| surface_name(*s).to_owned()).collect::<Vec<_>>(),
        bind,
    ) {
        where_sql.push(clause);
    }
    if let Some(model) = &filter.model {
        let i = bind(format!("%{model}%").into());
        where_sql.push(format!("d.model LIKE ?{i}"));
    }
    if filter.archived_only {
        where_sql.push("d.is_archived = 1".to_owned());
    }
}

/// Assembles the statement from a fixed set of fragments with bound parameters.
/// No user text ever reaches the statement text.
fn build_query(filter: &Resolved) -> (String, Vec<rusqlite::types::Value>) {
    let mut where_sql: Vec<String> = Vec::new();
    let mut binds: Vec<rusqlite::types::Value> = Vec::new();
    let mut bind = |value: rusqlite::types::Value| -> usize {
        binds.push(value);
        binds.len()
    };

    if !filter.include_hidden {
        where_sql.push("s.hidden_reason IS NULL".to_owned());
    }
    if let Some(cwd) = &filter.cwd {
        let i = bind(format!("{cwd}%").into());
        where_sql.push(format!("s.cwd LIKE ?{i}"));
    }
    for (column, value) in
        [("s.project_slug", filter.project.as_ref()), ("s.git_branch", filter.branch.as_ref())]
    {
        if let Some(v) = value {
            let i = bind(format!("%{v}%").into());
            where_sql.push(format!("{column} LIKE ?{i}"));
        }
    }
    if let Some(title) = &filter.title {
        let i = bind(format!("%{title}%").into());
        where_sql.push(format!("COALESCE(s.title, s.first_prompt, '') LIKE ?{i}"));
    }
    if let Some(id) = &filter.id {
        let i = bind(format!("{id}%").into());
        where_sql.push(format!("s.session_id LIKE ?{i}"));
    }
    if let Some(since) = filter.since_ms {
        let i = bind(since.into());
        where_sql.push(format!("s.modified_at_ms >= ?{i}"));
    }
    if let Some(until) = filter.until_ms {
        let i = bind(until.into());
        where_sql.push(format!("s.modified_at_ms <= ?{i}"));
    }
    if let Some(query) = &filter.query {
        let i = bind(query.clone().into());
        where_sql.push(format!(
            "s.session_id IN (SELECT session_id FROM session_fts WHERE session_fts MATCH ?{i})"
        ));
    }
    push_scope_predicates(filter, &mut where_sql, &mut bind);
    if !filter.include_deleted {
        where_sql.push(
            "NOT EXISTS (SELECT 1 FROM tombstone t
                   WHERE t.account_uuid = d.account_uuid AND t.org_uuid = d.org_uuid
                     AND (t.id = s.session_id OR 'local_' || t.id = d.host_id))"
                .to_owned(),
        );
    }

    // A LEFT JOIN keeps CLI-born sessions, which have no desktop entry at all.
    let joins_desktop = (!filter.accounts.is_empty() && filter.confirmed_only)
        || !filter.orgs.is_empty()
        || !filter.surfaces.is_empty()
        || filter.model.is_some()
        || filter.archived_only;
    let join = if joins_desktop { "JOIN" } else { "LEFT JOIN" };

    let mut sql = format!(
        "SELECT DISTINCT s.session_id, s.cwd, s.project_slug, s.origin_dir, s.git_branch,
                    s.title, s.first_prompt, s.created_at_ms, s.modified_at_ms, s.size_bytes,
                    s.hidden_reason, s.inferred_account
             FROM session s {join} desktop_entry d ON d.session_id = s.session_id"
    );
    if !where_sql.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_sql.join(" AND "));
    }
    sql.push_str(" ORDER BY ");
    sql.push_str(filter.sort.order_by());
    if let Some(limit) = filter.limit {
        let _ = write!(sql, " LIMIT {}", limit.min(10_000));
    }
    (sql, binds)
}

fn in_clause(
    column: &str,
    values: &[String],
    bind: &mut impl FnMut(rusqlite::types::Value) -> usize,
) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let placeholders: Vec<String> =
        values.iter().map(|v| format!("?{}", bind(v.clone().into()))).collect();
    Some(format!("{column} IN ({})", placeholders.join(", ")))
}

#[must_use]
pub fn surface_name(surface: Surface) -> &'static str {
    match surface {
        Surface::Code => "code",
        Surface::Cowork => "cowork",
    }
}
