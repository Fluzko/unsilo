//! Filesystem primitives with the guarantees the rest of the code assumes.
//!
//! Two things live here that are easy to get wrong and expensive to get wrong
//! late: writes that either happen completely or not at all, and a guard that
//! refuses to write anywhere Unsilo has no business writing.

use crate::error::{Error, Result};
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};

/// Claude writes its own session files with owner-only permissions. Anything
/// looser would leak conversation titles and working directories.
#[cfg(unix)]
pub const PRIVATE_MODE: u32 = 0o600;

/// Resolves `.` and `..` without touching the filesystem, so a path that has
/// never existed can still be compared. Symlinks are deliberately not followed:
/// the guard is about where we intend to write, and resolving links would let a
/// planted link move the target after the check.
#[must_use]
pub fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_str()),
        }
    }
    out
}

/// The set of roots Unsilo may write under. Every write goes through it, so a
/// path escaping them is an error at runtime rather than a finding after the
/// fact.
#[derive(Debug, Clone)]
pub struct Guard {
    roots: Vec<Utf8PathBuf>,
}

impl Guard {
    #[must_use]
    pub fn new(roots: &[Utf8PathBuf]) -> Self {
        Self { roots: roots.iter().map(|r| normalize(r)).collect() }
    }

    pub fn check(&self, path: &Utf8Path) -> Result<()> {
        let target = normalize(path);
        if self.roots.iter().any(|root| target.starts_with(root)) {
            return Ok(());
        }
        Err(Error::Usage(format!("unsilo refuses to write outside its own roots: {target}")))
    }
}

/// Writes through a temporary file in the same directory, then renames.
///
/// Same directory matters: a rename across filesystems is not atomic and would
/// fall back to a copy, which is exactly the partial write this avoids.
pub fn write_atomic(guard: &Guard, path: &Utf8Path, bytes: &[u8]) -> Result<()> {
    guard.check(path)?;
    let parent =
        path.parent().ok_or_else(|| Error::Usage(format!("{path} has no parent directory")))?;
    std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;

    let name = path.file_name().unwrap_or("unsilo");
    let tmp = parent.join(format!(".{name}.unsilo.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| Error::io(&tmp, e))?;
    set_private(&tmp)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(Error::io(path, e))
        }
    }
}

pub fn set_private(path: &Utf8Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_MODE))
            .map_err(|e| Error::io(path, e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn remove_file(guard: &Guard, path: &Utf8Path) -> Result<()> {
    guard.check(path)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// Hash of the first `len` bytes. A hash without its length says nothing about a
/// file that is still growing, so the two always travel together.
pub fn hash_prefix(path: &Utf8Path, len: u64) -> Result<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = Sha256::new();
    let mut remaining = len;
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(buf.len());
        let Some(slot) = buf.get_mut(..want) else { break };
        let read = file.read(slot).map_err(|e| Error::io(path, e))?;
        let Some(filled) = buf.get(..read) else { break };
        if read == 0 {
            break;
        }
        hasher.update(filled);
        remaining -= read as u64;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Volume and file identity, stable across renames. Reconciling by path breaks
/// the moment Claude renames an orphaned transcript out from under us.
#[cfg(unix)]
#[must_use]
pub fn file_id(path: &Utf8Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// `std`'s `MetadataExt::volume_serial_number` and `file_index` are still
/// unstable, so this goes through the Win32 call they wrap. The 64-bit index is
/// the classic NTFS file id: enough to recognise two names for one file, which
/// is all this is used for.
/// One Win32 call behind both the identity and the link count, so there is a
/// single place that touches unsafe.
#[cfg(windows)]
#[allow(unsafe_code)]
fn by_handle_info(
    path: &Utf8Path,
) -> Option<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let file = std::fs::File::open(path).ok()?;
    // SAFETY: every field is a plain integer or FILETIME, so an all-zero value is
    // a valid instance and the call overwrites it before it is read.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // Bound to the expected type rather than cast, so a mismatch is a compile
    // error here instead of a pointer coercion nobody notices.
    let handle: HANDLE = file.as_raw_handle();
    // SAFETY: `handle` is owned by `file`, which outlives the call, and `info` is
    // a correctly sized out parameter.
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    if ok == 0 {
        return None;
    }
    Some(info)
}

#[cfg(windows)]
#[must_use]
pub fn file_id(path: &Utf8Path) -> Option<(u64, u64)> {
    let info = by_handle_info(path)?;
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(not(any(unix, windows)))]
#[must_use]
pub fn file_id(_path: &Utf8Path) -> Option<(u64, u64)> {
    None
}

/// How many names the file has. One means this is the only copy left, which is
/// exactly the state Claude's retention cleanup leaves the store in.
#[cfg(unix)]
#[must_use]
pub fn link_count(path: &Utf8Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(std::fs::metadata(path).ok()?.nlink())
}

#[cfg(windows)]
#[must_use]
pub fn link_count(path: &Utf8Path) -> Option<u64> {
    Some(u64::from(by_handle_info(path)?.nNumberOfLinks))
}

#[cfg(not(any(unix, windows)))]
#[must_use]
pub fn link_count(_path: &Utf8Path) -> Option<u64> {
    None
}

#[must_use]
pub fn same_file(a: &Utf8Path, b: &Utf8Path) -> bool {
    match (file_id(a), file_id(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn guard() -> Guard {
        Guard::new(&["/home/u/.claude".into(), "/home/u/.unsilo".into()])
    }

    #[test]
    fn normalization_resolves_dots_without_touching_disk() {
        assert_eq!(normalize("/a/./b".into()), Utf8PathBuf::from("/a/b"));
        assert_eq!(normalize("/a/b/../c".into()), Utf8PathBuf::from("/a/c"));
        assert_eq!(normalize("/a/b/../../c".into()), Utf8PathBuf::from("/c"));
    }

    #[test]
    fn writes_inside_a_root_are_allowed() {
        assert!(guard().check("/home/u/.claude/projects/x/y.jsonl".into()).is_ok());
        assert!(guard().check("/home/u/.unsilo/store/a.jsonl".into()).is_ok());
    }

    #[test]
    fn writes_outside_every_root_are_refused() {
        let err = guard().check("/home/u/Documents/taxes.pdf".into()).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("outside its own roots"));
    }

    #[test]
    fn traversal_cannot_walk_out_of_a_root() {
        // The check runs on the normalized path, so ../.. does not sneak past it.
        assert!(guard().check("/home/u/.claude/../../etc/passwd".into()).is_err());
        assert!(guard().check("/home/u/.claude/projects/../../.ssh/id_rsa".into()).is_err());
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_inside_the_root() {
        // "/home/u/.claude-backup" starts with "/home/u/.claude" as text.
        assert!(guard().check("/home/u/.claude-backup/x".into()).is_err());
    }

    #[test]
    fn an_atomic_write_leaves_no_temporary_behind() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let guard = Guard::new(std::slice::from_ref(&root));
        let target = root.join("nested").join("file.json");

        write_atomic(&guard, &target, b"hello").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");

        let leftovers: Vec<String> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn written_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let guard = Guard::new(std::slice::from_ref(&root));
        let target = root.join("secret.json");

        write_atomic(&guard, &target, b"{}").unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, PRIVATE_MODE);
    }

    #[test]
    fn a_prefix_hash_ignores_bytes_appended_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let path = root.join("t.jsonl");
        std::fs::write(&path, b"line one\n").unwrap();

        let len = 9;
        let before = hash_prefix(&path, len).unwrap();
        std::fs::write(&path, b"line one\nline two\n").unwrap();
        assert_eq!(hash_prefix(&path, len).unwrap(), before, "the prefix is frozen");
        assert_ne!(hash_prefix(&path, 18).unwrap(), before);
    }

    #[test]
    fn hard_linked_paths_share_an_identity_and_distinct_files_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let a = root.join("a");
        let b = root.join("b");
        let c = root.join("c");
        std::fs::write(&a, b"x").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        std::fs::write(&c, b"x").unwrap();

        assert!(same_file(&a, &b));
        assert!(!same_file(&a, &c), "identical bytes are not the same file");
    }

    #[test]
    fn removing_something_already_gone_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).unwrap();
        let guard = Guard::new(std::slice::from_ref(&root));
        assert!(remove_file(&guard, &root.join("missing")).is_ok());
    }

    #[test]
    fn removal_respects_the_same_roots_as_writing() {
        assert!(remove_file(&guard(), "/etc/passwd".into()).is_err());
    }
}
