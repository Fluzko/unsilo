//! Colour must never reach something that is being parsed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use assert_cmd::Command;
use unsilo_testkit::World;

const ESC: char = '\u{1b}';

fn world() -> World {
    World::builder()
        .account("acct", "a@example.com", |a| {
            a.org("org", "Org", |o| {
                o.session("s-one", |s| s.cwd("/home/u/proj").title("Fix auth"));
                o.session("s-two", |s| s.cwd("/home/u/notes").cli_only());
            });
        })
        .active("acct", "org")
        .build()
}

fn run(w: &World, args: &[&str]) -> String {
    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    let out = cmd.args(args).assert();
    String::from_utf8_lossy(&out.get_output().stdout).into_owned()
}

fn run_with(w: &World, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.args(args).assert();
    String::from_utf8_lossy(&out.get_output().stdout).into_owned()
}

#[test]
fn a_captured_run_is_never_coloured() {
    // assert_cmd captures stdout, so this is exactly the piped case.
    let w = world();
    for args in [vec!["doctor"], vec!["find"], vec!["label", "--list"], vec!["apply", "--dry-run"]]
    {
        let text = run(&w, &args);
        assert!(!text.contains(ESC), "{args:?} coloured a pipe: {text:?}");
    }
}

#[test]
fn color_always_overrides_the_pipe_and_never_undoes_it() {
    let w = world();
    assert!(run(&w, &["--color", "always", "doctor"]).contains(ESC));
    assert!(!run(&w, &["--color", "never", "doctor"]).contains(ESC));
}

#[test]
fn no_color_is_honoured_and_an_explicit_request_still_wins() {
    let w = world();
    assert!(!run_with(&w, &["doctor"], &[("NO_COLOR", "1")]).contains(ESC));
    // The variable states a preference; passing the flag states an instruction.
    assert!(run_with(&w, &["--color", "always", "doctor"], &[("NO_COLOR", "1")]).contains(ESC));
}

#[test]
fn structured_output_is_never_coloured_whatever_was_asked_for() {
    let w = world();
    for args in [
        vec!["--color", "always", "--json", "doctor"],
        vec!["--color", "always", "find", "--format", "json"],
        vec!["--color", "always", "find", "--format", "paths"],
        vec!["--color", "always", "find", "--format", "resume"],
    ] {
        let text = run(&w, &args);
        assert!(!text.contains(ESC), "{args:?} dressed something meant to be parsed");
    }
}

#[test]
fn json_stays_parseable_with_colour_forced_on() {
    let w = world();
    let text = run(&w, &["--color", "always", "--json", "doctor"]);
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(parsed["schema"], 1);
}

#[test]
fn paths_output_is_usable_as_paths_with_colour_forced_on() {
    let w = world();
    let text = run(&w, &["--color", "always", "find", "--format", "paths"]);
    for line in text.lines() {
        let path = camino::Utf8Path::new(line);
        assert_eq!(path.extension(), Some("jsonl"), "{line:?}");
        assert!(path.exists(), "{line:?}");
    }
}

#[test]
fn stripping_the_colour_gives_back_the_plain_output() {
    // The property the whole approach rests on: colour repeats what the text
    // already says, so removing it loses nothing.
    let w = world();
    let colored = run(&w, &["--color", "always", "doctor"]);
    let plain = run(&w, &["--color", "never", "doctor"]);
    assert_eq!(unsilo::style::strip(&colored), plain);
}

#[test]
fn ascii_mode_leaves_no_box_drawing_behind() {
    let w = world();
    for args in [vec!["doctor"], vec!["find"], vec!["label", "--list"]] {
        let mut with_ascii = vec!["--ascii"];
        with_ascii.extend(args.iter().copied());
        let text = run(&w, &with_ascii);
        assert!(text.is_ascii(), "{args:?} emitted non-ascii: {text:?}");
    }
}

#[test]
fn an_unknown_colour_choice_is_a_usage_error() {
    let w = world();
    let mut cmd = Command::cargo_bin("unsilo").unwrap();
    cmd.env_clear();
    for (k, v) in w.env_pairs() {
        cmd.env(k, v);
    }
    cmd.args(["--color", "chartreuse", "doctor"]).assert().failure().code(2);
}

#[test]
fn the_table_columns_line_up_even_when_coloured() {
    // A coloured cell is longer in bytes than on screen, so padding has to count
    // what a reader sees. Getting that wrong shifts every column after it.
    let w = world();
    let coloured = unsilo::style::strip(&run(&w, &["--color", "always", "find"]));
    let plain = run(&w, &["--color", "never", "find"]);
    assert_eq!(coloured, plain, "colour changed the layout");

    let date_columns: Vec<usize> = plain.lines().filter_map(|line| line.find("2026-")).collect();
    assert!(date_columns.len() >= 2, "expected several rows");
    assert!(
        date_columns.windows(2).all(|pair| pair[0] == pair[1]),
        "dates start at different columns: {date_columns:?}"
    );
}
