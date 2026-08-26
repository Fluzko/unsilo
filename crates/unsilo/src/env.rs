//! Every root Unsilo touches, plus the three seams that make the rest testable.
//!
//! Nothing outside this module reads the process environment or the user's home.
//! Modules receive an [`Env`] and stay pure with respect to the machine they run on,
//! which is what lets the whole test suite run against a temp directory.

use crate::error::{Error, Result};
use camino::{Utf8Path, Utf8PathBuf};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

pub const VAR_HOME: &str = "HOME";
pub const VAR_USERPROFILE: &str = "USERPROFILE";
pub const VAR_APPDATA: &str = "APPDATA";
pub const VAR_LOCALAPPDATA: &str = "LOCALAPPDATA";
pub const VAR_CLAUDE_CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";
pub const VAR_UNSILO_HOME: &str = "UNSILO_HOME";
pub const VAR_UNSILO_USER_DATA: &str = "UNSILO_DESKTOP_USER_DATA";
pub const VAR_UNSILO_FAILPOINT: &str = "UNSILO_FAILPOINT";

pub trait VarSource: fmt::Debug {
    fn get(&self, key: &str) -> Option<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemVars;

impl VarSource for SystemVars {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
}

#[derive(Debug, Default, Clone)]
pub struct MapVars(BTreeMap<String, String>);

impl MapVars {
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    #[must_use]
    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }
}

impl VarSource for MapVars {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned().filter(|v| !v.is_empty())
    }
}

pub trait Clock: fmt::Debug + Send + Sync {
    /// Milliseconds since the Unix epoch. The desktop index stores epoch millis,
    /// so this is the unit that avoids conversions at every call site.
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    /// Same inode as the original: appends by Claude are visible through both paths.
    Hard,
    /// Independent bytes: goes stale as the session grows, needs re-syncing.
    Copy,
}

pub trait Linker: fmt::Debug + Send + Sync {
    fn link(&self, src: &Utf8Path, dst: &Utf8Path) -> Result<LinkKind>;
}

/// Hard link when the filesystem allows it, copy when it does not.
/// Never moves: the original stays exactly where Claude expects to find it.
#[derive(Debug, Default, Clone, Copy)]
pub struct HardLinkFirst;

impl Linker for HardLinkFirst {
    fn link(&self, src: &Utf8Path, dst: &Utf8Path) -> Result<LinkKind> {
        match std::fs::hard_link(src, dst) {
            Ok(()) => Ok(LinkKind::Hard),
            Err(e) if is_unsupported(&e) => {
                std::fs::copy(src, dst).map_err(|e| Error::io(dst, e))?;
                Ok(LinkKind::Copy)
            }
            Err(e) => Err(Error::io(dst, e)),
        }
    }
}

/// Forces the degraded path so CI can exercise it without a FAT or network volume.
#[derive(Debug, Default, Clone, Copy)]
pub struct AlwaysCopy;

impl Linker for AlwaysCopy {
    fn link(&self, src: &Utf8Path, dst: &Utf8Path) -> Result<LinkKind> {
        std::fs::copy(src, dst).map_err(|e| Error::io(dst, e))?;
        Ok(LinkKind::Copy)
    }
}

fn is_unsupported(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        e.kind(),
        ErrorKind::Unsupported | ErrorKind::CrossesDevices | ErrorKind::PermissionDenied
    )
}

pub trait Failpoints: fmt::Debug + Send + Sync {
    fn hit(&self, name: &str) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoFailpoints;

impl Failpoints for NoFailpoints {
    fn hit(&self, _name: &str) -> Result<()> {
        Ok(())
    }
}

/// Aborts at a named point so a crash mid-apply can be reproduced deterministically.
#[derive(Debug, Clone)]
pub struct NamedFailpoint(pub String);

impl Failpoints for NamedFailpoint {
    fn hit(&self, name: &str) -> Result<()> {
        if self.0 == name {
            return Err(Error::Failpoint(name.to_owned()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Env {
    pub home: Utf8PathBuf,
    /// Every Claude CLI config dir found, in probe order. May be empty.
    pub config_dirs: Vec<Utf8PathBuf>,
    /// Every desktop userData root found, in probe order. May be empty.
    pub user_data: Vec<Utf8PathBuf>,
    pub unsilo_home: Utf8PathBuf,
    pub clock: Arc<dyn Clock>,
    pub linker: Arc<dyn Linker>,
    pub failpoints: Arc<dyn Failpoints>,
}

impl Env {
    pub fn discover() -> Result<Self> {
        Self::from_vars(&SystemVars, &RealProbe)
    }

    pub fn from_vars(vars: &dyn VarSource, probe: &dyn Probe) -> Result<Self> {
        let home: Utf8PathBuf =
            vars.get(VAR_HOME).or_else(|| vars.get(VAR_USERPROFILE)).ok_or(Error::NoHome)?.into();

        let mut config_dirs = Vec::new();
        if let Some(v) = vars.get(VAR_CLAUDE_CONFIG_DIR) {
            config_dirs.push(Utf8PathBuf::from(v));
        }
        let default_config = home.join(".claude");
        if !config_dirs.contains(&default_config) {
            config_dirs.push(default_config);
        }
        config_dirs.retain(|d| probe.is_dir(&d.join("projects")));

        let user_data = match vars.get(VAR_UNSILO_USER_DATA) {
            Some(v) => vec![Utf8PathBuf::from(v)],
            None => desktop_candidates(&home, vars),
        }
        .into_iter()
        .filter(|d| probe.is_dir(&d.join("claude-code-sessions")))
        .collect();

        let unsilo_home =
            vars.get(VAR_UNSILO_HOME).map_or_else(|| home.join(".unsilo"), Utf8PathBuf::from);

        let failpoints: Arc<dyn Failpoints> = match vars.get(VAR_UNSILO_FAILPOINT) {
            Some(name) => Arc::new(NamedFailpoint(name)),
            None => Arc::new(NoFailpoints),
        };

        Ok(Self {
            home,
            config_dirs,
            user_data,
            unsilo_home,
            clock: Arc::new(SystemClock),
            linker: Arc::new(HardLinkFirst),
            failpoints,
        })
    }

    #[must_use]
    pub fn store_dir(&self) -> Utf8PathBuf {
        self.unsilo_home.join("store")
    }

    #[must_use]
    pub fn index_path(&self) -> Utf8PathBuf {
        self.unsilo_home.join("index.sqlite")
    }

    #[must_use]
    pub fn snapshots_dir(&self) -> Utf8PathBuf {
        self.unsilo_home.join("snapshots")
    }

    /// Roots Unsilo is allowed to write under. Anything else is a bug, and the
    /// test harness asserts on exactly this list.
    #[must_use]
    pub fn writable_roots(&self) -> Vec<Utf8PathBuf> {
        let mut roots = vec![self.unsilo_home.clone()];
        roots.extend(self.config_dirs.iter().cloned());
        roots.extend(self.user_data.iter().cloned());
        roots
    }

    /// The guard every write must pass. Built from the same roots the tests
    /// assert on, so there is one definition of "allowed to write here".
    #[must_use]
    pub fn guard(&self) -> crate::fsx::Guard {
        crate::fsx::Guard::new(&self.writable_roots())
    }

    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    #[must_use]
    pub fn with_linker(mut self, linker: Arc<dyn Linker>) -> Self {
        self.linker = linker;
        self
    }
}

/// The desktop app writes under different roots per platform, and on Windows it
/// uses two of them in different code paths, so both get probed.
fn desktop_candidates(home: &Utf8Path, vars: &dyn VarSource) -> Vec<Utf8PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push(home.join("Library/Application Support/Claude"));
    }
    if cfg!(target_os = "linux") {
        out.push(home.join(".config/Claude"));
    }
    if cfg!(target_os = "windows") {
        for var in [VAR_APPDATA, VAR_LOCALAPPDATA] {
            if let Some(v) = vars.get(var) {
                out.push(Utf8PathBuf::from(v).join("Claude"));
            }
        }
    }
    out
}

pub trait Probe: fmt::Debug {
    fn is_dir(&self, path: &Utf8Path) -> bool;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RealProbe;

impl Probe for RealProbe {
    fn is_dir(&self, path: &Utf8Path) -> bool {
        path.is_dir()
    }
}

#[derive(Debug, Default, Clone)]
pub struct FakeProbe(pub Vec<Utf8PathBuf>);

impl Probe for FakeProbe {
    fn is_dir(&self, path: &Utf8Path) -> bool {
        self.0.iter().any(|p| p == path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn probe(paths: &[&str]) -> FakeProbe {
        FakeProbe(paths.iter().map(|p| Utf8PathBuf::from(*p)).collect())
    }

    #[test]
    fn falls_back_to_the_default_config_dir() {
        let vars = MapVars::new().with(VAR_HOME, "/h");
        let env = Env::from_vars(&vars, &probe(&["/h/.claude/projects"])).unwrap();
        assert_eq!(env.config_dirs, vec![Utf8PathBuf::from("/h/.claude")]);
        assert_eq!(env.unsilo_home, Utf8PathBuf::from("/h/.unsilo"));
    }

    #[test]
    fn claude_config_dir_takes_precedence_and_the_default_still_counts() {
        let vars = MapVars::new().with(VAR_HOME, "/h").with(VAR_CLAUDE_CONFIG_DIR, "/alt");
        let env = Env::from_vars(&vars, &probe(&["/alt/projects", "/h/.claude/projects"])).unwrap();
        assert_eq!(env.config_dirs, ["/alt", "/h/.claude"].map(Utf8PathBuf::from));
    }

    #[test]
    fn config_dirs_without_a_projects_dir_are_dropped() {
        let vars = MapVars::new().with(VAR_HOME, "/h").with(VAR_CLAUDE_CONFIG_DIR, "/alt");
        let env = Env::from_vars(&vars, &probe(&["/h/.claude/projects"])).unwrap();
        assert_eq!(env.config_dirs, vec![Utf8PathBuf::from("/h/.claude")]);
    }

    #[test]
    fn a_missing_home_is_an_error_not_a_guess() {
        let err = Env::from_vars(&MapVars::new(), &probe(&[])).unwrap_err();
        assert!(matches!(err, Error::NoHome));
    }

    #[test]
    fn user_data_can_be_overridden_for_tests() {
        let vars = MapVars::new().with(VAR_HOME, "/h").with(VAR_UNSILO_USER_DATA, "/ud");
        let env = Env::from_vars(&vars, &probe(&["/ud/claude-code-sessions"])).unwrap();
        assert_eq!(env.user_data, vec![Utf8PathBuf::from("/ud")]);
    }

    #[test]
    fn writable_roots_cover_every_root_and_nothing_else() {
        let vars = MapVars::new()
            .with(VAR_HOME, "/h")
            .with(VAR_UNSILO_USER_DATA, "/ud")
            .with(VAR_UNSILO_HOME, "/u");
        let env =
            Env::from_vars(&vars, &probe(&["/h/.claude/projects", "/ud/claude-code-sessions"]))
                .unwrap();
        let roots = env.writable_roots();
        assert_eq!(roots, ["/u", "/h/.claude", "/ud"].map(Utf8PathBuf::from));
    }

    #[test]
    fn a_failpoint_only_fires_for_its_own_name() {
        let fp = NamedFailpoint("apply.after_ledger_pending".to_owned());
        assert!(fp.hit("apply.before_write").is_ok());
        assert!(fp.hit("apply.after_ledger_pending").is_err());
    }

    #[test]
    fn empty_env_vars_are_treated_as_unset() {
        let vars = MapVars::new().with(VAR_HOME, "/h").with(VAR_UNSILO_HOME, "");
        let env = Env::from_vars(&vars, &probe(&[])).unwrap();
        assert_eq!(env.unsilo_home, Utf8PathBuf::from("/h/.unsilo"));
    }
}
