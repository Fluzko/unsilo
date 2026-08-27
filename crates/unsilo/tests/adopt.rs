//! Giving the desktop an entry for a conversation it never knew about.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use unsilo::claude::identity::Identities;
use unsilo::env::{Env, FixedClock};
use unsilo::filter::{Filter, Origin};
use unsilo::index::Index;
use unsilo::ops::{apply, ingest, off};
use unsilo_testkit::World;

const NOW: i64 = 1_787_602_036_145;

fn world() -> World {
    World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s-desktop", |s| s.cwd("/home/u/proj").title("Made in the desktop"));
                o.session("s-cli", |s| {
                    s.cwd("/home/u/proj").title("Made in the terminal").cli_only()
                });
                o.session("s-cli-too", |s| s.cwd("/home/u/notes").cli_only());
                o.session("s-side", |s| s.cwd("/home/u/proj").sidechain().cli_only());
            });
        })
        .active("acct", "org")
        .build()
}

fn env(w: &World) -> Env {
    w.env().with_clock(Arc::new(FixedClock(NOW)))
}

fn adopting() -> apply::Options {
    apply::Options { adopt_cli_sessions: true, ..apply::Options::default() }
}

fn entries(w: &World) -> Vec<String> {
    let dir = w
        .user_data
        .join("claude-code-sessions")
        .join(w.account_uuid("acct"))
        .join(w.org_uuid("org"));
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("local_"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn without_the_flag_nothing_is_adopted_but_the_count_is_reported() {
    let w = world();
    let report = apply::run(&env(&w), &Filter::default(), &apply::Options::default()).unwrap();

    assert!(report.adopted.is_empty());
    // Reported anyway, so the number is not a surprise the first time the flag
    // is passed.
    assert_eq!(report.adoptable, 2, "the two cli-born conversations, not the sidechain");
}

#[test]
fn adoption_gives_the_desktop_an_entry_it_never_had() {
    let w = world();
    let before = entries(&w).len();
    let report = apply::run(&env(&w), &Filter::default(), &adopting()).unwrap();

    assert_eq!(report.adopted.len(), 2);
    assert_eq!(entries(&w).len(), before + 2);
}

#[test]
fn a_sidechain_is_never_adopted() {
    let w = world();
    let report = apply::run(&env(&w), &Filter::default(), &adopting()).unwrap();
    let side = w.session_id("s-side").unwrap();
    assert!(!report.adopted.iter().any(|a| a.session_id == side));
}

#[test]
fn the_entry_says_what_the_transcript_says() {
    let w = world();
    apply::run(&env(&w), &Filter::default(), &adopting()).unwrap();

    let cli = w.session_id("s-cli").unwrap();
    let dir = w
        .user_data
        .join("claude-code-sessions")
        .join(w.account_uuid("acct"))
        .join(w.org_uuid("org"));
    let file = entries(&w)
        .into_iter()
        .map(|n| dir.join(n))
        .find(|p| std::fs::read_to_string(p).unwrap().contains(&cli))
        .expect("the adopted entry");
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();

    assert_eq!(value["cliSessionId"], cli);
    assert_eq!(value["title"], "Made in the terminal");
    assert_eq!(value["cwd"], "/home/u/proj");
    assert_eq!(value["unsiloOrigin"], "cli");
    // Nothing account-scoped is invented for a conversation that never had any.
    assert!(value.get("remoteMcpServersConfig").is_none());
    assert!(value.get("enabledMcpTools").is_none());
}

#[test]
fn adopting_twice_does_not_produce_a_second_entry() {
    let w = world();
    let env = env(&w);
    apply::run(&env, &Filter::default(), &adopting()).unwrap();
    let after_first = entries(&w);

    let second = apply::run(&env, &Filter::default(), &adopting()).unwrap();

    assert!(second.adopted.is_empty());
    assert_eq!(entries(&w), after_first, "the host id is derived, so it is the same entry");
}

#[test]
fn off_removes_an_adopted_entry_like_any_other_thing_it_wrote() {
    let w = world();
    let env = env(&w);
    let before = w.claude_digest();

    apply::run(&env, &Filter::default(), &adopting()).unwrap();
    assert_ne!(before, w.claude_digest());

    off::run(&env, &off::Options::default()).unwrap();
    let after = w.claude_digest();
    assert_eq!(before, after, "added {:?}", before.added_in(&after));
}

#[test]
fn an_adopted_entry_is_replaced_by_a_native_one_rather_than_shown_twice() {
    let w = world();
    let env = env(&w);
    apply::run(&env, &Filter::default(), &adopting()).unwrap();
    let cli = w.session_id("s-cli").unwrap();

    // The desktop opens the conversation and writes its own entry for it.
    let dir = w
        .user_data
        .join("claude-code-sessions")
        .join(w.account_uuid("acct"))
        .join(w.org_uuid("org"));
    let native = dir.join("local_native-0000-0000-0000-000000000000.json");
    std::fs::write(
        &native,
        serde_json::to_vec(&serde_json::json!({
            "sessionId": "local_native-0000-0000-0000-000000000000",
            "cliSessionId": cli,
            "cwd": "/home/u/proj",
            "title": "Made in the terminal",
        }))
        .unwrap(),
    )
    .unwrap();

    let report = apply::run(&env, &Filter::default(), &adopting()).unwrap();

    let adopted_path = dir.join(format!("{}.json", unsilo::adopt::host_id_for(&cli)));
    assert!(report.pruned.contains(&adopted_path), "ours is superseded: {:?}", report.pruned);
    assert!(!adopted_path.exists());
    assert!(native.exists(), "the desktop's own entry is left alone");
}

#[test]
fn an_adopted_entry_does_not_pretend_to_prove_the_account() {
    let w = world();
    let env = env(&w);
    apply::run(&env, &Filter::default(), &adopting()).unwrap();
    let results = unsilo::ops::find::run(&env, &Filter::default()).unwrap();
    let text = unsilo::report::find(&results, &w.home);

    // Unsilo put that entry there, so it says nothing about which account owns
    // the conversation. Whatever is known still comes from the inference.
    assert!(
        text.contains("(cli only)") || text.contains('?'),
        "an adopted entry must not read as a stated account: {text}"
    );
}

#[test]
fn the_origin_filter_separates_the_two_kinds() {
    let w = world();
    let env = env(&w);
    apply::run(&env, &Filter::default(), &adopting()).unwrap();

    let index = Index::open(&env.index_path()).unwrap();
    ingest::run(&env, &index).unwrap();
    let ids = Identities::default();
    let ask = |origin: Option<Origin>| {
        let filter = Filter { origin, ..Filter::default() };
        index
            .query(&filter.resolve(&ids, NOW).unwrap())
            .unwrap()
            .into_iter()
            .map(|r| r.session_id)
            .collect::<Vec<_>>()
    };

    let cli = ask(Some(Origin::Cli));
    let desktop = ask(Some(Origin::Desktop));

    assert_eq!(desktop, vec![w.session_id("s-desktop").unwrap()]);
    assert_eq!(cli.len(), 2, "an entry we built does not make it desktop-born");
    assert!(cli.contains(&w.session_id("s-cli").unwrap()));
    assert!(!cli.contains(&w.session_id("s-desktop").unwrap()));
}

#[test]
fn adoption_respects_the_filters_like_everything_else() {
    let w = world();
    let only_notes = Filter { cwd: Some("/home/u/notes".to_owned()), ..Filter::default() };
    let report = apply::run(&env(&w), &only_notes, &adopting()).unwrap();

    assert_eq!(report.adopted.len(), 1);
    assert_eq!(report.adopted[0].session_id, w.session_id("s-cli-too").unwrap());
}

#[test]
fn a_dry_run_reports_the_adoptions_and_writes_none() {
    let w = world();
    let before = w.claude_digest();
    let report =
        apply::run(&env(&w), &Filter::default(), &apply::Options { dry_run: true, ..adopting() })
            .unwrap();

    assert_eq!(report.adopted.len(), 2);
    assert_eq!(before, w.claude_digest());
}
