//! What the user asked for, shared verbatim by `find` and `apply`.
//!
//! One type, one resolution, one query. If the two commands built their
//! selection separately they would drift, and the promise that you can preview
//! with `find` and then apply the same flags would quietly stop holding.

use crate::claude::desktop::Surface;
use crate::claude::identity::Identities;
use crate::claude::time::iso_to_epoch_ms;
use crate::error::{Error, Result};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Recent,
    Created,
    Size,
}

impl Sort {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "recent" => Some(Sort::Recent),
            "created" => Some(Sort::Created),
            "size" => Some(Sort::Size),
            _ => None,
        }
    }

    /// The ORDER BY clause. Built from a closed set, never from user text.
    #[must_use]
    pub fn order_by(self) -> &'static str {
        match self {
            Sort::Recent => "s.modified_at_ms DESC, s.session_id ASC",
            Sort::Created => "s.created_at_ms DESC, s.session_id ASC",
            Sort::Size => "s.size_bytes DESC, s.session_id ASC",
        }
    }
}

/// User intent, before any of it is turned into account uuids.
///
/// The booleans are independent switches over one selection, not a state machine
/// pretending to be flags, so grouping them into a type would only add a level of
/// naming.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub emails: Vec<String>,
    pub accounts: Vec<String>,
    pub orgs: Vec<String>,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub id: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub surfaces: Vec<Surface>,
    pub archived_only: bool,
    pub include_deleted: bool,
    pub include_hidden: bool,
    /// Require a desktop entry to prove the account, rather than accepting an
    /// inference drawn from when the conversation was alive.
    pub confirmed_only: bool,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub sort: Sort,
}

impl Filter {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.emails.is_empty()
            && self.accounts.is_empty()
            && self.orgs.is_empty()
            && self.cwd.is_none()
            && self.project.is_none()
            && self.branch.is_none()
            && self.model.is_none()
            && self.title.is_none()
            && self.id.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.surfaces.is_empty()
            && !self.archived_only
            && self.query.is_none()
    }

    /// Turns emails into account uuids and relative times into instants.
    ///
    /// An email that matches no known account is an error rather than an empty
    /// result: silently returning nothing would let `apply --email typo` prune
    /// everything the user could see.
    pub fn resolve(&self, identities: &Identities, now_ms: i64) -> Result<Resolved> {
        let mut accounts: BTreeSet<String> = self.accounts.iter().cloned().collect();
        for email in &self.emails {
            let matched: Vec<&String> = identities
                .accounts
                .iter()
                .filter(|(_, label)| label.name.eq_ignore_ascii_case(email))
                .map(|(uuid, _)| uuid)
                .collect();
            if matched.is_empty() {
                return Err(Error::Usage(format!(
                    "no known account has the email {email}. only the account signed in \
                     right now resolves on its own; label the others in identities.json"
                )));
            }
            accounts.extend(matched.into_iter().cloned());
        }

        Ok(Resolved {
            accounts: accounts.into_iter().collect(),
            orgs: self.orgs.clone(),
            cwd: self.cwd.clone(),
            project: self.project.clone(),
            branch: self.branch.clone(),
            model: self.model.clone(),
            title: self.title.clone(),
            id: self.id.clone(),
            since_ms: self.since.as_deref().map(|s| parse_when(s, now_ms)).transpose()?,
            until_ms: self.until.as_deref().map(|s| parse_when(s, now_ms)).transpose()?,
            surfaces: self.surfaces.clone(),
            archived_only: self.archived_only,
            include_deleted: self.include_deleted,
            include_hidden: self.include_hidden,
            confirmed_only: self.confirmed_only,
            query: self.query.clone(),
            limit: self.limit,
            sort: self.sort,
        })
    }
}

/// A filter with every reference turned into something the index can compare.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct Resolved {
    pub accounts: Vec<String>,
    pub orgs: Vec<String>,
    pub cwd: Option<String>,
    pub project: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub title: Option<String>,
    pub id: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub surfaces: Vec<Surface>,
    pub archived_only: bool,
    pub include_deleted: bool,
    pub include_hidden: bool,
    pub confirmed_only: bool,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub sort: Sort,
}

/// `7d`, `3w`, `6mo`, `1y`, or an absolute `2026-08-01` / full ISO timestamp.
pub fn parse_when(s: &str, now_ms: i64) -> Result<i64> {
    let s = s.trim();
    if let Some(ms) = iso_to_epoch_ms(s) {
        return Ok(ms);
    }
    // A bare date is the start of that day.
    if s.len() == 10 {
        if let Some(ms) = iso_to_epoch_ms(&format!("{s}T00:00:00.000Z")) {
            return Ok(ms);
        }
    }
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    let unit = s.get(digits.len()..).unwrap_or("").trim();
    let n: i64 =
        digits.parse().map_err(|_| Error::Usage(format!("unrecognised relative time {s:?}")))?;
    let days = match unit {
        "d" | "" => n,
        "w" => n * 7,
        "mo" | "m" => n * 30,
        "y" => n * 365,
        other => {
            return Err(Error::Usage(format!("unknown time unit {other:?}, use d, w, mo or y")));
        }
    };
    Ok(now_ms - days * 86_400_000)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const NOW: i64 = 1_787_602_036_145; // 2026-08-24T20:07:16.145Z

    #[test]
    fn relative_times_count_back_from_now() {
        assert_eq!(parse_when("0d", NOW).unwrap(), NOW);
        assert_eq!(parse_when("1d", NOW).unwrap(), NOW - 86_400_000);
        assert_eq!(parse_when("2w", NOW).unwrap(), NOW - 14 * 86_400_000);
        assert_eq!(parse_when("3mo", NOW).unwrap(), NOW - 90 * 86_400_000);
        assert_eq!(parse_when("1y", NOW).unwrap(), NOW - 365 * 86_400_000);
    }

    #[test]
    fn a_bare_number_means_days() {
        assert_eq!(parse_when("30", NOW).unwrap(), parse_when("30d", NOW).unwrap());
    }

    #[test]
    fn absolute_dates_and_timestamps_are_taken_as_given() {
        assert_eq!(parse_when("2026-08-01", NOW).unwrap(), 1_785_542_400_000);
        assert_eq!(parse_when("2026-08-24T20:07:16.145Z", NOW).unwrap(), NOW);
    }

    #[test]
    fn an_unknown_unit_is_a_usage_error_not_a_silent_zero() {
        let err = parse_when("5 fortnights", NOW).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(parse_when("soon", NOW).is_err());
    }

    #[test]
    fn an_email_resolves_to_its_account_uuid() {
        let mut ids = Identities::default();
        ids.set_manual_account("uuid-a", "work@example.com");
        ids.set_manual_account("uuid-b", "me@example.com");

        let filter = Filter { emails: vec!["WORK@example.com".to_owned()], ..Filter::default() };
        let resolved = filter.resolve(&ids, NOW).unwrap();
        assert_eq!(resolved.accounts, vec!["uuid-a".to_owned()]);
    }

    #[test]
    fn an_unknown_email_is_refused_rather_than_matching_nothing() {
        // Returning an empty set here would let `apply --email typo` prune
        // everything the user could see.
        let ids = Identities::default();
        let filter = Filter { emails: vec!["nobody@example.com".to_owned()], ..Filter::default() };
        let err = filter.resolve(&ids, NOW).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("identities.json"));
    }

    #[test]
    fn accounts_given_directly_and_by_email_are_unioned_without_duplicates() {
        let mut ids = Identities::default();
        ids.set_manual_account("uuid-a", "work@example.com");
        let filter = Filter {
            emails: vec!["work@example.com".to_owned()],
            accounts: vec!["uuid-a".to_owned(), "uuid-c".to_owned()],
            ..Filter::default()
        };
        let resolved = filter.resolve(&ids, NOW).unwrap();
        assert_eq!(resolved.accounts, vec!["uuid-a".to_owned(), "uuid-c".to_owned()]);
    }

    #[test]
    fn an_empty_filter_is_recognisable_because_apply_treats_it_as_everything() {
        assert!(Filter::default().is_empty());
        assert!(!Filter { branch: Some("main".to_owned()), ..Filter::default() }.is_empty());
        // Presentation only flags do not make a filter selective.
        assert!(Filter { limit: Some(10), include_hidden: true, ..Filter::default() }.is_empty());
    }

    #[test]
    fn sort_orders_come_from_a_closed_set() {
        assert_eq!(Sort::parse("recent"), Some(Sort::Recent));
        assert_eq!(Sort::parse("size"), Some(Sort::Size));
        assert_eq!(Sort::parse("; drop table session"), None);
        assert!(Sort::Recent.order_by().contains("modified_at_ms"));
    }
}
