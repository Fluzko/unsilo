//! Snapshots: what goes in, what must never go in, and getting it back out.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use unsilo::env::{Env, FixedClock};
use unsilo::ops::snapshot as op;
use unsilo::snapshot::manifest::{EntryKind, Scope};
use unsilo::snapshot::{Options, read};
use unsilo_testkit::World;

const NOW: i64 = 1_787_602_036_145;
const SENTINEL: &str = "SENTINEL-a1b2c3d4";

fn world() -> World {
    World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s-one", |s| s.cwd("/home/u/proj").title("One").subagents(2));
                o.session("s-two", |s| s.cwd("/home/u/notes").title("Two"));
                o.session("s-gone", |s| s.cwd("/home/u/proj"));
                o.tombstone("s-gone");
            });
        })
        .active("acct", "org")
        .credentials_with_sentinel(SENTINEL)
        .build()
}

fn env(w: &World) -> Env {
    w.env().with_clock(Arc::new(FixedClock(NOW)))
}

#[test]
fn a_claude_snapshot_captures_transcripts_subagents_and_the_desktop_index() {
    let w = world();
    let written = op::run(&env(&w), Scope::Claude, "full", Options::default()).unwrap();
    let manifest = &written.manifest;

    assert_eq!(manifest.count(EntryKind::Transcript), 3);
    assert_eq!(manifest.count(EntryKind::Subagent), 2, "they belong to their conversation");
    assert_eq!(manifest.count(EntryKind::DesktopEntry), 3);
    assert_eq!(manifest.count(EntryKind::Tombstone), 1, "a deletion is state worth keeping");
    assert!(manifest.has_bodies);
    assert!(written.path.exists());
}

#[test]
fn no_snapshot_ever_carries_a_credential() {
    // The sentinel is planted in .credentials.json and in a session key file.
    let w = world();
    let written = op::run(&env(&w), Scope::Claude, "full", Options::default()).unwrap();

    let raw = std::fs::read(&written.path).unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains(SENTINEL), "a credential reached the archive");
    let bodies = read::read_bodies(&written.path).unwrap();
    for (name, bytes) in &bodies {
        assert!(!String::from_utf8_lossy(bytes).contains(SENTINEL), "leaked through {name}");
    }
    assert!(!bodies.keys().any(|k| k.contains("credentials")), "{:?}", bodies.keys());
}

#[test]
fn every_body_matches_the_hash_and_length_the_manifest_recorded() {
    let w = world();
    let written = op::run(&env(&w), Scope::Claude, "full", Options::default()).unwrap();
    let bodies = read::read_bodies(&written.path).unwrap();

    assert_eq!(bodies.len(), written.manifest.entries.len());
    for entry in &written.manifest.entries {
        let body = bodies.get(&entry.archive_path).expect(&entry.archive_path);
        assert_eq!(body.len() as u64, entry.len);
        assert_eq!(unsilo::fsx::hash_bytes(body), entry.sha256);
        // The origin is what makes a restore onto different paths possible.
        assert!(entry.origin.exists(), "{}", entry.origin);
    }
}

#[test]
fn a_metadata_only_snapshot_keeps_the_hashes_and_drops_the_bodies() {
    let w = world();
    let full = op::run(&env(&w), Scope::Claude, "full", Options::default()).unwrap();
    let light = op::run(&env(&w), Scope::Claude, "light", Options { with_bodies: false }).unwrap();

    assert_eq!(light.manifest.entries.len(), full.manifest.entries.len());
    assert!(!light.manifest.has_bodies);
    // Both carry the same manifest, so the difference is exactly the bodies.
    assert!(light.archive_bytes < full.archive_bytes);
    assert!(read::read_bodies(&light.path).unwrap().is_empty());
    assert!(!read::read_bodies(&full.path).unwrap().is_empty());
}

#[test]
fn a_store_snapshot_captures_unsilos_own_state_and_not_claudes() {
    let w = world();
    let env = env(&w);
    std::fs::create_dir_all(env.store_dir().join("transcripts")).unwrap();
    std::fs::write(env.store_dir().join("transcripts").join("x.jsonl"), b"{}\n").unwrap();

    let written = op::run(&env, Scope::Store, "store", Options::default()).unwrap();
    assert_eq!(written.manifest.scope, Scope::Store);
    assert_eq!(written.manifest.count(EntryKind::Transcript), 0);
    assert!(written.manifest.count(EntryKind::StoreFile) >= 1);
}

#[test]
fn a_store_snapshot_does_not_contain_earlier_snapshots() {
    let w = world();
    let env = env(&w);
    op::run(&env, Scope::Claude, "first", Options::default()).unwrap();
    let second = op::run(&env, Scope::Store, "second", Options::default()).unwrap();

    assert!(
        !second.manifest.entries.iter().any(|e| e.archive_path.contains("snapshots")),
        "snapshots of snapshots grow without bound"
    );
}

#[test]
fn reopening_a_snapshot_returns_the_manifest_it_was_written_with() {
    let w = world();
    let written = op::run(&env(&w), Scope::Claude, "full", Options::default()).unwrap();
    let reopened = read::open(&written.path).unwrap();

    assert_eq!(reopened.manifest.entries.len(), written.manifest.entries.len());
    assert_eq!(reopened.manifest.created_at_ms, NOW);
    assert_eq!(reopened.manifest.active.unwrap().account, w.account_uuid("acct"));
    assert_eq!(reopened.manifest.unsilo_version, unsilo::VERSION);
}

#[test]
fn snapshots_are_deterministic_for_an_unchanged_tree() {
    let w = world();
    let first = op::run(&env(&w), Scope::Claude, "a", Options::default()).unwrap();
    let second = op::run(&env(&w), Scope::Claude, "b", Options::default()).unwrap();

    let paths = |m: &unsilo::snapshot::Manifest| -> Vec<String> {
        m.entries.iter().map(|e| e.archive_path.clone()).collect()
    };
    assert_eq!(paths(&first.manifest), paths(&second.manifest));
    assert_eq!(
        std::fs::read(&first.path).unwrap(),
        std::fs::read(&second.path).unwrap(),
        "same tree, same bytes"
    );
}

#[test]
fn a_snapshot_taken_while_a_transcript_grows_is_internally_consistent() {
    use std::io::Write as _;
    let w = world();
    let path = w.transcript_path("s-one").unwrap();
    let written = op::run(&env(&w), Scope::Claude, "mid", Options::default()).unwrap();

    let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{{\"type\":\"user\",\"uuid\":\"after\"}}").unwrap();
    drop(file);

    let bodies = read::read_bodies(&written.path).unwrap();
    for entry in &written.manifest.entries {
        let body = &bodies[&entry.archive_path];
        assert_eq!(unsilo::fsx::hash_bytes(body), entry.sha256, "{}", entry.archive_path);
    }
    assert!(!bodies.values().any(|b| String::from_utf8_lossy(b).contains("after")));
}

#[test]
fn the_baseline_is_captured_once_and_never_replaced() {
    let w = world();
    let env = env(&w);

    let first = op::ensure_baseline(&env).unwrap().expect("captured on first call");
    let bytes = std::fs::read(&first.path).unwrap();
    assert!(op::ensure_baseline(&env).unwrap().is_none(), "already there");
    assert_eq!(std::fs::read(&first.path).unwrap(), bytes, "untouched");
}

#[test]
fn rotation_trims_automatic_snapshots_and_spares_named_ones() {
    let w = world();
    let mut env = env(&w);
    op::ensure_baseline(&env).unwrap();
    op::run(&env, Scope::Claude, "keepme", Options { with_bodies: false }).unwrap();

    for tick in 0..5 {
        env = env.with_clock(Arc::new(FixedClock(NOW + tick)));
        op::auto(&env, "apply").unwrap();
    }

    let removed = op::rotate_auto(&env, 2).unwrap();
    assert_eq!(removed, 3);

    let left: Vec<String> = std::fs::read_dir(env.snapshots_dir())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.iter().any(|n| n.starts_with("baseline")), "{left:?}");
    assert!(left.iter().any(|n| n.starts_with("keepme")), "{left:?}");
    assert_eq!(left.iter().filter(|n| n.starts_with("auto-")).count(), 2);
}

#[test]
fn a_name_that_could_escape_the_snapshot_directory_is_refused() {
    let w = world();
    let env = env(&w);
    for name in ["../escape", "a/b", "..", ""] {
        let err = op::run(&env, Scope::Claude, name, Options::default()).unwrap_err();
        assert_eq!(err.exit_code(), 2, "{name}");
    }
}

#[test]
fn opening_something_that_is_not_a_snapshot_says_so() {
    let w = world();
    let bogus = w.unsilo_home.join("not-a-snapshot.tar.zst");
    std::fs::write(&bogus, b"definitely not zstd").unwrap();
    assert!(read::open(&bogus).is_err());
}

#[test]
fn taking_a_snapshot_does_not_disturb_claudes_tree() {
    let w = world();
    let before = w.claude_digest();
    op::run(&env(&w), Scope::Claude, "full", Options::default()).unwrap();
    assert_eq!(before, w.claude_digest());
}
