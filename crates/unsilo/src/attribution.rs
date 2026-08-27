//! Working out which account a CLI-born conversation belongs to.
//!
//! A transcript records no account: not the uuid, not the organization, not the
//! email. The association exists only in the desktop's index, and only for
//! sessions the desktop created. For everything else it has to be inferred, and
//! the only honest basis is time: remember which account was signed in when, and
//! compare that against when a conversation was active.
//!
//! Two properties this keeps. An inference is never stored as a fact, so a filter
//! can tell the difference. And when the sightings around a conversation
//! disagree, it stays unattributed rather than being assigned to whichever
//! account happened to be nearest.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Seen signed in while unsilo was running.
    Observed,
    /// Recovered from a rotating copy of Claude's config.
    Backup,
    /// Taken from the timestamps of a desktop entry, which names its account.
    Entry,
}

impl Source {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Observed => "observed",
            Source::Backup => "backup",
            Source::Entry => "entry",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "observed" => Some(Source::Observed),
            "backup" => Some(Source::Backup),
            "entry" => Some(Source::Entry),
            _ => None,
        }
    }
}

/// One moment at which a given account was known to be signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sighting {
    pub account: String,
    pub org: String,
    pub at_ms: i64,
    pub source: Source,
}

/// What a conversation's own timestamps say about when it existed.
#[derive(Debug, Clone, Copy)]
pub struct Activity {
    pub created_at_ms: Option<i64>,
    pub modified_at_ms: Option<i64>,
}

impl Activity {
    /// The instant a conversation is attributed by: when it started.
    ///
    /// A conversation belongs to whoever began it. Asking about its whole
    /// lifetime instead makes anything long-running unattributable, and answers a
    /// question nobody asked: a conversation started under one account and later
    /// continued under another is still that first account's.
    fn started_at(self) -> Option<i64> {
        self.created_at_ms.or(self.modified_at_ms)
    }
}

/// The account a conversation can be attributed to, or `None`.
///
/// Two kinds of evidence count, and nothing else. A sighting at the moment it
/// started is direct. Failing that, the nearest sighting before and the nearest
/// after must name the same account, which brackets the moment: the account
/// cannot have changed and changed back without a sighting in between saying so.
///
/// A single sighting before is deliberately not enough. Unsilo only observes when
/// it runs, so a week-old sighting says nothing about today: the account could
/// have changed the day after and nobody would have seen it. Extrapolating there
/// produces confident nonsense.
#[must_use]
pub fn infer(activity: Activity, sightings: &[Sighting]) -> Option<(String, String)> {
    let started = activity.started_at()?;

    let at: Vec<&Sighting> = sightings.iter().filter(|s| s.at_ms == started).collect();
    if !at.is_empty() {
        return agree(&at);
    }

    let before = sightings.iter().filter(|s| s.at_ms < started).max_by_key(|s| s.at_ms)?;
    let after = sightings.iter().filter(|s| s.at_ms > started).min_by_key(|s| s.at_ms)?;
    agree(&[before, after])
}

/// The account the given sightings unanimously name, if they do. Disagreement is
/// not resolved: guessing would let a filter prune or project conversations on
/// the strength of a coincidence.
fn agree(sightings: &[&Sighting]) -> Option<(String, String)> {
    let first = sightings.first()?;
    if sightings.iter().any(|s| s.account != first.account) {
        return None;
    }
    let org = if sightings.iter().all(|s| s.org == first.org) {
        first.org.clone()
    } else {
        // The account is settled even when the organization moved under it.
        String::new()
    };
    Some((first.account.clone(), org))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn sighting(account: &str, at_ms: i64) -> Sighting {
        Sighting {
            account: account.to_owned(),
            org: format!("org-of-{account}"),
            at_ms,
            source: Source::Observed,
        }
    }

    fn alive(from: i64, to: i64) -> Activity {
        Activity { created_at_ms: Some(from), modified_at_ms: Some(to) }
    }

    fn started(at: i64) -> Activity {
        Activity { created_at_ms: Some(at), modified_at_ms: Some(at + 10) }
    }

    #[test]
    fn a_conversation_bracketed_by_one_account_is_attributed() {
        let seen = [sighting("a", 100), sighting("a", 200)];
        assert_eq!(infer(started(120), &seen).unwrap().0, "a");
    }

    #[test]
    fn a_single_sighting_before_it_is_not_enough() {
        // Unsilo only sees when it runs. One observation beforehand cannot rule
        // out an unobserved switch in between.
        let seen = [sighting("a", 100)];
        assert_eq!(infer(started(200), &seen), None);
    }

    #[test]
    fn a_bracketing_pair_that_disagrees_is_not_resolved() {
        let seen = [sighting("a", 100), sighting("b", 400)];
        assert_eq!(infer(started(200), &seen), None);
    }

    #[test]
    fn a_sighting_at_the_exact_moment_is_direct_evidence() {
        let seen = [sighting("b", 100), sighting("a", 200), sighting("b", 400)];
        assert_eq!(infer(started(200), &seen).unwrap().0, "a");
    }

    #[test]
    fn a_conversation_that_outlives_the_account_that_started_it_keeps_its_owner() {
        // Started under a, still being written after a switch to b. It is a's:
        // asking about the whole lifetime would answer a question nobody asked.
        let seen = [sighting("a", 100), sighting("a", 150), sighting("b", 300)];
        assert_eq!(infer(alive(120, 400), &seen).unwrap().0, "a");
    }

    #[test]
    fn a_switch_around_the_moment_it_started_leaves_it_unattributed() {
        let seen = [sighting("a", 100), sighting("b", 200)];
        assert_eq!(infer(started(150), &seen), None, "we cannot tell who began it");
    }

    #[test]
    fn a_conversation_older_than_every_sighting_stays_unattributed() {
        let seen = [sighting("a", 1_000)];
        assert_eq!(infer(started(100), &seen), None, "nothing came before to bracket it");
    }

    #[test]
    fn no_sightings_means_no_inference() {
        assert_eq!(infer(started(100), &[]), None);
    }

    #[test]
    fn a_conversation_with_no_timestamps_cannot_be_placed() {
        let seen = [sighting("a", 100)];
        let nothing = Activity { created_at_ms: None, modified_at_ms: None };
        assert_eq!(infer(nothing, &seen), None);
    }

    #[test]
    fn one_timestamp_is_enough_to_place_it() {
        // A zero-length span still sits inside a sighting.
        let seen = [sighting("a", 150)];
        let only_modified = Activity { created_at_ms: None, modified_at_ms: Some(150) };
        assert_eq!(infer(only_modified, &seen).unwrap().0, "a");
    }

    #[test]
    fn the_account_survives_an_organization_change_under_it() {
        let mut later = sighting("a", 200);
        later.org = "org-other".to_owned();
        let seen = [sighting("a", 100), later];
        let (account, org) = infer(started(150), &seen).unwrap();
        assert_eq!(account, "a");
        assert!(org.is_empty(), "the account is settled, the organization is not");
    }

    #[test]
    fn sources_round_trip() {
        for source in [Source::Observed, Source::Backup, Source::Entry] {
            assert_eq!(Source::parse(source.as_str()), Some(source));
        }
        assert_eq!(Source::parse("telepathy"), None);
    }
}
