//! Project directory naming, as observed in Claude Code CLI 2.1.241.
//!
//! `projects/<slug>/<sessionId>.jsonl`, where the slug is derived from the cwd:
//!
//! ```js
//! cwd.replace(/[^a-zA-Z0-9]/g, "-")            // then, if longer than 200:
//! slug.slice(0, 200) + "-" + hash(cwd)
//! ```
//!
//! `replace` and `.length` in JavaScript operate on UTF-16 code units, not bytes
//! and not characters, so an emoji contributes two dashes. The hash used past 200
//! units is not reproducible from outside, so this module refuses instead of
//! guessing: callers record the directory a transcript was found in and reuse it.

const MAX_UNITS: usize = 200;

/// `None` when the cwd is long enough that Claude appends its own hash, which
/// cannot be recomputed here. Callers must fall back to a recorded directory.
#[must_use]
pub fn slug(cwd: &str) -> Option<String> {
    let units: Vec<u16> = cwd.encode_utf16().collect();
    if units.len() > MAX_UNITS {
        return None;
    }
    let mapped: Vec<u16> = units.into_iter().map(map_unit).collect();
    String::from_utf16(&mapped).ok()
}

/// The slug for a cwd that is short enough, or a stable stand-in for anything
/// longer. Only for building fixtures and diagnostics, never for locating a
/// transcript Claude wrote.
#[must_use]
pub fn slug_lossy(cwd: &str) -> String {
    slug(cwd).unwrap_or_else(|| {
        let mapped: Vec<u16> = cwd.encode_utf16().take(MAX_UNITS).map(map_unit).collect();
        String::from_utf16(&mapped).unwrap_or_default()
    })
}

fn map_unit(u: u16) -> u16 {
    let alnum = matches!(u, 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A);
    if alnum { u } else { b'-'.into() }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn separators_and_punctuation_become_dashes() {
        assert_eq!(slug("/Users/jdoe/code").as_deref(), Some("-Users-jdoe-code"));
    }

    #[test]
    fn windows_paths_lose_the_drive_colon_and_backslashes() {
        assert_eq!(slug(r"C:\Users\me\proj").as_deref(), Some("C--Users-me-proj"));
    }

    #[test]
    fn digits_survive_and_underscores_do_not() {
        assert_eq!(slug("a_1-b").as_deref(), Some("a-1-b"));
    }

    #[test]
    fn a_non_ascii_char_is_one_utf16_unit_so_one_dash() {
        assert_eq!(slug("/café").as_deref(), Some("-caf-"));
    }

    #[test]
    fn an_emoji_is_a_surrogate_pair_so_two_dashes() {
        // The trap: chars() would produce one dash and diverge from Claude.
        assert_eq!(slug("/a🎉b").as_deref(), Some("-a--b"));
    }

    #[test]
    fn exactly_two_hundred_units_still_resolves() {
        let cwd = "a".repeat(MAX_UNITS);
        assert_eq!(slug(&cwd).as_deref(), Some(cwd.as_str()));
    }

    #[test]
    fn past_two_hundred_units_we_refuse_rather_than_guess() {
        let cwd = "a".repeat(MAX_UNITS + 1);
        assert!(slug(&cwd).is_none());
        assert_eq!(slug_lossy(&cwd).len(), MAX_UNITS);
    }

    #[test]
    fn the_length_limit_counts_utf16_units_not_chars() {
        // 150 emoji are 150 chars but 300 UTF-16 units, so this is over the limit.
        let cwd = "🎉".repeat(150);
        assert_eq!(cwd.chars().count(), 150);
        assert!(slug(&cwd).is_none());
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert_eq!(slug("").as_deref(), Some(""));
    }
}
