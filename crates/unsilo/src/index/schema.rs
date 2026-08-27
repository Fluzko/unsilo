//! Schema and migrations.
//!
//! The schema version is tracked in `SQLite`'s own `user_version` and is
//! independent of the binary version: a released Unsilo must be able to open an
//! index written by an older one. Migrations are append only and every published
//! version stays in the list so the upgrade path can be tested end to end.

pub const MIGRATIONS: &[&str] = &[
    // 1: sessions, desktop entries, tombstones, identities cache, ledger, search.
    r"
    CREATE TABLE session (
        session_id      TEXT PRIMARY KEY,
        record_id       TEXT,
        cwd             TEXT,
        project_slug    TEXT,
        origin_dir      TEXT NOT NULL,
        git_branch      TEXT,
        cli_version     TEXT,
        title           TEXT,
        first_prompt    TEXT,
        created_at_ms   INTEGER,
        modified_at_ms  INTEGER,
        size_bytes      INTEGER NOT NULL DEFAULT 0,
        hidden_reason   TEXT,
        store_path      TEXT,
        link_kind       TEXT,
        vol_id          INTEGER,
        file_id         INTEGER,
        seen_at_ms      INTEGER NOT NULL
    );
    CREATE INDEX session_modified ON session (modified_at_ms DESC);
    CREATE INDEX session_slug ON session (project_slug);

    CREATE TABLE desktop_entry (
        host_id         TEXT NOT NULL,
        account_uuid    TEXT NOT NULL,
        org_uuid        TEXT NOT NULL,
        surface         TEXT NOT NULL,
        session_id      TEXT,
        title           TEXT,
        cwd             TEXT,
        model           TEXT,
        created_at_ms   INTEGER,
        last_activity_ms INTEGER,
        is_archived     INTEGER NOT NULL DEFAULT 0,
        path            TEXT NOT NULL,
        PRIMARY KEY (host_id, account_uuid, org_uuid, surface)
    );
    CREATE INDEX desktop_entry_session ON desktop_entry (session_id);
    CREATE INDEX desktop_entry_scope ON desktop_entry (account_uuid, org_uuid);

    CREATE TABLE tombstone (
        account_uuid    TEXT NOT NULL,
        org_uuid        TEXT NOT NULL,
        surface         TEXT NOT NULL,
        id              TEXT NOT NULL,
        deleted_at_ms   INTEGER,
        PRIMARY KEY (account_uuid, org_uuid, surface, id)
    );

    CREATE TABLE ledger (
        path            TEXT PRIMARY KEY,
        session_id      TEXT,
        host_id         TEXT,
        kind            TEXT NOT NULL,
        content_hash    TEXT,
        byte_len        INTEGER,
        state           TEXT NOT NULL,
        created_at_ms   INTEGER NOT NULL
    );

    CREATE VIRTUAL TABLE session_fts USING fts5(
        session_id UNINDEXED,
        title,
        first_prompt,
        body,
        tokenize = 'unicode61 remove_diacritics 2'
    );
    ",
    // 2: the same session uuid can exist in more than one project dir, with
    // different content. Observed on a real machine: 6.9 MB in one, 12.6 MB in
    // another. Both are kept; `session.origin_dir` names the primary.
    r"
    CREATE TABLE session_location (
        session_id      TEXT NOT NULL,
        origin_dir      TEXT NOT NULL,
        project_slug    TEXT,
        size_bytes      INTEGER NOT NULL,
        modified_at_ms  INTEGER,
        cwd             TEXT,
        PRIMARY KEY (session_id, origin_dir)
    );
    CREATE INDEX session_location_session ON session_location (session_id);
    ",
    // 3: an entry Unsilo projected is where a session can now be seen, not where
    // it came from. Filtering by account has to mean origin, or narrowing the
    // filter after an apply would match the apply's own output and prune nothing.
    r"
    ALTER TABLE desktop_entry ADD COLUMN projected INTEGER NOT NULL DEFAULT 0;
    ",
    // 4: a CLI transcript records no account, so the only way to attribute one is
    // to remember which account was signed in at a given moment and compare. Kept
    // as discrete sightings rather than merged ranges: appending an observation
    // can never corrupt an earlier one.
    r"
    CREATE TABLE account_sighting (
        account_uuid    TEXT NOT NULL,
        org_uuid        TEXT NOT NULL,
        at_ms           INTEGER NOT NULL,
        source          TEXT NOT NULL,
        PRIMARY KEY (account_uuid, org_uuid, at_ms)
    );
    CREATE INDEX account_sighting_at ON account_sighting (at_ms);

    ALTER TABLE session ADD COLUMN inferred_account TEXT;
    ALTER TABLE session ADD COLUMN inferred_org TEXT;
    ",
    // 5: an entry synthesized from a CLI transcript is neither native nor a copy
    // of one; it exists because the desktop never knew about that conversation.
    // Tracked so it stays distinguishable in listings and filters.
    r"
    ALTER TABLE desktop_entry ADD COLUMN adopted INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE session ADD COLUMN model TEXT;
    ",
];

#[must_use]
pub fn latest_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}
