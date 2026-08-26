//! The store and the ledger: the two pieces that make `off` possible.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use unsilo::env::{AlwaysCopy, Env, FixedClock, LinkKind};
use unsilo::index::Index;
use unsilo::ledger::{Disposition, KIND_DESKTOP_ENTRY, Ledger};
use unsilo::ops::ingest;
use unsilo::store::{Outcome, Store};
use unsilo::{claude::transcript, fsx};
use unsilo_testkit::World;

const NOW: i64 = 1_787_602_036_145;

fn world() -> World {
    World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s-one", |s| s.cwd("/home/u/proj").title("One"));
                o.session("s-two", |s| s.cwd("/home/u/proj").title("Two"));
            });
        })
        .active("acct", "org")
        .build()
}

fn env(w: &World) -> Env {
    w.env().with_clock(Arc::new(FixedClock(NOW)))
}

fn meta(w: &World, name: &str) -> transcript::Meta {
    transcript::parse(&w.transcript_path(name).unwrap()).unwrap().unwrap()
}

#[test]
fn ingesting_links_rather_than_moves() {
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    let source = w.transcript_path("s-one").unwrap();

    let ingested = store.ingest(&meta(&w, "s-one")).unwrap();

    assert_eq!(ingested.outcome, Outcome::Linked);
    assert_eq!(ingested.kind, LinkKind::Hard);
    assert!(source.exists(), "the original never moves");
    assert!(fsx::same_file(&source, &ingested.path), "one inode, two names");
}

#[test]
fn appends_by_claude_are_visible_through_the_stored_link() {
    use std::io::Write as _;
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    let stored = store.ingest(&meta(&w, "s-one")).unwrap().path;
    let before = std::fs::metadata(&stored).unwrap().len();

    let mut file =
        std::fs::OpenOptions::new().append(true).open(w.transcript_path("s-one").unwrap()).unwrap();
    writeln!(file, "{{\"type\":\"user\"}}").unwrap();
    drop(file);

    assert!(std::fs::metadata(&stored).unwrap().len() > before, "same inode, same bytes");
}

#[test]
fn ingesting_twice_is_a_no_op() {
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    store.ingest(&meta(&w, "s-one")).unwrap();
    assert_eq!(store.ingest(&meta(&w, "s-one")).unwrap().outcome, Outcome::AlreadyLinked);
}

#[test]
fn the_stored_transcript_survives_claudes_retention_cleanup() {
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    let stored = store.ingest(&meta(&w, "s-one")).unwrap().path;
    let bytes = std::fs::read(&stored).unwrap();

    assert!(w.simulate_retention_cleanup("s-one"));
    assert!(!w.transcript_path("s-one").unwrap().exists());

    // The link count never reached zero, so the data is still here.
    assert!(stored.exists());
    assert_eq!(std::fs::read(&stored).unwrap(), bytes);
    assert!(store.is_last_copy(&meta_id(&w, "s-one")), "and this is the only copy left");
}

fn meta_id(w: &World, name: &str) -> String {
    w.session_id(name).unwrap()
}

#[test]
fn a_cleaned_up_transcript_can_be_put_back_where_it_was() {
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    let original = w.transcript_path("s-one").unwrap();
    let meta = meta(&w, "s-one");
    store.ingest(&meta).unwrap();
    let bytes = std::fs::read(&original).unwrap();

    w.simulate_retention_cleanup("s-one");
    let restored = store.restore_into(&meta.session_id, &meta.origin_dir).unwrap();

    assert_eq!(restored.as_ref(), Some(&original));
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    assert!(!store.is_last_copy(&meta.session_id), "linked again, two copies");
}

#[test]
fn restoring_never_overwrites_what_is_already_there() {
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    let meta = meta(&w, "s-one");
    store.ingest(&meta).unwrap();

    assert_eq!(store.restore_into(&meta.session_id, &meta.origin_dir).unwrap(), None);
}

#[test]
fn a_longer_file_sharing_our_prefix_replaces_the_stored_copy() {
    use std::io::Write as _;
    let w = world();
    // AlwaysCopy so the store holds independent bytes, as it would on a volume
    // without hard links.
    let env = env(&w).with_linker(Arc::new(AlwaysCopy));
    let store = Store::new(&env);
    store.ingest(&meta(&w, "s-one")).unwrap();

    let mut file =
        std::fs::OpenOptions::new().append(true).open(w.transcript_path("s-one").unwrap()).unwrap();
    writeln!(file, "{{\"type\":\"user\",\"uuid\":\"later\"}}").unwrap();
    drop(file);

    let ingested = store.ingest(&meta(&w, "s-one")).unwrap();
    assert_eq!(ingested.outcome, Outcome::Refreshed, "append only means this is the same history");
    assert!(std::fs::read_to_string(&ingested.path).unwrap().contains("later"));
}

#[test]
fn a_divergent_history_is_kept_rather_than_replaced() {
    let w = world();
    let env = env(&w).with_linker(Arc::new(AlwaysCopy));
    let store = Store::new(&env);
    let stored = store.ingest(&meta(&w, "s-one")).unwrap().path;
    let original_bytes = std::fs::read(&stored).unwrap();

    // Same id, unrelated content: not an append, so not the same conversation.
    std::fs::write(w.transcript_path("s-one").unwrap(), b"{\"type\":\"user\",\"cwd\":\"/x\"}\n")
        .unwrap();

    let ingested = store.ingest(&meta(&w, "s-one")).unwrap();
    assert_eq!(ingested.outcome, Outcome::Diverged);
    assert_eq!(std::fs::read(&stored).unwrap(), original_bytes, "the store keeps what it had");
}

#[test]
fn the_copy_fallback_works_end_to_end() {
    let w = world();
    let env = env(&w).with_linker(Arc::new(AlwaysCopy));
    let index = Index::open_in_memory().unwrap();
    let summary = ingest::run(&env, &index).unwrap();

    assert_eq!(summary.sessions, 2);
    assert_eq!(summary.copies, 2, "no hard links available in this configuration");
    assert!(summary.diverged.is_empty());
    assert!(Store::new(&env).holds(&meta_id(&w, "s-one")));
}

#[test]
fn the_store_refuses_to_write_outside_its_roots() {
    let w = world();
    let mut env = env(&w);
    // A store pointed somewhere it has no claim to.
    env.unsilo_home = w.root.join("elsewhere");
    let guard = env.guard();
    assert!(guard.check(&w.root.join("outside").join("x")).is_err());
    assert!(guard.check(&env.unsilo_home.join("store").join("x")).is_ok());
}

#[test]
fn the_ledger_records_a_write_before_it_happens() {
    let w = world();
    let env = env(&w);
    let index = Index::open_in_memory().unwrap();
    let ledger = Ledger::new(&index);
    let target = env.unsilo_home.join("projected.json");

    ledger.begin(&target, KIND_DESKTOP_ENTRY, Some("sess"), Some("local_x"), NOW).unwrap();
    let pending = ledger.entries().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].state, "pending");
    assert!(pending[0].content_hash.is_none(), "nothing to hash yet, the file is not written");
}

#[test]
fn a_crash_between_the_row_and_the_write_is_reconciled() {
    let w = world();
    let env = env(&w);
    let index = Index::open_in_memory().unwrap();
    let ledger = Ledger::new(&index);

    let written = env.unsilo_home.join("written.json");
    let never = env.unsilo_home.join("never.json");
    ledger.begin(&written, KIND_DESKTOP_ENTRY, None, None, NOW).unwrap();
    ledger.begin(&never, KIND_DESKTOP_ENTRY, None, None, NOW).unwrap();
    fsx::write_atomic(&env.guard(), &written, b"{}").unwrap();

    let reconciled = ledger.reconcile().unwrap();

    assert_eq!(reconciled.completed, 1, "the file made it, so the row is ours");
    assert_eq!(reconciled.dropped, 1, "the file never appeared, so the row is not");
    let entries = ledger.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].state, "done");
}

#[test]
fn the_ledger_tells_off_what_is_safe_to_remove() {
    let w = world();
    let env = env(&w);
    let index = Index::open_in_memory().unwrap();
    let ledger = Ledger::new(&index);

    let untouched = env.unsilo_home.join("untouched.json");
    let rewritten = env.unsilo_home.join("rewritten.json");
    let deleted = env.unsilo_home.join("deleted.json");
    for (path, bytes) in
        [(&untouched, &b"{\"a\":1}"[..]), (&rewritten, b"{\"a\":1}"), (&deleted, b"{}")]
    {
        ledger.begin(path, KIND_DESKTOP_ENTRY, None, None, NOW).unwrap();
        fsx::write_atomic(&env.guard(), path, bytes).unwrap();
        ledger.commit(path, bytes).unwrap();
    }

    // The real case: Claude rewrote a projected entry when the session was
    // resumed under the account it was projected into.
    std::fs::write(&rewritten, b"{\"a\":2,\"lastActivityAt\":9}").unwrap();
    std::fs::remove_file(&deleted).unwrap();

    let judged = ledger.judge().unwrap();
    let by_path = |name: &str| {
        judged.iter().find(|j| j.entry.path.file_name() == Some(name)).unwrap().disposition
    };

    assert_eq!(by_path("untouched.json"), Disposition::Removable);
    assert_eq!(by_path("rewritten.json"), Disposition::Modified, "not ours to delete any more");
    assert_eq!(by_path("deleted.json"), Disposition::Missing);
}

#[test]
fn an_unconfirmed_row_is_never_treated_as_removable() {
    let w = world();
    let env = env(&w);
    let index = Index::open_in_memory().unwrap();
    let ledger = Ledger::new(&index);
    let path = env.unsilo_home.join("half.json");

    ledger.begin(&path, KIND_DESKTOP_ENTRY, None, None, NOW).unwrap();
    fsx::write_atomic(&env.guard(), &path, b"{}").unwrap();

    // Begun but never committed: we cannot prove those bytes are ours.
    assert_eq!(ledger.judge().unwrap()[0].disposition, Disposition::Modified);
}

#[test]
fn the_link_count_is_readable_on_every_platform() {
    // is_last_copy is what tells apply a transcript needs relinking, so a
    // platform that cannot answer would silently never relink anything.
    let w = world();
    let env = env(&w);
    let store = Store::new(&env);
    let id = meta_id(&w, "s-one");
    store.ingest(&meta(&w, "s-one")).unwrap();

    assert_eq!(fsx::link_count(&store.path_for(&id)), Some(2), "project dir plus store");
    assert!(!store.is_last_copy(&id));

    w.simulate_retention_cleanup("s-one");
    assert_eq!(fsx::link_count(&store.path_for(&id)), Some(1));
    assert!(store.is_last_copy(&id));
}

#[test]
fn purging_closes_the_database_before_deleting_it() {
    // Windows refuses to remove an open file, so a purge that holds its own
    // connection open works only on unix.
    let w = world();
    let env = env(&w);
    let index = Index::open(&env.index_path()).unwrap();
    ingest::run(&env, &index).unwrap();
    drop(index);
    unsilo::ops::snapshot::run(
        &env,
        unsilo::snapshot::Scope::Store,
        "safety",
        unsilo::snapshot::Options::default(),
    )
    .unwrap();

    unsilo::ops::off::run(&env, &unsilo::ops::off::Options { purge: true, dry_run: false })
        .unwrap();

    assert!(!env.store_dir().exists());
    assert!(!env.index_path().exists());
    for sidecar in ["index.sqlite-wal", "index.sqlite-shm"] {
        assert!(!env.unsilo_home.join(sidecar).exists(), "{sidecar} left behind");
    }
}
