//! The store: one hard link per transcript, in a directory of our own.
//!
//! Linking, never moving. The original stays exactly where Claude expects it,
//! and there is no window in which a transcript is missing from its project
//! directory. The side effect is the point: when Claude's retention cleanup
//! unlinks its copy, the inode survives here because the link count never
//! reached zero.
//!
//! That makes the store the last remaining copy of some transcripts, so nothing
//! in this module deletes from it.

use crate::claude::transcript::Meta;
use crate::env::{Env, LinkKind};
use crate::error::{Error, Result};
use crate::fsx;
use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Outcome {
    /// First time we have seen this session.
    Linked,
    /// Already the same file. Nothing to do, which is the common case.
    AlreadyLinked,
    /// The store held a copy that had fallen behind and was refreshed.
    Refreshed,
    /// Two different files claim the same session id and neither contains the
    /// other. The store keeps what it had; the caller decides what to say.
    Diverged,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ingested {
    pub session_id: String,
    pub path: Utf8PathBuf,
    pub outcome: Outcome,
    pub kind: LinkKind,
}

#[derive(Debug)]
pub struct Store<'a> {
    env: &'a Env,
}

impl<'a> Store<'a> {
    #[must_use]
    pub fn new(env: &'a Env) -> Self {
        Self { env }
    }

    #[must_use]
    pub fn transcripts_dir(&self) -> Utf8PathBuf {
        self.env.store_dir().join("transcripts")
    }

    #[must_use]
    pub fn path_for(&self, session_id: &str) -> Utf8PathBuf {
        self.transcripts_dir().join(format!("{session_id}.jsonl"))
    }

    /// Adds a transcript to the store, or confirms it is already there.
    pub fn ingest(&self, meta: &Meta) -> Result<Ingested> {
        let dst = self.path_for(&meta.session_id);
        let guard = self.env.guard();
        guard.check(&dst)?;
        std::fs::create_dir_all(self.transcripts_dir())
            .map_err(|e| Error::io(self.transcripts_dir(), e))?;

        if !dst.exists() {
            let kind = self.env.linker.link(&meta.path, &dst)?;
            return Ok(Ingested {
                session_id: meta.session_id.clone(),
                path: dst,
                outcome: Outcome::Linked,
                kind,
            });
        }

        if fsx::same_file(&meta.path, &dst) {
            return Ok(Ingested {
                session_id: meta.session_id.clone(),
                path: dst,
                outcome: Outcome::AlreadyLinked,
                kind: LinkKind::Hard,
            });
        }

        // Two distinct files for one session id. Transcripts are append only, so
        // if the stored bytes are a prefix of the live ones, the live file is the
        // same history further along and is safe to adopt.
        let stored_len = std::fs::metadata(&dst).map_err(|e| Error::io(&dst, e))?.len();
        let live_len = meta.size;
        if live_len >= stored_len
            && fsx::hash_prefix(&meta.path, stored_len)? == fsx::hash_prefix(&dst, stored_len)?
        {
            fsx::remove_file(&guard, &dst)?;
            let kind = self.env.linker.link(&meta.path, &dst)?;
            return Ok(Ingested {
                session_id: meta.session_id.clone(),
                path: dst,
                outcome: Outcome::Refreshed,
                kind,
            });
        }

        // Divergent histories. Keeping what we have is the half of the mistake
        // that loses nothing.
        Ok(Ingested {
            session_id: meta.session_id.clone(),
            path: dst,
            outcome: Outcome::Diverged,
            kind: LinkKind::Copy,
        })
    }

    #[must_use]
    pub fn holds(&self, session_id: &str) -> bool {
        self.path_for(session_id).exists()
    }

    /// Puts a transcript back into a project directory, for a session Claude's
    /// retention cleanup removed. Refuses to overwrite: if something is already
    /// there it is either the same file or a history we have no right to replace.
    pub fn restore_into(
        &self,
        session_id: &str,
        project_dir: &Utf8Path,
    ) -> Result<Option<Utf8PathBuf>> {
        let src = self.path_for(session_id);
        if !src.exists() {
            return Ok(None);
        }
        let dst = project_dir.join(format!("{session_id}.jsonl"));
        let guard = self.env.guard();
        guard.check(&dst)?;
        if dst.exists() {
            return Ok(None);
        }
        std::fs::create_dir_all(project_dir).map_err(|e| Error::io(project_dir, e))?;
        self.env.linker.link(&src, &dst)?;
        Ok(Some(dst))
    }

    /// How many links the stored transcript has. One means the project directory
    /// copy is gone and this is the only one left.
    #[must_use]
    pub fn is_last_copy(&self, session_id: &str) -> bool {
        fsx::link_count(&self.path_for(session_id)) == Some(1)
    }
}
