//! The fixture builder is load bearing for every other test, so it gets its own.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use unsilo_testkit::{TreeDigest, World};

fn two_accounts() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix auth"));
                o.session("s-old", |s| s.cwd("/home/u/proj").modified("2026-01-02T10:00:00.000Z"));
            });
        })
        .account("acct-personal", "me@example.com", |a| {
            a.org("org-personal", "Personal", |o| {
                o.session("s-notes", |s| s.cwd("/home/u/notes"));
            });
        })
        .active("acct-personal", "org-personal")
        .build()
}

#[test]
fn it_lays_out_both_stores_the_way_claude_does() {
    let w = two_accounts();
    let d = w.claude_digest();
    let paths = d.paths();

    let id = w.session_id("s-auth").unwrap();
    assert!(
        paths.iter().any(|p| p.ends_with(&format!("-home-u-proj/{id}.jsonl"))),
        "transcript missing, got {paths:#?}"
    );
    assert!(w.transcript_path("s-auth").is_some_and(|p| p.exists()));

    let entries = TreeDigest::of(&w.user_data);
    // Readable fixture names, uuid directories on disk: the only shape Claude uses.
    let prefix = format!(
        "claude-code-sessions/{}/{}/local_",
        w.account_uuid("acct-work"),
        w.org_uuid("org-acme")
    );
    assert!(
        entries.paths().iter().any(|p| p.starts_with(&prefix)),
        "desktop entry missing, got {:#?}",
        entries.paths()
    );
}

#[test]
fn the_active_account_is_what_env_and_config_json_agree_on() {
    let w = two_accounts();
    let raw = std::fs::read_to_string(w.home.join(".claude.json")).unwrap();
    assert!(raw.contains(&w.account_uuid("acct-personal")));
    assert!(raw.contains("me@example.com"));

    let env = w.env();
    assert_eq!(env.config_dirs, vec![w.config_dir.clone()]);
    assert_eq!(env.user_data, vec![w.user_data.clone()]);
    assert_eq!(env.unsilo_home, w.unsilo_home);
}

#[test]
fn ids_are_stable_across_builds_so_snapshots_do_not_churn() {
    assert_eq!(two_accounts().session_id("s-auth"), two_accounts().session_id("s-auth"));
}

#[test]
fn subagents_land_nested_and_are_not_top_level_transcripts() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/p").subagents(3));
            });
        })
        .build();
    let id = w.session_id("s").unwrap();
    let paths = w.claude_digest();
    let nested: Vec<_> =
        paths.paths().into_iter().filter(|p| p.contains(&format!("{id}/subagents/"))).collect();
    assert_eq!(nested.len(), 3, "got {:#?}", paths.paths());
}

#[test]
fn a_partial_last_line_is_written_as_a_real_truncated_append() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/p").partial_last_line());
            });
        })
        .build();
    let raw = std::fs::read_to_string(w.transcript_path("s").unwrap()).unwrap();
    let last = raw.lines().last().unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(last).is_err(), "last line should not parse");
    assert!(!raw.ends_with('\n'));
}

#[test]
fn digest_groups_paths_that_share_an_inode() {
    let w = two_accounts();
    let src = w.transcript_path("s-auth").unwrap();
    let dst = w.unsilo_home.join("linked.jsonl");
    std::fs::hard_link(&src, &dst).unwrap();

    let d = w.digest();
    assert_eq!(d.link_groups.len(), 1, "expected exactly one link group");
    let group = &d.link_groups[0];
    assert_eq!(group.len(), 2);
    assert!(group.iter().any(|p| p.ends_with("linked.jsonl")));
}

#[test]
fn digest_diffs_name_the_paths_that_moved() {
    let w = two_accounts();
    let before = w.digest();
    std::fs::write(w.unsilo_home.join("new.txt"), b"x").unwrap();
    let after = w.digest();

    assert_eq!(before.added_in(&after), vec!["unsilo/new.txt".to_owned()]);
    assert!(before.removed_in(&after).is_empty());
    assert_ne!(before, after);
}

#[test]
fn retention_cleanup_removes_the_project_dir_copy_only() {
    let w = two_accounts();
    let path = w.transcript_path("s-old").unwrap();
    assert!(path.exists());
    assert!(w.simulate_retention_cleanup("s-old"));
    assert!(!path.exists());
    // The desktop index entry survives, which is exactly the real failure mode.
    let entries = TreeDigest::of(&w.user_data);
    assert!(entries.paths().iter().any(|p| p.contains("local_")));
}

#[test]
fn a_desktop_rewrite_changes_the_entry_bytes() {
    let w = two_accounts();
    let before = TreeDigest::of(&w.user_data);
    let host = format!("local_{}", unsilo_testkit::ids::uuid_for("host:s-auth"));
    assert!(w.simulate_desktop_rewrite("acct-work", "org-acme", &host));
    let after = TreeDigest::of(&w.user_data);
    assert_eq!(before.changed_in(&after).len(), 1);
}

#[test]
fn credentials_sentinels_are_planted_where_a_leak_would_show_up() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |_| {});
        })
        .credentials_with_sentinel("SENTINEL-a1b2c3")
        .build();
    let creds = std::fs::read_to_string(w.config_dir.join(".credentials.json")).unwrap();
    assert!(creds.contains("SENTINEL-a1b2c3"));
    assert!(w.config_dir.join("sessions").join("1234.abcd.key").exists());
}

#[test]
fn the_remote_backend_flag_round_trips_into_config_json() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |_| {});
        })
        .hover_rest(true)
        .build();
    let raw = std::fs::read_to_string(w.home.join(".claude.json")).unwrap();
    assert!(raw.contains("\"tengu_hover_rest\": true"));
}

#[test]
fn orphaned_files_are_written_with_a_name_claude_itself_skips() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |_| {});
        })
        .orphaned("gone", "/home/u/p")
        .build();
    let paths = w.claude_digest();
    assert!(paths.paths().iter().any(|p| p.contains(".orphaned-")), "got {:#?}", paths.paths());
}
