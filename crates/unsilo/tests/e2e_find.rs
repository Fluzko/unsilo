//! `find` end to end, including the output formats other tools consume.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use assert_cmd::Command;
use unsilo_testkit::World;

fn world() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix the auth bug"));
                o.session("s-side", |s| s.cwd("/home/u/proj").sidechain());
            });
        })
        .account("acct-personal", "me@example.com", |a| {
            a.org("org-personal", "Personal", |o| {
                o.session("s-notes", |s| s.cwd("/home/u/notes").title("Grocery notes"));
            });
        })
        .active("acct-personal", "org-personal")
        .build()
}

fn unsilo(w: &World) -> Command {
    let mut cmd = Command::cargo_bin("unsilo").expect("binary built");
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
fn find_with_no_arguments_lists_everything_visible() {
    let w = world();
    let text = stdout(&w, &["find"]);
    assert!(text.contains("Fix the auth bug"), "{text}");
    assert!(text.contains("Grocery notes"), "{text}");
    assert!(text.contains("2 of 3 sessions"), "{text}");
}

#[test]
fn a_query_narrows_by_content() {
    let w = world();
    let text = stdout(&w, &["find", "grocery"]);
    assert!(text.contains("Grocery notes"), "{text}");
    assert!(!text.contains("Fix the auth bug"), "{text}");
}

#[test]
fn the_account_column_uses_the_email_when_it_is_known() {
    let w = world();
    let text = stdout(&w, &["find"]);
    assert!(text.contains("me@example.com"), "the active account resolves: {text}");
}

#[test]
fn filtering_by_an_unknown_email_is_a_usage_error_not_an_empty_list() {
    // An empty result here would make `apply --email typo` prune everything.
    let w = world();
    unsilo(&w).args(["find", "--email", "nobody@example.com"]).assert().failure().code(2);
}

#[test]
fn no_matches_exits_with_its_own_code() {
    let w = world();
    unsilo(&w).args(["find", "kumquat"]).assert().failure().code(5);
}

#[test]
fn the_paths_format_emits_one_transcript_path_per_line() {
    let w = world();
    let text = stdout(&w, &["find", "--format", "paths"]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    for line in lines {
        let path = camino::Utf8Path::new(line);
        assert_eq!(path.extension(), Some("jsonl"), "{line}");
        assert!(path.exists(), "{line} should be a real file");
    }
}

#[test]
fn the_resume_format_emits_a_command_that_would_reopen_the_session() {
    let w = world();
    let id = w.session_id("s-auth").unwrap();
    let text = stdout(&w, &["find", "--id", &id[..8], "--format", "resume"]);
    assert_eq!(text.trim(), format!("cd /home/u/proj && claude --resume {id}"));
}

#[test]
fn the_json_format_is_a_versioned_document_with_scopes_resolved() {
    let w = world();
    let json: serde_json::Value =
        serde_json::from_str(&stdout(&w, &["find", "--format", "json"])).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["matched"], 2);

    let rows = json["rows"].as_array().unwrap();
    let auth = rows.iter().find(|r| r["title"] == "Fix the auth bug").unwrap();
    assert_eq!(auth["scopes"][0]["account"], w.account_uuid("acct-work"));
    assert_eq!(json["identities"]["emails"][w.account_uuid("acct-personal")], "me@example.com");
}

#[test]
fn hidden_sessions_stay_hidden_unless_asked_for() {
    let w = world();
    let plain: serde_json::Value =
        serde_json::from_str(&stdout(&w, &["find", "--format", "json"])).unwrap();
    assert_eq!(plain["rows"].as_array().unwrap().len(), 2);

    let all: serde_json::Value =
        serde_json::from_str(&stdout(&w, &["find", "--include-hidden", "--format", "json"]))
            .unwrap();
    let rows = all["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|r| r["hidden_reason"] == "isSidechain"));
}

#[test]
fn an_unknown_sort_or_surface_is_refused_before_anything_runs() {
    let w = world();
    unsilo(&w).args(["find", "--sort", "sideways"]).assert().failure().code(2);
    unsilo(&w).args(["find", "--surface", "telepathy"]).assert().failure().code(2);
}

#[test]
fn find_never_touches_claudes_tree() {
    let w = world();
    let before = w.claude_digest();
    unsilo(&w).arg("find").assert().success();
    let after = w.claude_digest();
    assert_eq!(before, after, "changed {:?}", before.changed_in(&after));
}
