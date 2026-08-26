//! Capturing a snapshot.
//!
//! What goes in is an allow list, never a deny list. Enumerating what is
//! excluded means the day Claude adds a file holding a token, that file is in
//! the archive by default. Enumerating what is included means it never is.

use super::manifest::{Entry, EntryKind, MANIFEST_NAME, Manifest, SCHEMA, Scope};
use crate::claude::{identity, transcript};
use crate::env::Env;
use crate::error::{Error, Result};
use crate::fsx;
use camino::{Utf8Path, Utf8PathBuf};
use std::io::Read as _;

/// Anything Unsilo must never put in an archive, checked as a last line of
/// defence behind the allow list.
const NEVER: &[&str] = &[".credentials.json", ".claude.json", "cowork_settings.json"];

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Include file bodies. Without it the archive is a manifest of hashes,
    /// which restores visibility but not content.
    pub with_bodies: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { with_bodies: true }
    }
}

#[derive(Debug, Clone)]
pub struct Written {
    pub path: Utf8PathBuf,
    pub manifest: Manifest,
    pub archive_bytes: u64,
}

pub fn claude_snapshot(env: &Env, name: &str, options: Options) -> Result<Written> {
    let mut collected = Vec::new();
    for dir in &env.config_dirs {
        collect_transcripts(dir, &mut collected);
    }
    for root in &env.user_data {
        collect_desktop(root, &mut collected);
    }
    write(env, name, Scope::Claude, collected, options)
}

pub fn store_snapshot(env: &Env, name: &str, options: Options) -> Result<Written> {
    let mut collected = Vec::new();
    let root = &env.unsilo_home;
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(std::result::Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.path().to_path_buf()) else { continue };
        // Snapshots are not snapshotted, and neither is the write-ahead log.
        if path.starts_with(env.snapshots_dir())
            || path.extension().is_some_and(|e| e.starts_with("sqlite-"))
        {
            continue;
        }
        let archive_path = format!("store/{}", relative(root, &path));
        collected.push(Planned {
            archive_path,
            origin: path,
            kind: EntryKind::StoreFile,
            session_id: None,
            account: None,
            org: None,
        });
    }
    write(env, name, Scope::Store, collected, options)
}

#[derive(Debug, Clone)]
struct Planned {
    archive_path: String,
    origin: Utf8PathBuf,
    kind: EntryKind,
    session_id: Option<String>,
    account: Option<String>,
    org: Option<String>,
}

fn collect_transcripts(config_dir: &Utf8Path, out: &mut Vec<Planned>) {
    let projects = config_dir.join("projects");
    let Ok(dirs) = std::fs::read_dir(&projects) else { return };

    for project in dirs.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(project_dir) = Utf8PathBuf::from_path_buf(project.path()) else { continue };
        let Some(slug) = project_dir.file_name() else { continue };
        let Ok(files) = std::fs::read_dir(&project_dir) else { continue };

        for file in files.flatten() {
            let Ok(path) = Utf8PathBuf::from_path_buf(file.path()) else { continue };
            let Ok(kind) = file.file_type() else { continue };

            if kind.is_dir() {
                // Subagent transcripts belong to their conversation, so they
                // travel with it even though they are never listed as one.
                let subagents = path.join("subagents");
                let Some(session) = path.file_name() else { continue };
                let Ok(subs) = std::fs::read_dir(&subagents) else { continue };
                for sub in subs.flatten() {
                    let Ok(sub_path) = Utf8PathBuf::from_path_buf(sub.path()) else { continue };
                    if sub_path.extension() != Some("jsonl") {
                        continue;
                    }
                    let Some(name) = sub_path.file_name() else { continue };
                    out.push(Planned {
                        archive_path: format!("transcripts/{slug}/{session}/subagents/{name}"),
                        origin: sub_path,
                        kind: EntryKind::Subagent,
                        session_id: Some(session.to_owned()),
                        account: None,
                        org: None,
                    });
                }
                continue;
            }
            if !kind.is_file() || path.extension() != Some("jsonl") {
                continue;
            }
            let Some(stem) = path.file_stem().map(ToOwned::to_owned) else { continue };
            if !transcript::is_session_uuid(&stem) {
                continue;
            }
            out.push(Planned {
                archive_path: format!("transcripts/{slug}/{stem}.jsonl"),
                origin: path.clone(),
                kind: EntryKind::Transcript,
                session_id: Some(stem),
                account: None,
                org: None,
            });
        }
    }
}

fn collect_desktop(user_data: &Utf8Path, out: &mut Vec<Planned>) {
    for surface in ["claude-code-sessions", "local-agent-mode-sessions"] {
        let root = user_data.join(surface);
        let Ok(accounts) = std::fs::read_dir(&root) else { continue };

        for account_entry in accounts.flatten() {
            let account = account_entry.file_name().to_string_lossy().into_owned();
            if !crate::claude::is_uuid(&account) {
                continue;
            }
            let Ok(orgs) = std::fs::read_dir(account_entry.path()) else { continue };

            for org_entry in orgs.flatten() {
                let org = org_entry.file_name().to_string_lossy().into_owned();
                if !crate::claude::is_uuid(&org) {
                    continue;
                }
                let Ok(files) = std::fs::read_dir(org_entry.path()) else { continue };

                for file in files.flatten() {
                    let Ok(path) = Utf8PathBuf::from_path_buf(file.path()) else { continue };
                    let Some(name) = path.file_name() else { continue };
                    let kind = if name.starts_with("deleted_") {
                        EntryKind::Tombstone
                    } else if name.starts_with("local_") && path.extension() == Some("json") {
                        EntryKind::DesktopEntry
                    } else {
                        continue;
                    };
                    out.push(Planned {
                        archive_path: format!("desktop/{surface}/{account}/{org}/{name}"),
                        origin: path,
                        kind,
                        session_id: None,
                        account: Some(account.clone()),
                        org: Some(org.clone()),
                    });
                }
            }
        }
    }
}

fn write(
    env: &Env,
    name: &str,
    scope: Scope,
    planned: Vec<Planned>,
    options: Options,
) -> Result<Written> {
    if name.is_empty() || name.contains(['/', '\\', '.']) {
        return Err(Error::Usage(format!(
            "invalid snapshot name {name:?}: no slashes and no dots"
        )));
    }
    let dir = env.snapshots_dir();
    let path = dir.join(format!("{name}.tar.zst"));
    let guard = env.guard();
    guard.check(&path)?;
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;

    let identities = identity::Identities::load(&env.unsilo_home.join("identities.json"))?;
    let mut manifest = Manifest {
        schema: SCHEMA,
        scope,
        created_at_ms: env.clock.now_ms(),
        unsilo_version: crate::VERSION.to_owned(),
        active: identity::active(&env.home),
        accounts: identities.accounts.iter().map(|(k, v)| (k.clone(), v.name.clone())).collect(),
        orgs: identities.orgs.iter().map(|(k, v)| (k.clone(), v.name.clone())).collect(),
        has_bodies: options.with_bodies,
        entries: Vec::new(),
    };

    let file = std::fs::File::create(&path).map_err(|e| Error::io(&path, e))?;
    let encoder = zstd::Encoder::new(file, 9).map_err(|e| Error::io(&path, e))?.auto_finish();
    let mut archive = tar::Builder::new(encoder);

    let mut sorted = planned;
    // Deterministic order, so two snapshots of the same tree compare cleanly.
    sorted.sort_by(|a, b| a.archive_path.cmp(&b.archive_path));

    for item in sorted {
        if NEVER.iter().any(|deny| item.origin.file_name() == Some(*deny)) {
            return Err(Error::Usage(format!(
                "the collector proposed {}, which can never enter a snapshot",
                item.origin
            )));
        }
        let Ok(mut source) = std::fs::File::open(&item.origin) else { continue };
        // Length comes from the open handle: whatever is appended after this
        // point is simply not part of the snapshot.
        let len = source.metadata().map_err(|e| Error::io(&item.origin, e))?.len();
        let mut bytes = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
        source
            .by_ref()
            .take(len)
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io(&item.origin, e))?;

        manifest.entries.push(Entry {
            archive_path: item.archive_path.clone(),
            origin: item.origin.clone(),
            kind: item.kind,
            len: bytes.len() as u64,
            sha256: fsx::hash_bytes(&bytes),
            session_id: item.session_id,
            account: item.account,
            org: item.org,
        });

        if options.with_bodies {
            append(&mut archive, &item.archive_path, &bytes, &path)?;
        }
    }

    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| Error::json(&path, e))?;
    append(&mut archive, MANIFEST_NAME, &manifest_bytes, &path)?;
    archive.into_inner().map_err(|e| Error::io(&path, e))?;

    fsx::set_private(&path)?;
    let archive_bytes = std::fs::metadata(&path).map_err(|e| Error::io(&path, e))?.len();
    Ok(Written { path, manifest, archive_bytes })
}

fn append<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    name: &str,
    bytes: &[u8],
    context: &Utf8Path,
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(0);
    header.set_cksum();
    archive.append_data(&mut header, name, bytes).map_err(|e| Error::io(context, e))
}

fn relative(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root).unwrap_or(path).as_str().replace('\\', "/")
}
