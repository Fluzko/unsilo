//! The catalogue and the filters that both `find` and `apply` will share.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use unsilo::claude::identity::Identities;
use unsilo::env::FixedClock;
use unsilo::filter::{Filter, Sort};
use unsilo::index::Index;
use unsilo::ops::{find, ingest};
use unsilo_testkit::World;

/// 2026-08-24T20:07:16.145Z, so relative filters are deterministic.
const NOW: i64 = 1_787_602_036_145;

fn world() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-auth", |s| {
                    s.cwd("/home/u/proj")
                        .title("Fix the auth bug")
                        .branch("feature/auth")
                        .model("claude-opus-5")
                        .modified("2026-08-20T10:00:00.000Z")
                });
                o.session("s-old", |s| {
                    s.cwd("/home/u/proj").title("Ancient").modified("2026-01-02T10:00:00.000Z")
                });
                o.session("s-side", |s| s.cwd("/home/u/proj").sidechain());
                o.session("s-gone", |s| s.cwd("/home/u/proj").title("Deleted"));
                o.tombstone("s-gone");
            });
        })
        .account("acct-personal", "me@example.com", |a| {
            a.org("org-personal", "Personal", |o| {
                o.session("s-notes", |s| {
                    s.cwd("/home/u/notes")
                        .title("Grocery notes")
                        .model("claude-sonnet-5")
                        .modified("2026-08-22T10:00:00.000Z")
                });
            });
        })
        .active("acct-personal", "org-personal")
        .build()
}

fn env(w: &World) -> unsilo::Env {
    w.env().with_clock(std::sync::Arc::new(FixedClock(NOW)))
}

fn ids(w: &World) -> Identities {
    let mut ids = Identities::default();
    ids.learn_from(&w.home);
    // The non-active account is only knowable by hand, as on a real machine.
    ids.set_manual_account(&w.account_uuid("acct-work"), "work@example.com");
    ids
}

fn run(w: &World, filter: &Filter) -> Vec<String> {
    let env = env(w);
    let index = Index::open_in_memory().unwrap();
    ingest::run(&env, &index).unwrap();
    let resolved = filter.resolve(&ids(w), NOW).unwrap();
    index.query(&resolved).unwrap().into_iter().map(|r| r.session_id).collect()
}

fn titles(w: &World, filter: &Filter) -> Vec<String> {
    let env = env(w);
    let index = Index::open_in_memory().unwrap();
    ingest::run(&env, &index).unwrap();
    let resolved = filter.resolve(&ids(w), NOW).unwrap();
    index
        .query(&resolved)
        .unwrap()
        .into_iter()
        .map(|r| r.display_title().unwrap_or("?").to_owned())
        .collect()
}

#[test]
fn an_empty_filter_lists_every_visible_conversation() {
    let w = world();
    let found = run(&w, &Filter::default());
    // Five transcripts: one is a sidechain, one was deleted from its account.
    assert_eq!(found.len(), 3);
    assert!(!found.contains(&w.session_id("s-side").unwrap()));
    assert!(!found.contains(&w.session_id("s-gone").unwrap()));
}

#[test]
fn hidden_sessions_come_back_when_asked_for() {
    let w = world();
    let found = run(&w, &Filter { include_hidden: true, ..Filter::default() });
    assert_eq!(found.len(), 4, "the sidechain returns; the deleted one needs its own flag");
    assert!(found.contains(&w.session_id("s-side").unwrap()));

    let everything =
        run(&w, &Filter { include_hidden: true, include_deleted: true, ..Filter::default() });
    assert_eq!(everything.len(), 5);
}

#[test]
fn filtering_by_email_selects_that_accounts_sessions() {
    let w = world();
    let found =
        run(&w, &Filter { emails: vec!["work@example.com".to_owned()], ..Filter::default() });
    assert_eq!(found.len(), 2, "auth and old; the deleted one is excluded, the sidechain hidden");
    assert!(found.contains(&w.session_id("s-auth").unwrap()));
    assert!(!found.contains(&w.session_id("s-notes").unwrap()));
}

#[test]
fn a_tombstoned_session_needs_an_explicit_flag() {
    let w = world();
    let base = Filter { emails: vec!["work@example.com".to_owned()], ..Filter::default() };
    assert!(!run(&w, &base).contains(&w.session_id("s-gone").unwrap()));

    let with_deleted = Filter { include_deleted: true, ..base };
    assert!(run(&w, &with_deleted).contains(&w.session_id("s-gone").unwrap()));
}

#[test]
fn relative_time_filters_are_measured_from_the_injected_clock() {
    let w = world();
    let recent = run(&w, &Filter { since: Some("30d".to_owned()), ..Filter::default() });
    assert_eq!(recent.len(), 2, "the January session is out");
    assert!(!recent.contains(&w.session_id("s-old").unwrap()));

    let ancient = run(&w, &Filter { until: Some("30d".to_owned()), ..Filter::default() });
    assert_eq!(ancient, vec![w.session_id("s-old").unwrap()]);
}

#[test]
fn filters_of_different_kinds_intersect() {
    let w = world();
    let both = Filter {
        emails: vec!["work@example.com".to_owned()],
        since: Some("30d".to_owned()),
        ..Filter::default()
    };
    assert_eq!(run(&w, &both), vec![w.session_id("s-auth").unwrap()]);
}

#[test]
fn branch_project_and_model_all_narrow_the_set() {
    let w = world();
    let by_branch = Filter { branch: Some("auth".to_owned()), ..Filter::default() };
    assert_eq!(run(&w, &by_branch), vec![w.session_id("s-auth").unwrap()]);

    let by_project = Filter { project: Some("home-u-notes".to_owned()), ..Filter::default() };
    assert_eq!(run(&w, &by_project), vec![w.session_id("s-notes").unwrap()]);

    let by_model = Filter { model: Some("sonnet".to_owned()), ..Filter::default() };
    assert_eq!(run(&w, &by_model), vec![w.session_id("s-notes").unwrap()]);
}

#[test]
fn a_cwd_filter_matches_by_prefix() {
    let w = world();
    let found = run(&w, &Filter { cwd: Some("/home/u/pro".to_owned()), ..Filter::default() });
    assert_eq!(found.len(), 2);
    assert!(!found.contains(&w.session_id("s-notes").unwrap()));
}

#[test]
fn an_id_prefix_is_enough_to_pick_one_session() {
    let w = world();
    let full = w.session_id("s-auth").unwrap();
    let prefix = full.get(..8).unwrap().to_owned();
    assert_eq!(run(&w, &Filter { id: Some(prefix), ..Filter::default() }), vec![full]);
}

#[test]
fn full_text_search_reaches_titles_and_prompts() {
    let w = world();
    let found = titles(&w, &Filter { query: Some("grocery".to_owned()), ..Filter::default() });
    assert_eq!(found, vec!["Grocery notes".to_owned()], "search is case insensitive");
}

#[test]
fn search_terms_combine_the_way_fts_says_they_do() {
    let w = world();
    let and = Filter { query: Some("auth AND bug".to_owned()), ..Filter::default() };
    assert_eq!(run(&w, &and).len(), 1);

    let phrase = Filter { query: Some("\"grocery notes\"".to_owned()), ..Filter::default() };
    assert_eq!(run(&w, &phrase).len(), 1);

    let nothing = Filter { query: Some("kumquat".to_owned()), ..Filter::default() };
    assert!(run(&w, &nothing).is_empty());
}

#[test]
fn sorting_and_limiting_do_not_change_which_sessions_match() {
    let w = world();
    let by_recent = Filter { sort: Sort::Recent, ..Filter::default() };
    let ordered = run(&w, &by_recent);
    assert_eq!(ordered.first(), w.session_id("s-notes").as_ref(), "newest first");
    assert_eq!(ordered.last(), w.session_id("s-old").as_ref());

    let limited = run(&w, &Filter { limit: Some(2), ..by_recent });
    assert_eq!(limited.len(), 2);
    assert_eq!(limited, ordered[..2]);
}

#[test]
fn a_session_in_two_project_dirs_is_one_session_pointing_at_the_larger_copy() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                // Claude leaves the old copy behind when a project moves.
                o.session("moved", |s| s.cwd("/home/u/old").messages(2).also_in("/home/u/new", 20));
            });
        })
        .build();
    let env = env(&w);
    let index = Index::open_in_memory().unwrap();
    ingest::run(&env, &index).unwrap();

    let rows =
        index.query(&Filter::default().resolve(&Identities::default(), NOW).unwrap()).unwrap();
    assert_eq!(rows.len(), 1, "one uuid is one session, wherever its copies are");
    // Append only means the bigger file is the more complete history.
    assert!(rows[0].origin_dir.ends_with("-home-u-new"), "got {}", rows[0].origin_dir);

    let duplicates = index.duplicate_locations().unwrap();
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0].1.len(), 2);
}

#[test]
fn a_cli_born_session_with_no_desktop_entry_is_still_listed() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("cli-only", |s| s.cwd("/home/u/p").title("No desktop").cli_only());
            });
        })
        .build();
    assert_eq!(titles(&w, &Filter::default()), vec!["No desktop".to_owned()]);
}

#[test]
fn ingest_is_idempotent_and_reports_what_it_saw() {
    let w = world();
    let env = env(&w);
    let index = Index::open(&env.index_path()).unwrap();

    let first = ingest::run(&env, &index).unwrap();
    assert_eq!(first.sessions, 5);
    assert_eq!(first.desktop_entries, 5);
    assert_eq!(first.tombstones, 1);
    assert!(first.identities_learned > 0, "the active account is captured on first sight");

    let second = ingest::run(&env, &index).unwrap();
    assert_eq!(second.sessions, first.sessions);
    assert_eq!(index.count_sessions().unwrap(), 5);
    assert_eq!(second.identities_learned, 0, "already persisted, nothing new to learn");
}

#[test]
fn the_index_migrates_to_the_latest_schema_and_survives_reopening() {
    let w = world();
    let env = env(&w);
    {
        let index = Index::open(&env.index_path()).unwrap();
        assert_eq!(index.version(), unsilo::index::schema::latest_version());
        ingest::run(&env, &index).unwrap();
    }
    let reopened = Index::open(&env.index_path()).unwrap();
    assert_eq!(reopened.version(), unsilo::index::schema::latest_version());
    assert_eq!(reopened.count_sessions().unwrap(), 5);
}

#[test]
fn find_counts_matches_before_the_limit_is_applied() {
    let w = world();
    let results = find::run(&env(&w), &Filter { limit: Some(1), ..Filter::default() }).unwrap();
    assert_eq!(results.rows.len(), 1);
    assert_eq!(results.matched, 3, "so the summary can say one of three");
    assert_eq!(results.total, 5, "total counts everything indexed, filters aside");
}

#[test]
fn find_writes_only_inside_the_store() {
    let w = world();
    let before = w.claude_digest();
    find::run(&env(&w), &Filter::default()).unwrap();
    let after = w.claude_digest();

    assert_eq!(
        before,
        after,
        "find may build an index, but never touches Claude's tree; changed {:?}",
        before.changed_in(&after)
    );
    assert!(w.unsilo_home.join("index.sqlite").exists());
}
