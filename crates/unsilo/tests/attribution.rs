//! Attributing CLI-born conversations from when they were alive.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use unsilo::claude::identity::Identities;
use unsilo::env::{Env, FixedClock};
use unsilo::filter::Filter;
use unsilo::index::Index;
use unsilo::ops::{find, ingest};
use unsilo_testkit::World;

/// 2026-08-24T20:07:16.145Z
const NOW: i64 = 1_787_602_036_145;
const DAY: i64 = 86_400_000;

fn env_at(w: &World, at_ms: i64) -> Env {
    w.env().with_clock(Arc::new(FixedClock(at_ms)))
}

fn ingested(env: &Env) -> (Index, ingest::Summary) {
    let index = Index::open(&env.index_path()).unwrap();
    let summary = ingest::run(env, &index).unwrap();
    (index, summary)
}

fn ids(w: &World) -> Identities {
    let mut ids = Identities::default();
    ids.learn_from(&w.home);
    ids
}

fn selected(env: &Env, w: &World, filter: &Filter) -> Vec<String> {
    let index = Index::open(&env.index_path()).unwrap();
    let resolved = filter.resolve(&ids(w), NOW).unwrap();
    index.query(&resolved).unwrap().into_iter().map(|r| r.session_id).collect()
}

/// One desktop session, whose entry dates a sighting, plus CLI-born ones around
/// it. This is the retroactive case: nothing was observed live.
fn world() -> World {
    World::builder()
        .account("acct-work", "work@example.com", |a| {
            a.org("org-acme", "Acme", |o| {
                o.session("s-desktop", |s| {
                    s.cwd("/home/u/proj")
                        .title("From the desktop")
                        .created("2026-08-01T00:00:00.000Z")
                        .modified("2026-08-10T00:00:00.000Z")
                });
                o.session("s-during", |s| {
                    s.cwd("/home/u/proj")
                        .title("Alive while work was signed in")
                        .created("2026-08-03T00:00:00.000Z")
                        .modified("2026-08-04T00:00:00.000Z")
                        .cli_only()
                });
                o.session("s-ancient", |s| {
                    s.cwd("/home/u/proj")
                        .title("Older than anything we ever saw")
                        .created("2026-01-01T00:00:00.000Z")
                        .modified("2026-01-02T00:00:00.000Z")
                        .cli_only()
                });
            });
        })
        .active("acct-work", "org-acme")
        .build()
}

#[test]
fn a_desktop_entry_dates_a_sighting_that_attributes_its_neighbours() {
    let w = world();
    let env = env_at(&w, NOW);
    let (_index, summary) = ingested(&env);

    assert!(summary.sightings > 0, "the entry timestamps are sightings");
    let (attributed, needing) = summary.attributed;
    assert_eq!(needing, 2, "the two cli-born conversations");
    assert_eq!(attributed, 1, "only the one alive inside the sighting window");
}

#[test]
fn a_conversation_older_than_every_sighting_stays_unattributed() {
    let w = world();
    let env = env_at(&w, NOW);
    ingested(&env);

    let index = Index::open(&env.index_path()).unwrap();
    let rows = index.query(&Filter::default().resolve(&ids(&w), NOW).unwrap()).unwrap();
    let by_title = |t: &str| rows.iter().find(|r| r.display_title() == Some(t)).unwrap().clone();

    assert!(by_title("Alive while work was signed in").inferred_account.is_some());
    assert_eq!(
        by_title("Older than anything we ever saw").inferred_account,
        None,
        "nothing vouches for it, and inventing an account is worse than saying nothing"
    );
}

#[test]
fn an_email_filter_reaches_inferred_conversations() {
    let w = world();
    let env = env_at(&w, NOW);
    ingested(&env);

    let by_email = Filter { emails: vec!["work@example.com".to_owned()], ..Filter::default() };
    let found = selected(&env, &w, &by_email);

    // Without the inference this would be one: only the desktop session.
    assert_eq!(found.len(), 2);
    assert!(found.contains(&w.session_id("s-during").unwrap()));
    assert!(!found.contains(&w.session_id("s-ancient").unwrap()));
}

#[test]
fn confirmed_only_drops_back_to_what_an_entry_states() {
    let w = world();
    let env = env_at(&w, NOW);
    ingested(&env);

    let strict = Filter {
        emails: vec!["work@example.com".to_owned()],
        confirmed_only: true,
        ..Filter::default()
    };
    assert_eq!(selected(&env, &w, &strict), vec![w.session_id("s-desktop").unwrap()]);
}

#[test]
fn a_conversation_belongs_to_the_account_that_started_it() {
    let w = World::builder()
        .account("acct-a", "a@example.com", |a| {
            a.org("org-a", "A", |o| {
                o.session("s-a", |s| {
                    s.cwd("/home/u/p")
                        .created("2026-08-01T00:00:00.000Z")
                        .modified("2026-08-02T00:00:00.000Z")
                });
                o.session("s-straddling", |s| {
                    s.cwd("/home/u/p")
                        .title("Spans the switch")
                        .created("2026-08-01T12:00:00.000Z")
                        .modified("2026-08-06T00:00:00.000Z")
                        .cli_only()
                });
            });
        })
        .account("acct-b", "b@example.com", |a| {
            a.org("org-b", "B", |o| {
                o.session("s-b", |s| {
                    s.cwd("/home/u/p")
                        .created("2026-08-05T00:00:00.000Z")
                        .modified("2026-08-07T00:00:00.000Z")
                });
            });
        })
        .active("acct-b", "org-b")
        .build();
    let env = env_at(&w, NOW);
    ingested(&env);

    let index = Index::open(&env.index_path()).unwrap();
    let rows = index.query(&Filter::default().resolve(&ids(&w), NOW).unwrap()).unwrap();
    let spanning = rows.iter().find(|r| r.display_title() == Some("Spans the switch")).unwrap();

    // Begun while A was signed in and still being written after the switch to B.
    // Asking about its whole lifetime would answer a question nobody asked.
    assert_eq!(spanning.inferred_account.as_deref(), Some(w.account_uuid("acct-a").as_str()));
}

#[test]
fn a_switch_around_the_moment_it_started_leaves_it_unattributed() {
    let w = World::builder()
        .account("acct-a", "a@example.com", |a| {
            a.org("org-a", "A", |o| {
                o.session("s-a", |s| {
                    s.cwd("/home/u/p")
                        .created("2026-08-01T00:00:00.000Z")
                        .modified("2026-08-01T00:00:00.000Z")
                });
            });
        })
        .account("acct-b", "b@example.com", |a| {
            a.org("org-b", "B", |o| {
                o.session("s-b", |s| {
                    s.cwd("/home/u/p")
                        .created("2026-08-03T00:00:00.000Z")
                        .modified("2026-08-03T00:00:00.000Z")
                });
                o.session("s-between", |s| {
                    s.cwd("/home/u/p")
                        .title("Started between two accounts")
                        .created("2026-08-02T00:00:00.000Z")
                        .modified("2026-08-02T01:00:00.000Z")
                        .cli_only()
                });
            });
        })
        .active("acct-b", "org-b")
        .build();
    let env = env_at(&w, NOW);
    ingested(&env);

    let index = Index::open(&env.index_path()).unwrap();
    let rows = index.query(&Filter::default().resolve(&ids(&w), NOW).unwrap()).unwrap();
    let between =
        rows.iter().find(|r| r.display_title() == Some("Started between two accounts")).unwrap();

    assert_eq!(
        between.inferred_account, None,
        "the sightings around the moment it began disagree, so we cannot tell who began it"
    );
}

#[test]
fn attribution_improves_as_more_is_observed() {
    // A conversation from today cannot be placed before anything has been seen,
    // and can be once the current account has been observed alongside it.
    let w = World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s-now", |s| {
                    s.cwd("/home/u/p")
                        .created("2026-08-24T00:00:00.000Z")
                        .modified("2026-08-24T01:00:00.000Z")
                        .cli_only()
                });
            });
        })
        .active("acct", "org")
        .build();

    // Observed a week before the conversation existed: nothing to say.
    let early = env_at(&w, NOW - 7 * DAY);
    let (_i, first) = ingested(&early);
    assert_eq!(first.attributed, (0, 1));

    // Observed after it: now the last sighting before it belongs to this account.
    let later = env_at(&w, NOW);
    let (_i, second) = ingested(&later);
    assert_eq!(second.attributed, (1, 1));
}

#[test]
fn an_inference_is_marked_as_one_in_the_listing() {
    let w = world();
    let env = env_at(&w, NOW);
    let results = find::run(&env, &Filter::default()).unwrap();
    let text = unsilo::report::find(unsilo::style::Style::plain(), &results, &w.home);

    // A trailing "?" is the whole difference between stated and suspected.
    assert!(text.contains("work@example.com?"), "{text}");
    assert!(text.contains("work@example.com "), "the stated one carries no marker: {text}");
}

#[test]
fn sightings_only_accumulate_so_a_rerun_cannot_lose_one() {
    let w = world();
    let first = ingested(&env_at(&w, NOW)).1.sightings;
    let second = ingested(&env_at(&w, NOW + DAY)).1.sightings;
    assert!(second > first, "a new observation is added, none replaced");
}
