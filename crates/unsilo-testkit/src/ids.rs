use sha2::{Digest, Sha256};

/// A UUID-shaped id derived from a readable name, so fixtures stay deterministic
/// across runs and platforms and snapshot output does not churn.
#[must_use]
pub fn uuid_for(name: &str) -> String {
    let d = Sha256::digest(name.as_bytes());
    let mut h = String::with_capacity(32);
    for b in d.iter().take(16) {
        use std::fmt::Write as _;
        let _ = write!(h, "{b:02x}");
    }
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Whether a fixture name is already in uuid form, in which case it is used as is.
#[must_use]
pub fn looks_like_uuid(s: &str) -> bool {
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
    use super::*;

    #[test]
    fn ids_are_uuid_shaped_and_stable() {
        let a = uuid_for("s-auth");
        assert_eq!(a.len(), 36);
        assert_eq!(a, uuid_for("s-auth"));
        assert_ne!(a, uuid_for("s-other"));
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
    }
}
