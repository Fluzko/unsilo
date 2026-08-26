//! End to end against the real binary. Identical assertions on every platform:
//! fixture cwds are posix strings and project dir names are alphanumerics and
//! dashes, so the tree is byte identical on Linux, macOS and Windows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use assert_cmd::Command;
use unsilo_testkit::World;

fn world() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix auth"));
                o.session("s-deploy", |s| s.cwd("/home/u/proj").title("Deploy"));
                o.session("s-side", |s| s.cwd("/home/u/proj").sidechain());
            });
        })
        .account("acct-personal", "me@example.com", |a| {
            a.org("org-personal", "Personal", |o| {
                o.session("s-notes", |s| s.cwd("/home/u/notes").title("Notes").subagents(2));
            });
        })
        .active("acct-personal", "org-personal")
        .build()
}

fn unsilo(w: &World) -> Command {
    let mut cmd = Command::cargo_bin("unsilo").expect("binary built");
    // A bare env plus the fixture roots: nothing from the developer's own machine
    // can leak in and change what the test sees.
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    cmd
}

fn stdout(w: &World, args: &[&str]) -> String {
    let out = unsilo(w).args(args).assert().success();
    String::from_utf8_lossy(&out.get_output().stdout).into_owned()
}

#[test]
fn doctor_reports_the_fixture_roots_and_not_the_real_machine() {
    let w = world();
    let text = stdout(&w, &["doctor"]);

    assert!(text.contains(w.config_dir.as_str()), "{text}");
    assert!(text.contains(w.user_data.as_str()), "{text}");
    assert!(text.contains(w.unsilo_home.as_str()), "{text}");
}

#[test]
fn doctor_counts_conversations_apart_from_subagents_and_hidden_sessions() {
    let w = world();
    let json: serde_json::Value = serde_json::from_str(&stdout(&w, &["doctor", "--json"])).unwrap();
    let dir = &json["config_dirs"][0];

    assert_eq!(dir["conversations"], 3, "four transcripts, one of them a sidechain");
    assert_eq!(dir["hidden"]["isSidechain"], 1);
    assert_eq!(dir["subagents"], 2, "nested subagents are never conversations");
    assert_eq!(dir["project_dirs"], 2);
}

#[test]
fn doctor_names_the_sessions_the_active_account_cannot_see() {
    let w = world();
    let json: serde_json::Value = serde_json::from_str(&stdout(&w, &["doctor", "--json"])).unwrap();

    // The whole reason the tool exists, as one number.
    assert_eq!(json["invisible_under_active"], 3);
    assert_eq!(json["active"]["account"], w.account_uuid("acct-personal"));
    assert_eq!(json["active"]["email"], "me@example.com");
    assert_eq!(json["writes_allowed"], true);

    let text = stdout(&w, &["doctor"]);
    assert!(text.contains("3 desktop sessions NOT visible"), "{text}");
}

#[test]
fn only_the_active_account_gets_an_email_from_local_config() {
    let w = world();
    let json: serde_json::Value = serde_json::from_str(&stdout(&w, &["doctor", "--json"])).unwrap();
    let unresolved = json["unresolved_accounts"].as_array().unwrap();

    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0], w.account_uuid("acct-work"));
}

#[test]
fn the_remote_backend_blocks_writes_but_not_reading() {
    let w = World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s", |s| s.cwd("/home/u/p"));
            });
        })
        .hover_rest(true)
        .build();

    let json: serde_json::Value = serde_json::from_str(&stdout(&w, &["doctor", "--json"])).unwrap();
    assert_eq!(json["remote_backend"], true);
    assert_eq!(json["writes_allowed"], false);
    assert!(json["compat_reason"].as_str().unwrap().contains("tengu_hover_rest"));
    assert_eq!(json["config_dirs"][0]["conversations"], 1, "reading still works");
}

#[test]
fn strict_fails_on_a_warning_and_passes_on_a_clean_install() {
    let blocked = World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s", |s| s.cwd("/home/u/p"));
            });
        })
        .hover_rest(true)
        .build();
    unsilo(&blocked).args(["doctor", "--strict"]).assert().failure();

    let clean = World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s", |s| s.cwd("/home/u/p"));
            });
        })
        .active("acct", "org")
        .build();
    unsilo(&clean).args(["doctor", "--strict"]).assert().success();
}

#[test]
fn doctor_writes_nothing() {
    let w = world();
    let before = w.digest();
    unsilo(&w).arg("doctor").assert().success();
    let after = w.digest();

    assert_eq!(
        before,
        after,
        "doctor is read only; added {:?}, changed {:?}, removed {:?}",
        before.added_in(&after),
        before.changed_in(&after),
        before.removed_in(&after)
    );
}

#[test]
fn the_json_output_is_a_versioned_document() {
    let w = world();
    let json: serde_json::Value = serde_json::from_str(&stdout(&w, &["doctor", "--json"])).unwrap();
    assert_eq!(json["schema"], 1);
    assert!(json["unsilo_version"].is_string());
}

#[test]
fn an_unknown_command_exits_with_the_usage_code() {
    let w = world();
    unsilo(&w).arg("nope").assert().failure().code(2);
}

#[test]
fn a_missing_home_fails_instead_of_guessing() {
    let mut cmd = Command::cargo_bin("unsilo").expect("binary built");
    cmd.env_clear();
    cmd.arg("doctor").assert().failure().code(1);
}
