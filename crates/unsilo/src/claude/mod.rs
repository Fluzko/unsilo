//! Readers for Claude Code's on-disk formats.
//!
//! Everything here documents what was observed and in which version, because
//! that knowledge goes stale: the layout is not a published contract.

pub mod desktop;
pub mod identity;
pub mod layout;
pub mod slug;
pub mod time;
pub mod transcript;

/// Canonical 8-4-4-4-12 hex form. Account, organization and session directories
/// are all named this way; anything else is a sentinel directory belonging to
/// some other feature and is not ours to interpret.
#[must_use]
pub fn is_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::is_uuid;

    #[test]
    fn only_canonical_uuids_pass() {
        assert!(is_uuid("1e3fc9c4-ff2f-4fd5-8b42-5f10d0547d12"));
        assert!(!is_uuid("skills-plugin"));
        assert!(!is_uuid("1e3fc9c4_ff2f_4fd5_8b42_5f10d0547d12"));
        assert!(!is_uuid(""));
    }
}
