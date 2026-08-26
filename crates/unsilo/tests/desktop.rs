//! The desktop index and identity resolution against real fixture trees.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use unsilo::claude::{desktop, identity};
use unsilo_testkit::World;

fn two_accounts() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix auth"));
                o.session("s-deploy", |s| s.cwd("/home/u/proj").title("Deploy"));
                o.session("s-gone", |s| s.cwd("/home/u/proj"));
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

#[test]
fn the_inventory_finds_every_account_and_org_not_just_the_active_one() {
    let w = two_accounts();
    let inv = desktop::inventory(&w.user_data);

    assert_eq!(inv.entries.len(), 4);
    assert_eq!(inv.tombstones.len(), 1);
    assert!(inv.unreadable.is_empty());
    assert_eq!(inv.scopes[&w.account_uuid("acct-work")], vec![w.org_uuid("org-acme")]);
    assert_eq!(inv.scopes[&w.account_uuid("acct-personal")], vec![w.org_uuid("org-personal")]);
}

#[test]
fn the_active_account_sees_only_its_own_directory() {
    let w = two_accounts();
    let inv = desktop::inventory(&w.user_data);

    // This is the bug, stated as an assertion: three of four are invisible.
    assert_eq!(
        inv.entries_in(&w.account_uuid("acct-personal"), &w.org_uuid("org-personal")).len(),
        1
    );
    assert_eq!(
        inv.missing_from(&w.account_uuid("acct-personal"), &w.org_uuid("org-personal")).len(),
        3
    );
}

#[test]
fn every_entry_bridges_back_to_a_transcript() {
    let w = two_accounts();
    let inv = desktop::inventory(&w.user_data);

    for entry in &inv.entries {
        let cli = entry.cli_session_id.as_deref().expect("entry points at a transcript");
        let scan = unsilo::claude::transcript::scan(&w.config_dir);
        assert!(
            scan.sessions.iter().any(|m| m.session_id == cli),
            "{} points at a transcript that is not on disk",
            entry.host_id
        );
    }
}

#[test]
fn a_tombstoned_session_is_not_offered_for_projection() {
    let w = two_accounts();
    let inv = desktop::inventory(&w.user_data);
    let gone = inv.entries.iter().find(|e| e.cli_session_id == w.session_id("s-gone")).unwrap();

    assert!(
        inv.is_tombstoned(&w.account_uuid("acct-work"), &w.org_uuid("org-acme"), gone),
        "deleted where the user deleted it"
    );
    assert!(
        !inv.is_tombstoned(&w.account_uuid("acct-personal"), &w.org_uuid("org-personal"), gone),
        "a deletion in one account says nothing about another"
    );
}

#[test]
fn projection_strips_the_account_scoped_payload_and_shrinks_the_entry() {
    let w = two_accounts();
    let inv = desktop::inventory(&w.user_data);
    let entry = &inv.entries[0];

    assert!(entry.account_scoped_bytes > 0, "the fixture carries an MCP payload");

    let projected = entry.projected(false);
    let bytes = serde_json::to_vec(&projected).unwrap();
    assert!((bytes.len() as u64) < entry.size);
    assert!(!String::from_utf8_lossy(&bytes).contains("remoteMcpServersConfig"));
    // Identity survives: this is still the same session, in another list.
    assert_eq!(projected.get("sessionId").and_then(|v| v.as_str()), Some(entry.host_id.as_str()));
    assert_eq!(
        projected.get("cliSessionId").and_then(|v| v.as_str()),
        entry.cli_session_id.as_deref()
    );
}

#[test]
fn identities_learn_the_active_account_and_persist_it() {
    let w = two_accounts();
    let path = w.unsilo_home.join("identities.json");

    let mut ids = identity::Identities::default();
    let added = ids.learn_from(&w.home);
    assert!(added >= 2, "an account and an org at minimum");
    assert_eq!(ids.email(&w.account_uuid("acct-personal")), Some("me@example.com"));
    assert_eq!(ids.org_name(&w.org_uuid("org-personal")), Some("Personal"));

    ids.save(&path).unwrap();
    assert_eq!(identity::Identities::load(&path).unwrap(), ids);
}

#[test]
fn an_account_that_was_never_active_stays_unresolved_until_named() {
    let w = two_accounts();
    let inv = desktop::inventory(&w.user_data);

    let mut ids = identity::Identities::default();
    ids.learn_from(&w.home);

    let seen: Vec<&str> = inv.scopes.keys().map(String::as_str).collect();
    assert_eq!(
        ids.unresolved(seen.clone()),
        vec![w.account_uuid("acct-work")],
        "only the active account is knowable from local config"
    );

    ids.set_manual_account(&w.account_uuid("acct-work"), "work@example.com");
    assert!(ids.unresolved(seen).is_empty());
}

#[test]
fn a_truncated_config_falls_through_to_a_backup() {
    let w = two_accounts();
    let good = std::fs::read(w.home.join(".claude.json")).unwrap();
    std::fs::create_dir_all(w.config_dir.join("backups")).unwrap();
    std::fs::write(w.config_dir.join("backups").join(".claude.json.backup.1787600414340"), &good)
        .unwrap();
    // Caught mid rewrite, which happens: the file turns over every few minutes.
    std::fs::write(w.home.join(".claude.json"), b"{\"oauthAcc").unwrap();

    let active = identity::active(&w.home).expect("recovered from the backup");
    assert_eq!(active.account, w.account_uuid("acct-personal"));
    assert_eq!(active.email.as_deref(), Some("me@example.com"));
}

#[test]
fn the_remote_backend_flag_is_read_from_the_fixture() {
    let off = two_accounts();
    assert_eq!(identity::remote_backend_enabled(&off.home, None), Some(false));

    let on = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |_| {});
        })
        .hover_rest(true)
        .build();
    assert_eq!(identity::remote_backend_enabled(&on.home, None), Some(true));
}

#[test]
fn a_sentinel_directory_is_not_mistaken_for_an_account() {
    let w = World::builder()
        .account("1e3fc9c4-ff2f-4fd5-8b42-5f10d0547d12", "a@example.com", |a| {
            a.org("9410ab45-877e-4164-a574-f5176d4fb07a", "Acme", |o| {
                o.session("s", |s| s.cwd("/home/u/p"));
            });
        })
        .sentinel_dir("skills-plugin")
        .build();

    let inv = desktop::inventory(&w.user_data);
    assert_eq!(
        inv.scopes.keys().collect::<Vec<_>>(),
        vec!["1e3fc9c4-ff2f-4fd5-8b42-5f10d0547d12"],
        "a uuid shaped fixture name is used verbatim"
    );
    assert!(!inv.scopes.contains_key("skills-plugin"));
}
