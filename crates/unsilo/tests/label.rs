//! Naming accounts by hand, which is the only way for one that is never active.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use assert_cmd::Command;
use unsilo::claude::identity::{Identities, Source};
use unsilo::ops::label::{self, Kind};
use unsilo_testkit::World;

fn world() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| s.cwd("/home/u/proj").title("Fix auth"));
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

fn identities(w: &World) -> Identities {
    Identities::load(&w.unsilo_home.join("identities.json")).unwrap()
}

#[test]
fn learn_captures_only_the_account_signed_in_right_now() {
    let w = world();
    let learned = label::learn(&w.env()).unwrap();

    assert_eq!(learned.active_account.as_deref(), Some(w.account_uuid("acct-personal").as_str()));
    assert_eq!(learned.active_email.as_deref(), Some("me@example.com"));
    assert!(learned.added > 0);

    let ids = identities(&w);
    assert_eq!(ids.email(&w.account_uuid("acct-personal")), Some("me@example.com"));
    assert_eq!(ids.email(&w.account_uuid("acct-work")), None, "not signed in, not knowable");
}

#[test]
fn learn_is_idempotent_so_it_is_safe_on_every_session_start() {
    let w = world();
    assert!(label::learn(&w.env()).unwrap().added > 0);
    assert_eq!(label::learn(&w.env()).unwrap().added, 0);
}

#[test]
fn learn_writes_nothing_outside_the_store() {
    let w = world();
    let before = w.claude_digest();
    label::learn(&w.env()).unwrap();
    assert_eq!(before, w.claude_digest());
}

#[test]
fn an_account_that_is_never_active_can_still_be_named() {
    let w = world();
    let uuid = w.account_uuid("acct-work");
    let labelled = label::set(&w.env(), &uuid[..8], "work@example.com").unwrap();

    assert_eq!(labelled.kind, Kind::Account);
    assert_eq!(labelled.uuid, uuid);
    assert_eq!(identities(&w).email(&uuid), Some("work@example.com"));
}

#[test]
fn a_manual_label_outlives_learning() {
    let w = world();
    let uuid = w.account_uuid("acct-personal");
    label::set(&w.env(), &uuid[..8], "chosen@example.com").unwrap();

    label::learn(&w.env()).unwrap();

    let ids = identities(&w);
    assert_eq!(ids.email(&uuid), Some("chosen@example.com"), "learning must not overwrite it");
    assert_eq!(ids.accounts[&uuid].source, Source::Manual);
}

#[test]
fn the_kind_comes_from_the_id_not_from_a_flag() {
    // An account uuid and an org uuid look identical, so asking the user which
    // one they meant would be asking them to know something we already know.
    let w = world();
    let org = w.org_uuid("org-acme");
    let labelled = label::set(&w.env(), &org[..8], "Acme Corp").unwrap();

    assert_eq!(labelled.kind, Kind::Org);
    assert_eq!(identities(&w).org_name(&org), Some("Acme Corp"));
}

#[test]
fn an_unknown_or_ambiguous_id_is_refused_rather_than_guessed() {
    let w = world();
    let unknown = label::set(&w.env(), "ffffffff", "x").unwrap_err();
    assert_eq!(unknown.exit_code(), 2);
    assert!(unknown.to_string().contains("--list"), "{unknown}");

    // An empty prefix matches everything, which is the ambiguous case.
    let ambiguous = label::set(&w.env(), "", "x").unwrap_err();
    assert_eq!(ambiguous.exit_code(), 2);
}

#[test]
fn an_empty_name_is_refused() {
    let w = world();
    let uuid = w.account_uuid("acct-work");
    assert!(label::set(&w.env(), &uuid[..8], "   ").is_err());
}

#[test]
fn the_listing_says_where_each_name_came_from() {
    let w = world();
    label::learn(&w.env()).unwrap();
    label::set(&w.env(), &w.account_uuid("acct-work")[..8], "work@example.com").unwrap();

    let listing = label::list(&w.env()).unwrap();
    let by_uuid = |uuid: &str| listing.accounts.iter().find(|r| r.uuid == uuid).unwrap().clone();

    let personal = by_uuid(&w.account_uuid("acct-personal"));
    assert_eq!(personal.source, Some(Source::Learned));
    assert!(personal.is_active);

    let work = by_uuid(&w.account_uuid("acct-work"));
    assert_eq!(work.source, Some(Source::Manual));
    assert!(!work.is_active);
}

#[test]
fn the_command_line_covers_all_three_modes() {
    let w = world();
    let run = |args: &[&str]| {
        let mut cmd = Command::cargo_bin("unsilo").unwrap();
        cmd.env_clear();
        for (k, v) in w.env_pairs() {
            cmd.env(k, v);
        }
        cmd.args(args).assert().success()
    };

    run(&["label", "--learn"]);
    run(&["label", &w.account_uuid("acct-work")[..8], "work@example.com"]);
    let out = run(&["label", "--list"]);
    let text = String::from_utf8_lossy(&out.get_output().stdout).into_owned();
    assert!(text.contains("work@example.com"), "{text}");
    assert!(text.contains("manual"), "{text}");

    // Neither a pair nor a mode: a usage error, not a silent no-op.
    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    cmd.arg("label").assert().failure().code(2);
}
