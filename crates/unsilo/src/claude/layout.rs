//! Deciding whether the Claude installation in front of us is one we understand.
//!
//! None of this is a published contract, so it will change. Writing blind into a
//! layout we do not recognise is exactly how one program corrupts another's data.
//! Read-only commands keep working on anything; the writing ones refuse.

use crate::claude::desktop::Surface;
use std::collections::BTreeMap;
use std::fmt;

/// The newest CLI whose on-disk layout was actually read and verified.
pub const VERIFIED_CLI: Version = Version { major: 2, minor: 1, patch: 241 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split('.').map(|p| p.trim().parse::<u32>().ok());
        Some(Self {
            major: parts.next()??,
            minor: parts.next().flatten().unwrap_or(0),
            patch: parts.next().flatten().unwrap_or(0),
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compat {
    /// Verified layout. Everything is allowed.
    Known,
    /// Newer than what was verified, but the same shape. Reads and writes are
    /// allowed with a warning: refusing on every patch release would make the
    /// tool useless within a week.
    Unverified(String),
    /// Reads only. `apply`, `off` and `restore` exit 3.
    Refuses(String),
}

impl Compat {
    #[must_use]
    pub fn allows_writes(&self) -> bool {
        !matches!(self, Compat::Refuses(_))
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Compat::Known => None,
            Compat::Unverified(r) | Compat::Refuses(r) => Some(r),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Fingerprint {
    /// CLI version string to how many transcripts carried it.
    pub cli_versions: BTreeMap<String, usize>,
    pub newest_cli: Option<Version>,
    pub surfaces: Vec<Surface>,
    /// `None` when the flag could not be read at all.
    pub remote_backend: Option<bool>,
    pub has_transcripts: bool,
    pub has_desktop_index: bool,
}

impl Fingerprint {
    #[must_use]
    pub fn compat(&self) -> Compat {
        if self.remote_backend == Some(true) {
            return Compat::Refuses(
                "the remote transcript backend is on (tengu_hover_rest); \
                 transcripts are no longer plain files"
                    .to_owned(),
            );
        }
        match self.newest_cli {
            Some(v) if v.major != VERIFIED_CLI.major => Compat::Refuses(format!(
                "cli {v} has a different major version than the verified one ({VERIFIED_CLI})"
            )),
            Some(v) if v > VERIFIED_CLI => Compat::Unverified(format!(
                "cli {v} is newer than the verified one ({VERIFIED_CLI})"
            )),
            None if self.has_transcripts => {
                Compat::Unverified("no transcript declares its cli version".to_owned())
            }
            _ => Compat::Known,
        }
    }

    /// The versions seen, most common first. Ties break on the version so the
    /// output is stable between runs.
    #[must_use]
    pub fn versions_by_frequency(&self) -> Vec<(&str, usize)> {
        let mut out: Vec<(&str, usize)> =
            self.cli_versions.iter().map(|(v, n)| (v.as_str(), *n)).collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn fp(version: Option<&str>) -> Fingerprint {
        let mut f = Fingerprint {
            remote_backend: Some(false),
            has_transcripts: true,
            ..Default::default()
        };
        if let Some(v) = version {
            f.cli_versions.insert(v.to_owned(), 1);
            f.newest_cli = Version::parse(v);
        }
        f
    }

    #[test]
    fn versions_parse_leniently_but_not_wrongly() {
        assert_eq!(Version::parse("2.1.241"), Some(Version { major: 2, minor: 1, patch: 241 }));
        assert_eq!(Version::parse("2.1"), Some(Version { major: 2, minor: 1, patch: 0 }));
        assert_eq!(Version::parse("2"), Some(Version { major: 2, minor: 0, patch: 0 }));
        assert_eq!(Version::parse(""), None);
        assert_eq!(Version::parse("next"), None);
    }

    #[test]
    fn versions_order_by_component_not_by_string() {
        // The trap: "2.1.9" sorts after "2.1.241" as text.
        assert!(Version::parse("2.1.241").unwrap() > Version::parse("2.1.9").unwrap());
        assert!(Version::parse("2.2.0").unwrap() > Version::parse("2.1.999").unwrap());
    }

    #[test]
    fn the_verified_version_is_fully_allowed() {
        assert_eq!(fp(Some("2.1.241")).compat(), Compat::Known);
        assert_eq!(fp(Some("2.1.100")).compat(), Compat::Known);
    }

    #[test]
    fn a_newer_patch_warns_but_still_writes() {
        let compat = fp(Some("2.1.999")).compat();
        assert!(compat.allows_writes());
        assert!(matches!(compat, Compat::Unverified(_)));
    }

    #[test]
    fn a_different_major_stops_writing() {
        let compat = fp(Some("3.0.0")).compat();
        assert!(!compat.allows_writes());
        assert!(compat.reason().is_some_and(|r| r.contains("major")));
    }

    #[test]
    fn the_remote_backend_stops_writing_regardless_of_version() {
        let mut f = fp(Some("2.1.241"));
        f.remote_backend = Some(true);
        let compat = f.compat();
        assert!(!compat.allows_writes());
        assert!(compat.reason().is_some_and(|r| r.contains("tengu_hover_rest")));
    }

    #[test]
    fn transcripts_with_no_version_are_flagged_but_not_blocked() {
        let compat = fp(None).compat();
        assert!(compat.allows_writes());
        assert!(matches!(compat, Compat::Unverified(_)));
    }

    #[test]
    fn an_empty_installation_is_not_an_error() {
        let f = Fingerprint { remote_backend: Some(false), ..Default::default() };
        assert_eq!(f.compat(), Compat::Known);
    }

    #[test]
    fn versions_are_ranked_by_frequency_then_stably() {
        let mut f = fp(None);
        f.cli_versions.insert("2.1.220".to_owned(), 49);
        f.cli_versions.insert("2.1.237".to_owned(), 20);
        f.cli_versions.insert("2.1.228".to_owned(), 20);
        assert_eq!(
            f.versions_by_frequency(),
            vec![("2.1.220", 49), ("2.1.228", 20), ("2.1.237", 20)]
        );
    }
}
