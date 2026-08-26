//! Restore, and the append-only rule that makes its conflicts exact.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::Write as _;
use std::sync::Arc;
use unsilo::env::{Env, FixedClock};
use unsilo::ops::restore::{self, Options, Verdict};
use unsilo::ops::snapshot as snap;
use unsilo::snapshot::{Options as SnapOptions, Scope};
use unsilo_testkit::World;

const NOW: i64 = 1_787_602_036_145;

fn world() -> World {
    World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s-one", |s| s.cwd("/home/u/proj").title("One"));
                o.session("s-two", |s| s.cwd("/home/u/notes").title("Two"));
            });
        })
        .active("acct", "org")
        .build()
}

fn env(w: &World) -> Env {
    w.env().with_clock(Arc::new(FixedClock(NOW)))
}

fn take(w: &World, name: &str) {
    snap::run(&env(w), Scope::Claude, name, SnapOptions::default()).unwrap();
}

fn verdict(report: &restore::Report, needle: &str) -> Verdict {
    report
        .planned
        .iter()
        .find(|p| p.target.as_str().contains(needle))
        .unwrap_or_else(|| panic!("no plan for {needle}"))
        .verdict
}

#[test]
fn a_deleted_transcript_comes_back_byte_for_byte() {
    let w = world();
    let path = w.transcript_path("s-one").unwrap();
    let original = std::fs::read(&path).unwrap();
    take(&w, "before");

    std::fs::remove_file(&path).unwrap();
    let report = restore::run(&env(&w), "before", &Options::default()).unwrap();

    assert_eq!(report.restored, 1);
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[test]
fn an_untouched_tree_restores_to_nothing() {
    let w = world();
    take(&w, "before");
    let before = w.claude_digest();

    let report = restore::run(&env(&w), "before", &Options::default()).unwrap();

    assert_eq!(report.restored, 0);
    assert!(report.planned.iter().all(|p| p.verdict == Verdict::Identical));
    assert_eq!(before, w.claude_digest(), "restoring a current snapshot is a no-op");
}

#[test]
fn a_transcript_that_grew_since_the_snapshot_is_left_alone() {
    let w = world();
    take(&w, "before");
    let path = w.transcript_path("s-one").unwrap();

    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{{\"type\":\"user\",\"uuid\":\"later\"}}").unwrap();
    drop(file);

    let report = restore::run(&env(&w), "before", &Options::default()).unwrap();

    // Append only: the local file starts with the snapshot's bytes, so it is the
    // same conversation further along. No merge, no date heuristic.
    assert_eq!(verdict(&report, w.session_id("s-one").unwrap().as_str()), Verdict::LocalIsNewer);
    assert_eq!(report.restored, 0);
    assert!(std::fs::read_to_string(&path).unwrap().contains("later"), "kept");
}

#[test]
fn a_divergent_history_stops_the_restore_instead_of_guessing() {
    let w = world();
    take(&w, "before");
    let path = w.transcript_path("s-one").unwrap();
    std::fs::write(&path, b"{\"type\":\"user\",\"cwd\":\"/elsewhere\"}\n").unwrap();
    let local = std::fs::read(&path).unwrap();

    let err = restore::run(&env(&w), "before", &Options::default()).unwrap_err();
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains("diverge"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), local, "nothing was written");
}

#[test]
fn conflicts_can_be_skipped_or_overwritten_but_only_on_request() {
    let w = world();
    take(&w, "before");
    let path = w.transcript_path("s-one").unwrap();
    let snapshotted = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"{\"type\":\"user\",\"cwd\":\"/elsewhere\"}\n").unwrap();
    let diverged = std::fs::read(&path).unwrap();

    let skipped =
        restore::run(&env(&w), "before", &Options { skip_conflicts: true, ..Options::default() })
            .unwrap();
    assert_eq!(skipped.conflicts, 1);
    assert_eq!(std::fs::read(&path).unwrap(), diverged, "skipping leaves it");

    restore::run(&env(&w), "before", &Options { force: true, ..Options::default() }).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), snapshotted, "force replaces it");
}

#[test]
fn a_dry_run_reports_its_plan_and_writes_nothing() {
    let w = world();
    take(&w, "before");
    let path = w.transcript_path("s-one").unwrap();
    std::fs::remove_file(&path).unwrap();
    let before = w.claude_digest();

    let report =
        restore::run(&env(&w), "before", &Options { dry_run: true, ..Options::default() }).unwrap();

    assert_eq!(report.restored, 1, "the plan is reported, not swallowed by an error");
    assert_eq!(before, w.claude_digest());
    assert!(!path.exists());
}

#[test]
fn a_desktop_entry_removed_from_its_account_can_be_put_back() {
    let w = world();
    take(&w, "before");
    let dir = w
        .user_data
        .join("claude-code-sessions")
        .join(w.account_uuid("acct"))
        .join(w.org_uuid("org"));
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("local_")))
        .collect();
    std::fs::remove_file(&entries[0]).unwrap();

    let report = restore::run(&env(&w), "before", &Options::default()).unwrap();

    assert_eq!(report.restored, 1);
    assert!(entries[0].exists(), "visibility restored");
}

#[test]
fn a_snapshot_from_another_machine_lands_under_our_own_roots() {
    let source = world();
    take(&source, "portable");
    let archive = source.unsilo_home.join("snapshots").join("portable.tar.zst");

    // A second installation with different absolute paths.
    let target = world();
    for name in ["s-one", "s-two"] {
        std::fs::remove_file(target.transcript_path(name).unwrap()).unwrap();
    }
    let copied = target.unsilo_home.join("snapshots").join("portable.tar.zst");
    std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
    std::fs::copy(&archive, &copied).unwrap();

    let report = restore::run(&env(&target), "portable", &Options::default()).unwrap();

    assert!(report.restored >= 2);
    for name in ["s-one", "s-two"] {
        let path = target.transcript_path(name).unwrap();
        assert!(path.exists(), "{path}");
        // Re-rooted under this machine's home, never written to the source's.
        assert!(path.starts_with(&target.home), "{path}");
    }
    assert!(
        !source.transcript_path("s-one").unwrap().to_string().starts_with(target.home.as_str())
    );
}

#[test]
fn a_metadata_only_snapshot_cannot_be_restored_from() {
    let w = world();
    snap::run(&env(&w), Scope::Claude, "light", SnapOptions { with_bodies: false }).unwrap();
    let err = restore::run(&env(&w), "light", &Options::default()).unwrap_err();
    assert!(err.to_string().contains("metadata only"), "{err}");
}

#[test]
fn restoring_the_baseline_undoes_a_deletion_spree() {
    let w = world();
    let env = env(&w);
    snap::ensure_baseline(&env).unwrap();
    let before = w.claude_digest();

    for name in ["s-one", "s-two"] {
        std::fs::remove_file(w.transcript_path(name).unwrap()).unwrap();
    }
    restore::run(&env, "baseline", &Options::default()).unwrap();

    assert_eq!(before, w.claude_digest(), "the baseline is what off returns to");
}
