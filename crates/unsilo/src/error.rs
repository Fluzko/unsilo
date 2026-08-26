use camino::Utf8PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Exit codes are part of the CLI contract: scripts branch on them.
/// Adding a variant means deciding which code it maps to, on purpose.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not determine the user home directory")]
    NoHome,

    #[error("unrecognised Claude layout: {0}")]
    UnknownLayout(String),

    #[error("{0} pending changes")]
    DryRunPending(usize),

    #[error("no results for the given filters")]
    NoMatches,

    #[error("invalid usage: {0}")]
    Usage(String),

    #[error("failpoint fired: {0}")]
    Failpoint(String),

    #[error("{path}: {source}")]
    Io {
        path: Utf8PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    BareIo(#[from] std::io::Error),

    #[error("index: {0}")]
    Index(#[from] rusqlite::Error),

    #[error("{path}: invalid json: {source}")]
    Json {
        path: Utf8PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Usage(_) => 2,
            Error::UnknownLayout(_) => 3,
            Error::DryRunPending(_) => 4,
            Error::NoMatches => 5,
            _ => 1,
        }
    }

    pub fn io(path: impl Into<Utf8PathBuf>, source: std::io::Error) -> Self {
        Error::Io { path: path.into(), source }
    }

    pub fn json(path: impl Into<Utf8PathBuf>, source: serde_json::Error) -> Self {
        Error::Json { path: path.into(), source }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_match_the_documented_contract() {
        assert_eq!(Error::Usage(String::new()).exit_code(), 2);
        assert_eq!(Error::UnknownLayout(String::new()).exit_code(), 3);
        assert_eq!(Error::DryRunPending(1).exit_code(), 4);
        assert_eq!(Error::NoMatches.exit_code(), 5);
        assert_eq!(Error::NoHome.exit_code(), 1);
    }

    #[test]
    fn io_errors_carry_the_path_that_failed() {
        let e = Error::io("/tmp/x", std::io::Error::other("boom"));
        assert!(e.to_string().contains("/tmp/x"), "{e}");
    }
}
