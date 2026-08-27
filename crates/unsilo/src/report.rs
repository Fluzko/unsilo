//! Rendering. Kept apart from the operations so that changing how something
//! looks cannot change what it decided, and so tests assert on values.

use crate::ops::doctor::{Report, Severity};
use std::fmt::Write as _;

/// Precision loss is the point: this is a size for a human to read, not a value
/// anything computes with.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS.get(unit).copied().unwrap_or("B"))
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[must_use]
pub fn doctor(r: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "unsilo {}\n", r.unsilo_version);
    layout_section(r, &mut out);
    accounts_section(r, &mut out);
    transcripts_section(r, &mut out);
    retention_section(r, &mut out);
    store_section(r, &mut out);
    problems_section(r, &mut out);
    out
}

fn layout_section(r: &Report, out: &mut String) {
    let _ = writeln!(out, "layout");
    for dir in &r.config_dirs {
        let _ = writeln!(out, "  cli config dir      {}", dir.path);
        let _ = writeln!(
            out,
            "                      {} conversations, {} project dirs, {}",
            dir.conversations,
            dir.project_dirs,
            human_bytes(dir.bytes)
        );
        for (reason, count) in &dir.hidden {
            let _ = writeln!(out, "                      {count} hidden ({reason})");
        }
        if dir.subagents > 0 {
            let _ = writeln!(
                out,
                "                      {} subagent transcripts (not conversations)",
                dir.subagents
            );
        }
        if dir.skipped > 0 {
            let _ = writeln!(out, "                      {} files skipped", dir.skipped);
        }
    }
    if r.config_dirs.is_empty() {
        let _ = writeln!(out, "  cli config dir      (none)");
    }
    for path in &r.user_data {
        let _ = writeln!(out, "  desktop userData    {path}");
    }
    if r.user_data.is_empty() {
        let _ = writeln!(out, "  desktop userData    (none)");
    }
    if !r.cli_versions.is_empty() {
        let versions: Vec<String> =
            r.cli_versions.iter().take(3).map(|(v, n)| format!("{v} ({n})")).collect();
        let _ = writeln!(out, "  cli versions        {}", versions.join(", "));
    }
    let backend = match r.remote_backend {
        Some(true) => "REMOTE (tengu_hover_rest on)",
        Some(false) => "local files",
        None => "undetermined",
    };
    let _ = writeln!(out, "  storage backend     {backend}");
    let _ = writeln!(
        out,
        "  writes              {}",
        if r.writes_allowed { "allowed" } else { "BLOCKED" }
    );
}

fn accounts_section(r: &Report, out: &mut String) {
    let _ = writeln!(out, "\naccounts");
    for account in &r.accounts {
        let email = account.email.clone().unwrap_or_else(|| "(email unresolved)".to_owned());
        let _ = writeln!(
            out,
            "  {}  {:<34}{}",
            short(&account.uuid),
            email,
            if account.is_active { "ACTIVE" } else { "" }
        );
        for org in &account.orgs {
            let name = org.name.clone().unwrap_or_else(|| "(unnamed)".to_owned());
            let name: String = name.chars().take(26).collect();
            let _ = writeln!(
                out,
                "            {}  {:<26} {} sessions, {} deleted{}",
                short(&org.uuid),
                name,
                org.entries,
                org.tombstones,
                if org.is_active { "  <-" } else { "" }
            );
        }
    }
    if r.accounts.is_empty() {
        let _ = writeln!(out, "  (none)");
    }
    if r.invisible_under_active > 0 {
        let _ = writeln!(
            out,
            "\n  {} desktop sessions NOT visible under the active account",
            r.invisible_under_active
        );
    }
}

fn transcripts_section(r: &Report, out: &mut String) {
    let _ = writeln!(out, "\ntranscripts");
    let _ = writeln!(out, "  conversations       {}", r.conversations());
    if r.subagents() > 0 {
        let _ = writeln!(out, "  subagents           {}", r.subagents());
    }
    let _ = writeln!(out, "  with desktop entry  {} of {}", r.linked_entries, r.total_entries);
    if r.tail_unresolved > 0 {
        let _ = writeln!(out, "  tail unresolved     {}", r.tail_unresolved);
    }
    for (session_id, dirs) in &r.duplicate_locations {
        let _ = writeln!(
            out,
            "  duplicated          {} across {} project dirs",
            short(session_id),
            dirs.len()
        );
    }
}

fn retention_section(r: &Report, out: &mut String) {
    let _ = writeln!(out, "\nretention");
    let _ = writeln!(
        out,
        "  cleanupPeriodDays   {} ({})",
        r.retention.cleanup_period_days,
        if r.retention.from_settings { "settings.json" } else { "default" }
    );
    let _ = writeln!(
        out,
        "  at risk             {} transcripts, {}",
        r.retention.at_risk,
        human_bytes(r.retention.at_risk_bytes)
    );
}

fn store_section(r: &Report, out: &mut String) {
    let _ = writeln!(out, "\nstore");
    let _ = writeln!(out, "  {}", r.store.path);
    let viable = match r.store.hardlinks_viable {
        Some(true) => "viable (same volume)",
        Some(false) => "NO (other volume), copies will be used",
        None => "undetermined",
    };
    let _ = writeln!(out, "  hardlinks           {viable}");
    let _ = writeln!(
        out,
        "  contents            {} transcripts, {} ledger entries",
        r.store.transcripts, r.store.ledger_entries
    );
}

fn problems_section(r: &Report, out: &mut String) {
    let _ = writeln!(out, "\nproblems");
    if r.problems.is_empty() {
        let _ = writeln!(out, "  none");
    }
    for problem in &r.problems {
        let tag = match problem.severity {
            Severity::Info => "note ",
            Severity::Warn => "warn ",
            Severity::Blocker => "BLOCK",
        };
        let _ = writeln!(out, "  {tag} {}", problem.message);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes_stay_readable_at_every_scale() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1_048_576), "1.0 MB");
        assert_eq!(human_bytes(224 * 1_048_576), "224.0 MB");
        assert_eq!(human_bytes(5_368_709_120), "5.0 GB");
    }

    #[test]
    fn short_ids_never_panic_on_a_short_input() {
        assert_eq!(short("3db70634-c5f9"), "3db70634");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }
}

#[must_use]
pub fn find(results: &crate::ops::find::Results, home: &camino::Utf8Path) -> String {
    let mut out = String::new();
    if results.rows.is_empty() {
        let _ = writeln!(out, "\n  no results out of {} sessions\n", results.total);
        return out;
    }

    let _ = writeln!(
        out,
        "\n{:<9} {:<11} {:<28} {:<9} {:<22} TITLE",
        "ID", "DATE", "PROJECT", "SIZE", "ACCOUNT"
    );
    for row in &results.rows {
        let date = row.modified_at_ms.map_or_else(|| "?".to_owned(), iso_date);
        let account = row.scopes.first().map_or_else(
            || "(cli only)".to_owned(),
            |scope| {
                results
                    .identities
                    .emails
                    .get(&scope.account)
                    .cloned()
                    .unwrap_or_else(|| short(&scope.account).to_owned())
            },
        );
        let title = row.display_title().unwrap_or("(untitled)").replace(['\n', '\r'], " ");
        let _ = writeln!(
            out,
            "{:<9} {:<11} {:<28} {:<9} {:<22} {}",
            row.short_id(),
            date,
            truncate(&shorten_home(row.cwd.as_deref().unwrap_or("?"), home), 28, Keep::Tail),
            human_bytes(u64::try_from(row.size_bytes).unwrap_or(0)),
            truncate(&account, 22, Keep::Head),
            truncate(&title, 48, Keep::Head)
        );
    }
    let _ = writeln!(out, "\n  {} of {} sessions\n", results.matched, results.total);
    out
}

/// `cd <cwd> && claude --resume <id>`, ready to run. Closes the loop without
/// Unsilo having to spawn anything itself.
#[must_use]
pub fn resume_commands(results: &crate::ops::find::Results) -> String {
    let mut out = String::new();
    for row in &results.rows {
        match &row.cwd {
            Some(cwd) => {
                let _ = writeln!(out, "cd {cwd} && claude --resume {}", row.session_id);
            }
            None => {
                let _ = writeln!(out, "claude --resume {}", row.session_id);
            }
        }
    }
    out
}

#[must_use]
pub fn paths(results: &crate::ops::find::Results) -> String {
    let mut out = String::new();
    for row in &results.rows {
        let _ = writeln!(out, "{}/{}.jsonl", row.origin_dir, row.session_id);
    }
    out
}

fn shorten_home(path: &str, home: &camino::Utf8Path) -> String {
    path.strip_prefix(home.as_str()).map_or_else(|| path.to_owned(), |rest| format!("~{rest}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// Paths differ at the end, so drop the front.
    Tail,
    /// Prose reads from the front. A title containing a slash is still prose.
    Head,
}

fn truncate(s: &str, width: usize, keep: Keep) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_owned();
    }
    let room = width.saturating_sub(2);
    match keep {
        Keep::Tail => format!("..{}", chars.iter().skip(chars.len() - room).collect::<String>()),
        Keep::Head => format!("{}..", chars.iter().take(room).collect::<String>()),
    }
}

fn iso_date(ms: i64) -> String {
    // Only the date part is shown, so a plain civil conversion is enough.
    let days = ms.div_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[must_use]
pub fn snapshot(written: &crate::snapshot::write::Written) -> String {
    use crate::snapshot::EntryKind;
    let manifest = &written.manifest;
    let mut out = String::new();
    let _ = writeln!(out, "\n  scope           {:?}", manifest.scope);
    for (label, kind) in [
        ("transcripts", EntryKind::Transcript),
        ("subagents", EntryKind::Subagent),
        ("desktop", EntryKind::DesktopEntry),
        ("deleted", EntryKind::Tombstone),
        ("store", EntryKind::StoreFile),
    ] {
        let count = manifest.count(kind);
        if count > 0 {
            let _ = writeln!(out, "  {label:<15} {count}");
        }
    }
    if manifest.has_bodies {
        let _ = writeln!(
            out,
            "  size            {} -> {}",
            human_bytes(manifest.total_bytes()),
            human_bytes(written.archive_bytes)
        );
    } else {
        let _ = writeln!(
            out,
            "  size            metadata only, {}",
            human_bytes(written.archive_bytes)
        );
    }
    let _ = writeln!(out, "  written to      {}\n", written.path);
    out
}

#[must_use]
pub fn restore(r: &crate::ops::restore::Report) -> String {
    use crate::ops::restore::Verdict;
    let mut out = String::new();
    let _ = writeln!(out, "\n  snapshot        {}", r.snapshot);
    let _ = writeln!(out, "  scope           {:?}\n", r.scope);

    for item in &r.planned {
        let mark = match item.verdict {
            Verdict::Restore => "+",
            Verdict::Identical => "=",
            Verdict::LocalIsNewer => ">",
            Verdict::Conflict => "!",
        };
        // Everything untouched is the boring majority; only show what moves.
        if item.verdict != Verdict::Identical {
            let _ = writeln!(out, "  {mark} {}", item.target);
        }
    }

    let _ = writeln!(
        out,
        "\n  {} restored, {} untouched, {} in conflict{}\n",
        r.restored,
        r.skipped,
        r.conflicts,
        if r.dry_run { "  (dry run, nothing was written)" } else { "" }
    );
    out
}

#[must_use]
pub fn apply(r: &crate::ops::apply::Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "\n  active account  {} / {}{}",
        short(&r.active.account),
        short(&r.active.org),
        r.active.email.as_ref().map_or_else(String::new, |e| format!("  ({e})"))
    );
    let _ = writeln!(out, "  selected        {}\n", r.selected);

    if !r.projected.is_empty() {
        let _ = writeln!(out, "  desktop");
        for item in &r.projected {
            let _ = writeln!(
                out,
                "    + {}  {}  (from {}, {} of mcp dropped)",
                item.host_id.get(6..14).unwrap_or(&item.host_id),
                item.title.as_deref().unwrap_or("(untitled)"),
                item.from,
                human_bytes(item.stripped_bytes)
            );
        }
    }
    if r.already_visible > 0 {
        let _ = writeln!(out, "    = {} already visible", r.already_visible);
    }
    if !r.relinked.is_empty() {
        let _ = writeln!(out, "\n  cli");
        for item in &r.relinked {
            let _ = writeln!(out, "    + {}  relinked from the store", short(&item.session_id));
        }
    }
    for path in &r.pruned {
        let _ = writeln!(out, "    - {path}");
    }
    for path in &r.kept_modified {
        let _ = writeln!(out, "    ! {path}  modified since, kept");
    }

    if r.baseline_created {
        let _ = writeln!(out, "\n  baseline captured");
    }
    if let Some(path) = &r.auto_snapshot {
        let _ = writeln!(out, "  snapshot        {}", path.file_name().unwrap_or_default());
    }
    let _ = writeln!(
        out,
        "\n  {} changes{}\n",
        r.changes(),
        if r.dry_run { "  (dry run, nothing was written)" } else { "" }
    );
    out
}

#[must_use]
pub fn off(r: &crate::ops::off::Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out);
    for path in &r.removed {
        let _ = writeln!(out, "  - {path}");
    }
    for path in &r.kept {
        let _ = writeln!(out, "  ! {path}  modified since, kept");
    }
    for path in &r.missing {
        let _ = writeln!(out, "  . {path}  was already gone");
    }
    let _ = writeln!(
        out,
        "\n  {} removed, {} kept{}",
        r.removed.len(),
        r.kept.len(),
        if r.dry_run { "  (dry run, nothing was written)" } else { "" }
    );
    if r.purged {
        let _ = writeln!(out, "  store and index deleted");
    } else {
        let _ = writeln!(
            out,
            "  store untouched: {} transcripts. unsilo apply to turn it back on\n",
            r.store_transcripts
        );
    }
    out
}

#[must_use]
pub fn labelled(l: &crate::ops::label::Labelled) -> String {
    let mut out = String::new();
    let kind = match l.kind {
        crate::ops::label::Kind::Account => "account",
        crate::ops::label::Kind::Org => "org",
    };
    let _ = writeln!(out, "\n  {kind:<8} {}", l.uuid);
    match &l.replaced {
        Some(before) if before != &l.name => {
            let _ = writeln!(out, "  name     {} -> {}\n", before, l.name);
        }
        _ => {
            let _ = writeln!(out, "  name     {}\n", l.name);
        }
    }
    out
}

#[must_use]
pub fn learned(l: &crate::ops::label::Learned) -> String {
    let mut out = String::new();
    match (&l.active_account, &l.active_email) {
        (Some(account), Some(email)) => {
            let _ = writeln!(out, "\n  active   {}  {}", short(account), email);
        }
        (Some(account), None) => {
            let _ = writeln!(out, "\n  active   {}  (no email in config)", short(account));
        }
        _ => {
            let _ = writeln!(out, "\n  active   (could not read the signed in account)");
        }
    }
    let _ = writeln!(
        out,
        "  learned  {}\n",
        if l.added == 0 { "nothing new".to_owned() } else { format!("{} new label(s)", l.added) }
    );
    out
}

#[must_use]
pub fn labels(l: &crate::ops::label::Listing) -> String {
    use crate::claude::identity::Source;
    let mut out = String::new();
    for (heading, rows) in [("accounts", &l.accounts), ("organizations", &l.orgs)] {
        let _ = writeln!(out, "\n{heading}");
        if rows.is_empty() {
            let _ = writeln!(out, "  (none)");
        }
        for row in rows {
            let source = match row.source {
                Some(Source::Manual) => "manual",
                Some(Source::Learned) => "learned",
                None => "unnamed",
            };
            let _ = writeln!(
                out,
                "  {}  {:<34} {:<8} {} sessions{}",
                short(&row.uuid),
                row.name.clone().unwrap_or_else(|| "(unnamed)".to_owned()),
                source,
                row.sessions,
                if row.is_active { "  <-" } else { "" }
            );
        }
    }
    let unnamed = l.accounts.iter().filter(|r| r.name.is_none()).count()
        + l.orgs.iter().filter(|r| r.name.is_none()).count();
    if unnamed > 0 {
        let _ = writeln!(out, "\n  {unnamed} unnamed. `unsilo label <id> <name>` to fix\n");
    } else {
        let _ = writeln!(out);
    }
    out
}
