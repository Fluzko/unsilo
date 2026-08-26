//! The parser against real trees built by the testkit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use unsilo::claude::transcript::{self, Hidden};
use unsilo_testkit::World;

fn one(w: &World, name: &str) -> transcript::Meta {
    let path = w.transcript_path(name).expect("known session");
    transcript::parse(&path).expect("readable").expect("a transcript")
}

#[test]
fn scan_separates_conversations_from_everything_else() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("plain", |s| s.cwd("/home/u/p"));
                o.session("with-subs", |s| s.cwd("/home/u/p").subagents(3));
                o.session("side", |s| s.cwd("/home/u/p").sidechain());
            });
        })
        .orphaned("gone", "/home/u/p")
        .build();

    let scan = transcript::scan(&w.config_dir);

    assert_eq!(scan.sessions.len(), 3, "three top level transcripts");
    assert_eq!(scan.subagents, 3, "nested subagents counted apart, never as conversations");
    assert_eq!(scan.skipped, 1, "the orphaned rename is skipped like Claude skips it");
    assert_eq!(scan.project_dirs, 1);
    assert!(scan.unreadable.is_empty());

    let visible = scan.sessions.iter().filter(|m| m.hidden_from_resume().is_none()).count();
    assert_eq!(visible, 2, "the sidechain is not a conversation");
}

#[test]
fn the_recorded_origin_dir_is_kept_rather_than_recomputed() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/proj"));
            });
        })
        .build();
    let m = one(&w, "s");
    assert_eq!(m.origin_dir, w.config_dir.join("projects").join("-home-u-proj"));
    assert_eq!(m.cwd.as_deref(), Some("/home/u/proj"));
    assert_eq!(m.path.file_name(), Some(format!("{}.jsonl", m.session_id).as_str()));
}

#[test]
fn a_relocated_record_in_the_tail_overrides_the_head_cwd() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/old").relocated_to("/home/u/new"));
            });
        })
        .build();
    let m = one(&w, "s");
    assert_eq!(m.cwd.as_deref(), Some("/home/u/new"));
    // The file still lives under the original slug, which is why origin_dir exists.
    assert!(m.origin_dir.as_str().ends_with("-home-u-old"));
}

#[test]
fn a_custom_title_beats_the_first_prompt() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("titled", |s| s.cwd("/home/u/p").title("Fix auth"));
                o.session("untitled", |s| s.cwd("/home/u/p"));
            });
        })
        .build();
    assert_eq!(one(&w, "titled").display_title(), Some("Fix auth"));
    assert_eq!(one(&w, "untitled").title, None);
    assert!(one(&w, "untitled").display_title().is_some_and(|t| t.starts_with("prompt 0")));
}

#[test]
fn a_half_written_final_line_is_expected_not_an_error() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/p").title("Fix auth").partial_last_line());
            });
        })
        .build();
    let m = one(&w, "s");
    assert_eq!(m.title.as_deref(), Some("Fix auth"), "records before the tear still parse");
    assert!(!m.tail_unresolved);
}

#[test]
fn a_record_larger_than_the_window_forces_it_to_grow() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                // One record well past the 64 KiB window, with the title behind it.
                o.session("s", |s| {
                    s.cwd("/home/u/p").title("Behind the wall").giant_record(400_000)
                });
            });
        })
        .build();
    let m = one(&w, "s");
    assert!(m.size > 400_000);
    assert_eq!(m.title.as_deref(), Some("Behind the wall"));
    assert!(!m.tail_unresolved);
}

#[test]
fn daemon_sessions_are_hidden_the_way_claude_hides_them() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("d", |s| s.cwd("/home/u/p").session_kind("daemon"));
                o.session("w", |s| s.cwd("/home/u/p").session_kind("daemon-worker"));
                o.session("bg", |s| s.cwd("/home/u/p").session_kind("bg"));
            });
        })
        .build();
    assert_eq!(one(&w, "d").hidden_from_resume(), Some(Hidden::Daemon));
    assert_eq!(one(&w, "w").hidden_from_resume(), Some(Hidden::Daemon));
    assert_eq!(one(&w, "bg").hidden_from_resume(), None, "bg is a real session");
}

#[test]
fn metadata_comes_from_the_records_not_the_filesystem() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| {
                    s.cwd("/home/u/p")
                        .branch("feature/x")
                        .cli_version("2.1.241")
                        .created("2026-01-02T03:04:05.000Z")
                        .modified("2026-03-04T05:06:07.000Z")
                });
            });
        })
        .build();
    let m = one(&w, "s");
    assert_eq!(m.git_branch.as_deref(), Some("feature/x"));
    assert_eq!(m.cli_version.as_deref(), Some("2.1.241"));
    assert_eq!(m.created_at_ms, Some(1_767_323_045_000));
    assert_eq!(m.modified_at_ms, Some(1_772_600_767_000));
    assert!(m.created_at_ms < m.modified_at_ms);
    assert_eq!(m.record_id.as_deref(), Some(m.session_id.as_str()));
}

#[test]
fn reading_a_transcript_while_claude_appends_to_it_never_corrupts() {
    use std::io::Write as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/p").title("Live").messages(20));
            });
        })
        .build();
    let path = w.transcript_path("s").unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let (path, stop) = (path.clone(), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            while !stop.load(Ordering::Relaxed) {
                let line = format!(
                    r#"{{"type":"assistant","timestamp":"2026-09-01T00:00:00.000Z","message":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
                    "x".repeat(512)
                );
                let _ = f.write_all(line.as_bytes());
                let _ = f.write_all(b"\n");
            }
        })
    };

    let first = transcript::parse(&path).expect("readable").expect("a transcript");
    let mut last_size = 0;
    for _ in 0..200 {
        let m = transcript::parse(&path).expect("readable").expect("a transcript");
        // Head derived fields cannot move: the prefix they come from is frozen.
        assert_eq!(m.session_id, first.session_id);
        assert_eq!(m.first_prompt, first.first_prompt);
        assert_eq!(m.created_at_ms, first.created_at_ms);
        // The size comes from the open handle, so it is a prefix of what is on
        // disk by now: it only ever grows, and never disagrees with what we read.
        assert!(m.size >= last_size, "size went backwards: {} then {}", last_size, m.size);
        last_size = m.size;
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
}

#[test]
fn a_title_set_long_ago_scrolls_out_of_the_tail_window() {
    use std::io::Write as _;

    // Claude reads a fixed size tail too, so a custom-title buried under enough
    // later output stops being visible there for both of us. Documented rather
    // than worked around: the index keeps the title it saw the first time.
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/p").title("Set early"));
            });
        })
        .build();
    let path = w.transcript_path("s").unwrap();
    assert_eq!(one(&w, "s").title.as_deref(), Some("Set early"));

    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    for _ in 0..300 {
        writeln!(
            f,
            r#"{{"type":"assistant","timestamp":"2026-09-01T00:00:00.000Z","message":{{"content":[{{"type":"text","text":"{}"}}]}}}}"#,
            "x".repeat(512)
        )
        .unwrap();
    }
    drop(f);

    let m = transcript::parse(&path).unwrap().unwrap();
    assert!(m.size > 64 * 1024);
    assert_eq!(m.title, None, "out of the window, and that is the same rule Claude follows");
    assert!(m.first_prompt.is_some(), "head derived data is unaffected");
}

#[test]
fn an_empty_or_misnamed_file_is_not_a_transcript() {
    let w = World::builder()
        .account("a", "a@example.com", |a| {
            a.org("o", "O", |o| {
                o.session("s", |s| s.cwd("/home/u/p"));
            });
        })
        .build();
    let dir = w.config_dir.join("projects").join("-home-u-p");
    let empty = dir.join("00000000-0000-4000-8000-000000000000.jsonl");
    std::fs::write(&empty, b"").unwrap();
    assert!(transcript::parse(&empty).unwrap().is_none());

    let named = dir.join("not-a-uuid.jsonl");
    std::fs::write(&named, b"{}\n").unwrap();
    assert!(transcript::parse(&named).unwrap().is_none());
}

#[test]
fn a_missing_file_reports_the_path_it_failed_on() {
    let missing = camino::Utf8PathBuf::from("/nope/00000000-0000-4000-8000-000000000000.jsonl");
    let err = transcript::parse(&missing).unwrap_err();
    assert!(err.to_string().contains("/nope/"), "{err}");
}
