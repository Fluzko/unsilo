//! The invariant the whole tool rests on: apply then off is the identity.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use assert_cmd::Command;
use std::sync::Arc;
use unsilo::env::{AlwaysCopy, Env, FixedClock};
use unsilo::filter::Filter;
use unsilo::ops::{apply, off};
use unsilo_testkit::World;

const NOW: i64 = 1_787_602_036_145;

/// Two accounts, three conversations, one of them deleted from its own list.
fn world() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix auth"));
                o.session("s-deploy", |s| s.cwd("/home/u/proj").title("Deploy"));
                o.session("s-gone", |s| s.cwd("/home/u/proj").title("Deleted"));
                o.tombstone("s-gone");
            });
        })
        .account("acct-personal", "me@example.com", |a| {
            a.org("org-personal", "Personal", |o| {
                o.session("s-notes", |s| s.cwd("/home/u/notes").title("Notes"));
            });
        })
        .active("acct-personal", "org-personal")
        .build()
}

fn env(w: &World) -> Env {
    w.env().with_clock(Arc::new(FixedClock(NOW)))
}

fn ids(w: &World) -> unsilo::claude::identity::Identities {
    let mut ids = unsilo::claude::identity::Identities::default();
    ids.learn_from(&w.home);
    ids.set_manual_account(&w.account_uuid("acct-work"), "work@example.com");
    ids.save(&w.unsilo_home.join("identities.json")).unwrap();
    ids
}

fn active_dir(w: &World) -> camino::Utf8PathBuf {
    w.user_data
        .join("claude-code-sessions")
        .join(w.account_uuid("acct-personal"))
        .join(w.org_uuid("org-personal"))
}

fn entries_in(dir: &camino::Utf8Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("local_"))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- the invariant

#[test]
fn apply_then_off_restores_the_tree_exactly() {
    let w = world();
    let env = env(&w);
    ids(&w);
    let before = w.claude_digest();

    let applied = apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    // Without this the test would also pass for an apply that does nothing.
    assert!(applied.changes() > 0, "apply made no changes, the test proves nothing");
    assert_ne!(before, w.claude_digest());

    off::run(&env, &off::Options::default()).unwrap();
    let after = w.claude_digest();

    assert_eq!(
        before,
        after,
        "added {:?}, changed {:?}, removed {:?}",
        before.added_in(&after),
        before.changed_in(&after),
        before.removed_in(&after)
    );
}

#[test]
fn the_identity_holds_with_copies_instead_of_hard_links() {
    let w = world();
    let env = env(&w).with_linker(Arc::new(AlwaysCopy));
    ids(&w);
    let before = w.claude_digest();

    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    off::run(&env, &off::Options::default()).unwrap();

    assert_eq!(before, w.claude_digest());
}

#[test]
fn the_identity_holds_after_a_retention_cleanup_and_relink() {
    let w = world();
    let env = env(&w);
    ids(&w);
    // Prime the store while the transcript is still in place.
    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    off::run(&env, &off::Options::default()).unwrap();
    let before = w.claude_digest();

    assert!(w.simulate_retention_cleanup("s-auth"));
    let applied = apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    assert_eq!(applied.relinked.len(), 1, "the store still had it");

    off::run(&env, &off::Options::default()).unwrap();
    assert_eq!(before, w.claude_digest(), "including the transcript that was cleaned up");
}

// ------------------------------------------------------------------- behaviour

#[test]
fn a_session_from_another_account_becomes_visible_under_the_active_one() {
    let w = world();
    let env = env(&w);
    ids(&w);
    assert_eq!(entries_in(&active_dir(&w)).len(), 1, "only its own to begin with");

    let report = apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();

    assert_eq!(report.projected.len(), 2, "auth and deploy; the deleted one stays deleted");
    let names = entries_in(&active_dir(&w));
    assert_eq!(names.len(), 3);
    assert!(report.projected.iter().all(|p| p.stripped_bytes > 0), "the MCP payload is dropped");
}

#[test]
fn a_projected_entry_keeps_its_identity_and_loses_only_the_account_scoped_payload() {
    let w = world();
    ids(&w);
    apply::run(&env(&w), &Filter::default(), &apply::Options::default()).unwrap();

    let auth = w.session_id("s-auth").unwrap();
    let file = entries_in(&active_dir(&w))
        .into_iter()
        .map(|n| active_dir(&w).join(n))
        .find(|p| std::fs::read_to_string(p).unwrap().contains(&auth))
        .expect("the projected entry");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();

    assert_eq!(value["cliSessionId"], auth);
    assert_eq!(value["title"], "Fix auth");
    assert!(value.get("remoteMcpServersConfig").is_none());
    assert!(value.get("enabledMcpTools").is_none());
}

#[test]
fn a_session_deleted_from_its_own_list_is_not_selected_by_default() {
    let w = world();
    ids(&w);
    let report = apply::run(&env(&w), &Filter::default(), &apply::Options::default()).unwrap();

    let gone = w.session_id("s-gone").unwrap();
    assert!(!report.projected.iter().any(|p| p.session_id.as_deref() == Some(gone.as_str())));
}

#[test]
fn a_tombstone_in_the_target_list_blocks_projection_even_when_selected() {
    // The user deleted this session from the account they are signed in as. A
    // deletion in one list says nothing about another, but it says everything
    // about its own.
    let w = World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix auth"));
                o.session("s-deploy", |s| s.cwd("/home/u/proj").title("Deploy"));
            });
        })
        .account("acct-personal", "me@example.com", |a| {
            a.org("org-personal", "Personal", |o| {
                o.tombstone("s-auth");
            });
        })
        .active("acct-personal", "org-personal")
        .build();
    ids(&w);

    let report = apply::run(&env(&w), &Filter::default(), &apply::Options::default()).unwrap();

    let auth = w.session_id("s-auth").unwrap();
    let deploy = w.session_id("s-deploy").unwrap();
    assert!(!report.projected.iter().any(|p| p.session_id.as_deref() == Some(auth.as_str())));
    assert!(report.projected.iter().any(|p| p.session_id.as_deref() == Some(deploy.as_str())));
}

#[test]
fn apply_is_idempotent() {
    let w = world();
    let env = env(&w);
    ids(&w);
    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    let after_first = w.claude_digest();

    let second = apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();

    assert_eq!(second.changes(), 0);
    assert_eq!(after_first, w.claude_digest());
}

#[test]
fn apply_is_declarative_so_narrowing_the_filter_removes_what_no_longer_belongs() {
    let w = world();
    let env = env(&w);
    ids(&w);
    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    assert_eq!(entries_in(&active_dir(&w)).len(), 3);

    let narrowed = Filter { emails: vec!["me@example.com".to_owned()], ..Filter::default() };
    let report = apply::run(&env, &narrowed, &apply::Options::default()).unwrap();

    assert_eq!(report.pruned.len(), 2, "what the filter no longer selects is taken back out");
    assert_eq!(entries_in(&active_dir(&w)).len(), 1);
}

#[test]
fn off_keeps_an_entry_claude_rewrote_after_we_projected_it() {
    let w = world();
    let env = env(&w);
    ids(&w);
    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();

    // What happens when a projected session is resumed under the account it was
    // projected into.
    let index = unsilo::index::Index::open(&env.index_path()).unwrap();
    let ledger = unsilo::ledger::Ledger::new(&index);
    let projected = ledger.entries().unwrap().first().unwrap().path.clone();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&projected).unwrap()).unwrap();
    value["completedTurns"] = serde_json::json!(42);
    std::fs::write(&projected, serde_json::to_vec(&value).unwrap()).unwrap();

    let report = off::run(&env, &off::Options::default()).unwrap();

    assert!(report.kept.contains(&projected), "not ours to delete any more");
    assert!(projected.exists());
}

#[test]
fn off_never_touches_the_store() {
    let w = world();
    let env = env(&w);
    ids(&w);
    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    let store = env.store_dir().join("transcripts");
    let before = std::fs::read_dir(&store).unwrap().flatten().count();

    let report = off::run(&env, &off::Options::default()).unwrap();

    assert_eq!(report.store_transcripts, before);
    assert!(!report.purged);
    assert_eq!(std::fs::read_dir(&store).unwrap().flatten().count(), before);
}

#[test]
fn purge_is_refused_without_a_store_snapshot_and_allowed_with_one() {
    let w = world();
    let env = env(&w);
    ids(&w);
    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();

    let err = off::run(&env, &off::Options { purge: true, ..off::Options::default() }).unwrap_err();
    assert!(err.to_string().contains("store snapshot"), "{err}");
    assert!(env.store_dir().exists(), "nothing was destroyed");

    unsilo::ops::snapshot::run(
        &env,
        unsilo::snapshot::Scope::Store,
        "safety",
        unsilo::snapshot::Options::default(),
    )
    .unwrap();
    off::run(&env, &off::Options { purge: true, ..off::Options::default() }).unwrap();
    assert!(!env.store_dir().exists());
}

#[test]
fn a_dry_run_exits_with_the_pending_code_and_writes_nothing() {
    let w = world();
    let env = env(&w);
    ids(&w);
    let before = w.claude_digest();

    let report = apply::run(
        &env,
        &Filter::default(),
        &apply::Options { dry_run: true, ..apply::Options::default() },
    )
    .unwrap();

    assert!(report.changes() > 0, "the plan is reported, not swallowed by an error");
    assert!(report.dry_run);
    assert_eq!(before, w.claude_digest(), "and nothing was written");
}

#[test]
fn apply_refuses_to_write_when_the_layout_is_not_recognised() {
    let w = World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s", |s| s.cwd("/home/u/p"));
            });
        })
        .active("acct", "org")
        .hover_rest(true)
        .build();
    let before = w.claude_digest();

    let err = apply::run(&env(&w), &Filter::default(), &apply::Options::default()).unwrap_err();

    assert_eq!(err.exit_code(), 3);
    assert_eq!(before, w.claude_digest());
}

#[test]
fn a_crash_between_the_ledger_row_and_the_write_leaves_a_recoverable_tree() {
    let w = world();
    let env = env(&w);
    ids(&w);
    let before = w.claude_digest();

    let failing = env.clone();
    let failing = Env {
        failpoints: Arc::new(unsilo::env::NamedFailpoint("apply.after_ledger_pending".to_owned())),
        ..failing
    };
    assert!(apply::run(&failing, &Filter::default(), &apply::Options::default()).is_err());

    // The next run settles the pending row, and off gets back to where we began.
    off::run(&env, &off::Options::default()).unwrap();
    assert_eq!(before, w.claude_digest());
}

#[test]
fn apply_takes_a_baseline_before_it_writes_anything() {
    let w = world();
    let env = env(&w);
    ids(&w);
    let report = apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();

    assert!(report.baseline_created);
    assert!(env.snapshots_dir().join("baseline.tar.zst").exists());
    assert!(report.auto_snapshot.is_some_and(|p| p.exists()));
}

// ------------------------------------------------------------------------ e2e

#[test]
fn the_binary_applies_and_turns_off_from_the_command_line() {
    let w = world();
    ids(&w);
    let before = w.claude_digest();

    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    cmd.arg("apply").assert().success();
    assert_ne!(before, w.claude_digest());

    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    cmd.arg("off").assert().success();
    assert_eq!(before, w.claude_digest());
}

#[test]
fn a_dry_run_prints_its_plan_and_still_signals_pending_work() {
    let w = world();
    ids(&w);
    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    let out = cmd.args(["apply", "--dry-run"]).assert().failure().code(4);
    let text = String::from_utf8_lossy(&out.get_output().stdout).into_owned();

    // The plan has to be visible; an exit code alone is not a preview.
    assert!(text.contains("dry run"), "{text}");
    assert!(text.contains("Fix auth"), "{text}");
}

#[test]
fn doctor_reports_what_apply_actually_left_behind() {
    let w = world();
    // Before: three of the four sessions cannot be seen from the active account.
    let env = env(&w);
    ids(&w);
    assert_eq!(unsilo::ops::doctor::run(&env).unwrap().invisible_under_active, 3);

    apply::run(&env, &Filter::default(), &apply::Options::default()).unwrap();
    let report = unsilo::ops::doctor::run(&env).unwrap();

    assert_eq!(report.store.ledger_entries, 2, "the two projected entries");
    // Everything is visible now except the one the user deleted from its own
    // list, which apply leaves alone on purpose.
    assert_eq!(report.invisible_under_active, 1);
    assert!(report.store.transcripts >= 4);
}
