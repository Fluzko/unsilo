//! Rendering. Kept apart from the operations so that changing how something
//! looks cannot change what it decided, and so tests assert on values.
//!
//! Every function takes a [`Style`], and every function still produces readable
//! output with [`Style::plain`]. Colour and framing carry no information of their
//! own: they repeat what the text already says, faster.

use crate::ops::doctor::{Report, Severity};
use crate::style::{Mark, Style, pad};
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

// ------------------------------------------------------------------- doctor

#[must_use]
pub fn doctor(st: Style, r: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\nunsilo {}", st.dim(r.unsilo_version));

    // The reason this tool exists, said before anything else. It used to be a
    // line buried inside the accounts section.
    if r.invisible_under_active > 0 {
        let _ = writeln!(
            out,
            "\n{}",
            st.headline(&format!(
                "{} desktop sessions NOT visible under the active account",
                r.invisible_under_active
            ))
        );
    }
    let (attributed, needing) = r.attribution;
    if needing > 0 && attributed < needing {
        if r.invisible_under_active == 0 {
            let _ = writeln!(out);
        }
        let _ = writeln!(
            out,
            "{}",
            st.dim(&format!(
                "  {needing} conversation(s) started in the terminal record no account; \
                 {attributed} placed by timestamp"
            ))
        );
    }

    layout_section(st, r, &mut out);
    accounts_section(st, r, &mut out);
    transcripts_section(st, r, &mut out);
    retention_section(st, r, &mut out);
    store_section(st, r, &mut out);
    problems_section(st, r, &mut out);
    out
}

fn layout_section(st: Style, r: &Report, out: &mut String) {
    let _ = writeln!(out, "\n{}", st.section("layout"));
    for dir in &r.config_dirs {
        let _ = writeln!(out, "  cli config dir      {}", st.id(dir.path.as_str()));
        let _ = writeln!(
            out,
            "                      {} conversations, {} project dirs, {}",
            st.bold(&dir.conversations.to_string()),
            dir.project_dirs,
            st.dim(&human_bytes(dir.bytes))
        );
        for (reason, count) in &dir.hidden {
            let _ = writeln!(
                out,
                "                      {}",
                st.dim(&format!("{count} hidden ({reason})"))
            );
        }
        if dir.subagents > 0 {
            let _ = writeln!(
                out,
                "                      {}",
                st.dim(&format!("{} subagent transcripts (not conversations)", dir.subagents))
            );
        }
        if dir.skipped > 0 {
            let _ = writeln!(
                out,
                "                      {}",
                st.dim(&format!("{} files skipped", dir.skipped))
            );
        }
    }
    if r.config_dirs.is_empty() {
        let _ = writeln!(out, "  cli config dir      {}", st.warn("(none)"));
    }
    for path in &r.user_data {
        let _ = writeln!(out, "  desktop userData    {}", st.id(path.as_str()));
    }
    if r.user_data.is_empty() {
        let _ = writeln!(out, "  desktop userData    {}", st.warn("(none)"));
    }
    if !r.cli_versions.is_empty() {
        let versions: Vec<String> =
            r.cli_versions.iter().take(3).map(|(v, n)| format!("{v} ({n})")).collect();
        let _ = writeln!(out, "  cli versions        {}", st.dim(&versions.join(", ")));
    }
    let backend = match r.remote_backend {
        Some(true) => st.bad("REMOTE (tengu_hover_rest on)"),
        Some(false) => "local files".to_owned(),
        None => st.warn("undetermined"),
    };
    let _ = writeln!(out, "  storage backend     {backend}");
    let writes = if r.writes_allowed { st.ok("allowed") } else { st.bad("BLOCKED") };
    let _ = writeln!(out, "  writes              {writes}");
}

fn accounts_section(st: Style, r: &Report, out: &mut String) {
    let _ = writeln!(out, "\n{}", st.section("accounts"));
    for account in &r.accounts {
        let email = account
            .email
            .clone()
            .map_or_else(|| st.warn("(email unresolved)"), |email| st.id(&email));
        let active = if account.is_active { st.ok("ACTIVE") } else { String::new() };
        let _ = writeln!(out, "  {}  {}{active}", short(&account.uuid), pad(&email, 34));
        for org in &account.orgs {
            let name = org
                .name
                .clone()
                .map_or_else(|| st.warn("(unnamed)"), |name| truncate(&name, 26, Keep::Head));
            let _ = writeln!(
                out,
                "            {}  {} {} sessions, {} deleted{}",
                short(&org.uuid),
                pad(&name, 26),
                org.entries,
                org.tombstones,
                if org.is_active { st.ok("  <-") } else { String::new() }
            );
        }
    }
    if r.accounts.is_empty() {
        let _ = writeln!(out, "  {}", st.warn("(none)"));
    }
    if !r.unresolved_accounts.is_empty() {
        let _ = writeln!(
            out,
            "  {}",
            st.dim(&format!("{} unnamed. `unsilo label <id> <name>`", r.unresolved_accounts.len()))
        );
    }
}

fn transcripts_section(st: Style, r: &Report, out: &mut String) {
    let _ = writeln!(out, "\n{}", st.section("transcripts"));
    let _ = writeln!(out, "  conversations       {}", st.bold(&r.conversations().to_string()));
    if r.subagents() > 0 {
        let _ = writeln!(out, "  subagents           {}", st.dim(&r.subagents().to_string()));
    }
    let _ = writeln!(out, "  with desktop entry  {} of {}", r.linked_entries, r.total_entries);
    let (attributed, needing) = r.attribution;
    if needing > 0 {
        let _ = writeln!(
            out,
            "  cli born            {needing}, of which {} attributed by timestamp",
            if attributed > 0 { st.ok(&attributed.to_string()) } else { st.warn("0") }
        );
    }
    if r.tail_unresolved > 0 {
        let _ = writeln!(out, "  tail unresolved     {}", st.warn(&r.tail_unresolved.to_string()));
    }
    for (session_id, dirs) in &r.duplicate_locations {
        let _ = writeln!(
            out,
            "  duplicated          {} across {} project dirs",
            st.id(short(session_id)),
            dirs.len()
        );
    }
}

fn retention_section(st: Style, r: &Report, out: &mut String) {
    let _ = writeln!(out, "\n{}", st.section("retention"));
    let _ = writeln!(
        out,
        "  cleanupPeriodDays   {} {}",
        r.retention.cleanup_period_days,
        st.dim(if r.retention.from_settings { "(settings.json)" } else { "(default)" })
    );
    let at_risk = if r.retention.at_risk > 0 {
        st.warn(&r.retention.at_risk.to_string())
    } else {
        r.retention.at_risk.to_string()
    };
    let _ = writeln!(
        out,
        "  at risk             {at_risk} transcripts, {}",
        st.dim(&human_bytes(r.retention.at_risk_bytes))
    );
}

fn store_section(st: Style, r: &Report, out: &mut String) {
    let _ = writeln!(out, "\n{}", st.section("store"));
    let _ = writeln!(out, "  {}", st.id(r.store.path.as_str()));
    let viable = match r.store.hardlinks_viable {
        Some(true) => st.ok("viable (same volume)"),
        Some(false) => st.warn("NO (other volume), copies will be used"),
        None => st.warn("undetermined"),
    };
    let _ = writeln!(out, "  hardlinks           {viable}");
    let _ = writeln!(
        out,
        "  contents            {} transcripts, {} ledger entries",
        st.bold(&r.store.transcripts.to_string()),
        r.store.ledger_entries
    );
}

fn problems_section(st: Style, r: &Report, out: &mut String) {
    let _ = writeln!(out, "\n{}", st.section("problems"));
    if r.problems.is_empty() {
        let _ = writeln!(out, "  {}\n", st.ok("none"));
        return;
    }
    for problem in &r.problems {
        let tag = match problem.severity {
            Severity::Info => st.dim("note "),
            Severity::Warn => st.warn("warn "),
            Severity::Blocker => st.bad("BLOCK"),
        };
        let _ = writeln!(out, "  {tag} {}", problem.message);
    }
    let _ = writeln!(out);
}

// --------------------------------------------------------------------- find

/// Wide enough for most emails, and every cell is cut to fit rather than allowed
/// to shove the column after it off its position.
const ACCOUNT_WIDTH: usize = 23;

/// What an account cell is saying, which decides how it is coloured.
#[derive(Debug, Clone, Copy)]
enum Cell {
    /// A desktop entry states it.
    Stated,
    /// Worked out from when the conversation started.
    Inferred,
    /// Nothing knows.
    Absent,
}

#[must_use]
pub fn find(st: Style, results: &crate::ops::find::Results, home: &camino::Utf8Path) -> String {
    let mut out = String::new();
    if results.rows.is_empty() {
        let _ = writeln!(
            out,
            "\n  {}\n",
            st.dim(&format!("no results out of {} sessions", results.total))
        );
        return out;
    }

    let _ = writeln!(out, "\n{}", st.section("conversations"));
    let _ = writeln!(
        out,
        "{}",
        st.dim(&format!(
            "{}{}{}{}{}TITLE",
            pad("ID", 10),
            pad("DATE", 12),
            pad("PROJECT", 29),
            pad("SIZE", 10),
            pad("ACCOUNT", ACCOUNT_WIDTH)
        ))
    );

    for row in &results.rows {
        let date = row.modified_at_ms.map_or_else(|| "?".to_owned(), iso_date);
        let name_of = |uuid: &str| {
            results.identities.emails.get(uuid).cloned().unwrap_or_else(|| short(uuid).to_owned())
        };
        // A trailing "?" is the whole difference between what an entry states and
        // what the timestamps suggest. An entry Unsilo built states nothing.
        //
        // Truncated before it is coloured: shortening a string that already holds
        // escape sequences cuts them in half.
        let (account_text, kind) = match (row.scopes.first(), &row.inferred_account) {
            (Some(scope), inferred) if scope.adopted => match inferred {
                Some(account) => (format!("{}?", name_of(account)), Cell::Inferred),
                None => ("(cli only)".to_owned(), Cell::Absent),
            },
            (Some(scope), _) => (name_of(&scope.account), Cell::Stated),
            (None, Some(inferred)) => (format!("{}?", name_of(inferred)), Cell::Inferred),
            (None, None) => ("(cli only)".to_owned(), Cell::Absent),
        };
        let account_text = truncate(&account_text, ACCOUNT_WIDTH - 1, Keep::Head);
        let account = match kind {
            Cell::Stated => st.id(&account_text),
            Cell::Inferred => st.warn(&account_text),
            Cell::Absent => st.dim(&account_text),
        };
        let title = row.display_title().unwrap_or("(untitled)").replace(['\n', '\r'], " ");

        let _ = writeln!(
            out,
            "{}{}{}{}{}{}",
            pad(&st.id(row.short_id()), 10),
            pad(&st.dim(&date), 12),
            pad(
                &truncate(&shorten_home(row.cwd.as_deref().unwrap_or("?"), home), 28, Keep::Tail),
                29
            ),
            pad(&st.dim(&human_bytes(u64::try_from(row.size_bytes).unwrap_or(0))), 10),
            pad(&account, ACCOUNT_WIDTH),
            truncate(&title, 44, Keep::Head)
        );
    }
    let _ = writeln!(
        out,
        "\n  {} of {} sessions\n",
        st.bold(&results.matched.to_string()),
        results.total
    );
    out
}

/// `cd <cwd> && claude --resume <id>`, ready to run. Never dressed: this is meant
/// to be pasted into a shell.
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

/// One path per line, for piping. Never dressed.
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

fn iso_date(ms: i64) -> String {
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

// ----------------------------------------------------------------- snapshot

#[must_use]
pub fn snapshot(st: Style, written: &crate::snapshot::write::Written) -> String {
    use crate::snapshot::EntryKind;
    let manifest = &written.manifest;
    let mut out = String::new();
    let _ =
        writeln!(out, "\n{}", st.section(&format!("snapshot {:?}", manifest.scope).to_lowercase()));
    for (label, kind) in [
        ("transcripts", EntryKind::Transcript),
        ("subagents", EntryKind::Subagent),
        ("desktop", EntryKind::DesktopEntry),
        ("deleted", EntryKind::Tombstone),
        ("store", EntryKind::StoreFile),
    ] {
        let count = manifest.count(kind);
        if count > 0 {
            let _ = writeln!(out, "  {} {count}", pad(label, 15));
        }
    }
    if manifest.has_bodies {
        let _ = writeln!(
            out,
            "  size            {} {} {}",
            st.dim(&human_bytes(manifest.total_bytes())),
            st.dim("->"),
            st.bold(&human_bytes(written.archive_bytes))
        );
    } else {
        let _ = writeln!(
            out,
            "  size            {}",
            st.dim(&format!("metadata only, {}", human_bytes(written.archive_bytes)))
        );
    }
    let _ = writeln!(out, "  written to      {}\n", st.id(written.path.as_str()));
    out
}

// ------------------------------------------------------------------ restore

#[must_use]
pub fn restore(st: Style, r: &crate::ops::restore::Report) -> String {
    use crate::ops::restore::Verdict;
    let mut out = String::new();
    let _ = writeln!(out, "\n{}", st.section(&format!("restore {:?}", r.scope).to_lowercase()));
    let _ = writeln!(out, "  from            {}\n", st.id(r.snapshot.as_str()));

    for item in &r.planned {
        // Everything untouched is the boring majority; only show what moves.
        if item.verdict == Verdict::Identical {
            continue;
        }
        let mark = match item.verdict {
            Verdict::Restore => Mark::Added,
            Verdict::Identical => Mark::Unchanged,
            Verdict::LocalIsNewer => Mark::Newer,
            Verdict::Conflict => Mark::Kept,
        };
        let _ = writeln!(out, "  {} {}", st.marker(mark), st.id(item.target.as_str()));
    }

    let conflicts = if r.conflicts > 0 {
        st.warn(&format!("{} in conflict", r.conflicts))
    } else {
        st.dim("0 in conflict")
    };
    let _ = writeln!(
        out,
        "\n  {} restored, {} untouched, {conflicts}{}\n",
        st.bold(&r.restored.to_string()),
        st.dim(&r.skipped.to_string()),
        dry_run_note(st, r.dry_run)
    );
    out
}

fn dry_run_note(st: Style, dry_run: bool) -> String {
    if dry_run { st.dim("  (dry run, nothing was written)") } else { String::new() }
}

// -------------------------------------------------------------------- apply

#[must_use]
pub fn apply(st: Style, r: &crate::ops::apply::Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\n{}", st.section("apply"));
    let _ = writeln!(
        out,
        "  active account  {} / {}{}",
        st.id(short(&r.active.account)),
        st.id(short(&r.active.org)),
        r.active.email.as_ref().map_or_else(String::new, |e| st.dim(&format!("  ({e})")))
    );
    let _ = writeln!(out, "  selected        {}", st.bold(&r.selected.to_string()));

    if !r.projected.is_empty() {
        let _ = writeln!(out, "\n  {}", st.dim("desktop"));
        for item in &r.projected {
            let _ = writeln!(
                out,
                "    {} {}  {}  {}",
                st.marker(Mark::Added),
                st.id(item.host_id.get(6..14).unwrap_or(&item.host_id)),
                truncate(item.title.as_deref().unwrap_or("(untitled)"), 44, Keep::Head),
                st.dim(&format!(
                    "from {}, {} of mcp dropped",
                    item.from,
                    human_bytes(item.stripped_bytes)
                ))
            );
        }
    }
    if r.already_visible > 0 {
        let _ = writeln!(
            out,
            "    {} {}",
            st.marker(Mark::Unchanged),
            st.dim(&format!("{} already visible", r.already_visible))
        );
    }
    if !r.adopted.is_empty() {
        let _ = writeln!(out, "\n  {}", st.dim("adopted from the cli"));
        for item in &r.adopted {
            let title = item
                .title
                .as_deref()
                .map_or_else(|| "(untitled)".to_owned(), |t| t.replace(['\n', '\r'], " "));
            let _ = writeln!(
                out,
                "    {} {}  {}",
                st.marker(Mark::Added),
                st.id(short(&item.session_id)),
                truncate(&title, 60, Keep::Head)
            );
        }
    }
    if !r.relinked.is_empty() {
        let _ = writeln!(out, "\n  {}", st.dim("cli"));
        for item in &r.relinked {
            let _ = writeln!(
                out,
                "    {} {}  {}",
                st.marker(Mark::Added),
                st.id(short(&item.session_id)),
                st.dim("relinked from the store")
            );
        }
    }
    apply_removals(st, r, &mut out);
    if r.adoptable > 0 {
        let _ = writeln!(
            out,
            "\n  {}",
            st.warn(&format!("{} conversation(s) the desktop has never known about", r.adoptable))
        );
        let _ = writeln!(
            out,
            "  {}",
            st.dim("--adopt-cli-sessions would give them an entry so it lists them")
        );
    }
    if r.baseline_created {
        let _ = writeln!(out, "\n  {}", st.ok("baseline captured"));
    }
    if let Some(path) = &r.auto_snapshot {
        let _ = writeln!(out, "  snapshot        {}", st.dim(path.file_name().unwrap_or_default()));
    }
    let _ = writeln!(
        out,
        "\n  {} changes{}\n",
        st.bold(&r.changes().to_string()),
        dry_run_note(st, r.dry_run)
    );
    out
}

fn apply_removals(st: Style, r: &crate::ops::apply::Report, out: &mut String) {
    if !r.pruned.is_empty() || !r.kept_modified.is_empty() {
        let _ = writeln!(out);
    }
    for path in &r.pruned {
        let _ = writeln!(out, "    {} {}", st.marker(Mark::Removed), st.id(path.as_str()));
    }
    for path in &r.kept_modified {
        let _ = writeln!(
            out,
            "    {} {}  {}",
            st.marker(Mark::Kept),
            st.id(path.as_str()),
            st.dim("modified since, kept")
        );
    }
}

// ---------------------------------------------------------------------- off

#[must_use]
pub fn off(st: Style, r: &crate::ops::off::Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "\n{}", st.section("off"));
    for path in &r.removed {
        let _ = writeln!(out, "  {} {}", st.marker(Mark::Removed), st.id(path.as_str()));
    }
    for path in &r.kept {
        let _ = writeln!(
            out,
            "  {} {}  {}",
            st.marker(Mark::Kept),
            st.id(path.as_str()),
            st.dim("modified since, kept")
        );
    }
    for path in &r.missing {
        let _ = writeln!(
            out,
            "  {} {}  {}",
            st.marker(Mark::Missing),
            st.id(path.as_str()),
            st.dim("was already gone")
        );
    }
    let _ = writeln!(
        out,
        "\n  {} removed, {} kept{}",
        st.bold(&r.removed.len().to_string()),
        st.dim(&r.kept.len().to_string()),
        dry_run_note(st, r.dry_run)
    );
    if r.purged {
        let _ = writeln!(out, "  {}\n", st.bad("store and index deleted"));
    } else {
        let _ = writeln!(
            out,
            "  {}\n",
            st.dim(&format!(
                "store untouched: {} transcripts. unsilo apply to turn it back on",
                r.store_transcripts
            ))
        );
    }
    out
}

// -------------------------------------------------------------------- label

#[must_use]
pub fn labelled(st: Style, l: &crate::ops::label::Labelled) -> String {
    let mut out = String::new();
    let kind = match l.kind {
        crate::ops::label::Kind::Account => "account",
        crate::ops::label::Kind::Org => "org",
    };
    let _ = writeln!(out, "\n  {} {}", pad(kind, 8), st.id(&l.uuid));
    match &l.replaced {
        Some(before) if before != &l.name => {
            let _ = writeln!(
                out,
                "  name     {} {} {}\n",
                st.dim(before),
                st.dim("->"),
                st.ok(&l.name)
            );
        }
        _ => {
            let _ = writeln!(out, "  name     {}\n", st.ok(&l.name));
        }
    }
    out
}

#[must_use]
pub fn learned(st: Style, l: &crate::ops::label::Learned) -> String {
    let mut out = String::new();
    match (&l.active_account, &l.active_email) {
        (Some(account), Some(email)) => {
            let _ = writeln!(out, "\n  active   {}  {}", short(account), st.id(email));
        }
        (Some(account), None) => {
            let _ = writeln!(
                out,
                "\n  active   {}  {}",
                short(account),
                st.warn("(no email in config)")
            );
        }
        _ => {
            let _ =
                writeln!(out, "\n  active   {}", st.warn("(could not read the signed in account)"));
        }
    }
    let learned = if l.added == 0 {
        st.dim("nothing new")
    } else {
        st.ok(&format!("{} new label(s)", l.added))
    };
    let _ = writeln!(out, "  learned  {learned}\n");
    out
}

#[must_use]
pub fn labels(st: Style, l: &crate::ops::label::Listing) -> String {
    use crate::claude::identity::Source;
    let mut out = String::new();
    for (heading, rows) in [("accounts", &l.accounts), ("organizations", &l.orgs)] {
        let _ = writeln!(out, "\n{}", st.section(heading));
        if rows.is_empty() {
            let _ = writeln!(out, "  {}", st.dim("(none)"));
        }
        for row in rows {
            let source = match row.source {
                Some(Source::Manual) => st.ok("manual"),
                Some(Source::Learned) => st.dim("learned"),
                None => st.warn("unnamed"),
            };
            let name = row.name.clone().map_or_else(|| st.warn("(unnamed)"), |name| st.id(&name));
            let _ = writeln!(
                out,
                "  {}  {} {} {} sessions{}",
                short(&row.uuid),
                pad(&name, 34),
                pad(&source, 8),
                row.sessions,
                if row.is_active { st.ok("  <-") } else { String::new() }
            );
        }
    }
    let unnamed = l.accounts.iter().filter(|r| r.name.is_none()).count()
        + l.orgs.iter().filter(|r| r.name.is_none()).count();
    if unnamed > 0 {
        let _ = writeln!(
            out,
            "\n  {}\n",
            st.dim(&format!("{unnamed} unnamed. `unsilo label <id> <name>` to fix"))
        );
    } else {
        let _ = writeln!(out);
    }
    out
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

    #[test]
    fn truncation_keeps_the_end_of_a_path_and_the_start_of_prose() {
        assert_eq!(truncate("/a/very/long/path", 8, Keep::Tail), "..g/path");
        assert_eq!(truncate("a long title here", 8, Keep::Head), "a long..");
        assert_eq!(truncate("short", 8, Keep::Head), "short");
    }
}
