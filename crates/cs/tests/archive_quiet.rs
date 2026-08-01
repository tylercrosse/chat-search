//! What `cs archive` prints, and — mostly — does not (chat-search-a7k.5).
//!
//! Driven through the real binary rather than through `archive()`, because what this bead
//! promises is about bytes arriving in `~/Library/Logs/chat-search/archive.log`. A test that
//! called the function and inspected a returned `String` would be asserting on a refactor of
//! its own choosing, and would keep passing if a stray `println!` were added beside it.

use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    /// Doubles as `HOME`, so source detection finds nothing (see [`Fixture::run`]).
    home: PathBuf,
    config: PathBuf,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_missing_source(false)
    }

    /// `missing` adds a configured source whose directory does not exist, which is one of the
    /// two halves of the source-drift report (chat-search-a7k.12). It is the cheap way to get
    /// drift without depending on which agents are installed: the other half, an unconfigured
    /// candidate, is discovered under `HOME` and this fixture deliberately empties that.
    fn with_missing_source(missing: bool) -> Self {
        let home = std::env::temp_dir().join(format!("cs-quiet-{}", uuid::Uuid::new_v4()));
        let source = home.join("sessions");
        std::fs::create_dir_all(&source).unwrap();

        let config = home.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "archive_root = \"{}\"\n\
                 machine_alias = \"test-box\"\n\
                 \n\
                 [[sources]]\n\
                 id = \"codex\"\n\
                 path = \"{}\"\n\
                 include = [\"**/*.jsonl\"]\n",
                home.join("archive").display(),
                source.display(),
            ) + if missing {
                "\n[[sources]]\nid = \"gemini-cli\"\npath = \"/nonexistent/gemini\"\n"
            } else {
                ""
            },
        )
        .unwrap();

        Fixture { home, config, source }
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.source.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_cs"))
            .arg("archive")
            .args(["--config", self.config.to_str().unwrap()])
            .args(args)
            // Candidate source paths are `~`-relative, so a real HOME would make every
            // assertion below depend on which agents happen to be installed on the machine
            // running the test: their directories would surface as unconfigured drift and
            // print on a run that is supposed to be silent.
            .env("HOME", &self.home)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out
    }

    fn stdout(&self, args: &[&str]) -> String {
        String::from_utf8(self.run(args).stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

#[test]
fn a_run_that_observed_nothing_prints_nothing() {
    // The launchd case, 288 times a day. The first run has an empty source directory and the
    // second has an unchanged file; neither records an event, so neither says anything.
    let f = Fixture::new();
    assert_eq!(f.stdout(&[]), "");

    f.write("proj/a.jsonl", "{\"one\":1}\n");
    assert_ne!(f.stdout(&[]), "", "capturing a new file is news");
    assert_eq!(f.stdout(&[]), "", "the same file, unchanged, is not");
}

#[test]
fn a_run_that_captured_something_prints_the_whole_table() {
    let f = Fixture::new();
    f.write("proj/a.jsonl", "{\"one\":1}\n");
    let out = f.stdout(&[]);

    let columns = [
        "source", "new", "appended", "rewritten", "vanished", "unchanged", "cloned", "copied",
        "ms",
    ];
    for column in columns {
        assert!(out.contains(column), "column {column} missing from:\n{out}");
    }
    assert!(out.contains("  codex "), "{out}");
    assert!(out.contains("apparent size"), "{out}");
    assert!(out.contains("shares blocks with the source"), "{out}");
    assert!(out.contains("archive:"), "{out}");
}

#[test]
fn a_file_that_vanished_is_not_silence() {
    // Nothing is captured when a file disappears — but a transcript leaving the disk is the
    // single most expensive thing this project can fail to mention, and it is recorded as an
    // event, so it prints.
    let f = Fixture::new();
    f.write("proj/a.jsonl", "{\"one\":1}\n");
    f.stdout(&[]);
    std::fs::remove_file(f.source.join("proj/a.jsonl")).unwrap();

    let out = f.stdout(&[]);
    assert!(out.contains("  codex "), "a vanished file went unreported: {out:?}");
}

#[test]
fn verbose_restores_the_table_on_an_idle_run() {
    let f = Fixture::new();
    f.write("proj/a.jsonl", "{\"one\":1}\n");
    f.stdout(&[]);

    assert_eq!(f.stdout(&[]), "");
    assert!(f.stdout(&["--verbose"]).contains("  codex "));
}

#[test]
fn a_dry_run_answers_even_when_the_answer_is_nothing() {
    // Typed by hand, so silence would read as a broken binary rather than as good news.
    let f = Fixture::new();
    let out = f.stdout(&["--dry-run"]);
    assert!(out.contains("dry run — nothing written"), "{out}");
    assert!(out.contains("  codex "), "{out}");
}

#[test]
fn the_drift_report_survives_an_otherwise_silent_run() {
    // The point of the exemption: quiet mode removes noise, and a warning it could swallow
    // would be worse than the noise. The staleness nag (chat-search-a7k.10) lands here too.
    let f = Fixture::with_missing_source(true);
    let out = f.stdout(&[]);

    assert!(out.contains("missing"), "{out}");
    assert!(out.contains("gemini-cli"), "{out}");
    assert!(!out.contains("unchanged"), "the table came back with it:\n{out}");
    // The blank line above the report belongs to the table it separates from. With no table
    // there is nothing to separate, and a lone leading newline is exactly the kind of stray
    // byte quiet mode exists to remove.
    assert!(!out.starts_with('\n'), "leading blank line: {out:?}");
}

#[test]
fn json_is_never_quieted() {
    // A consumer polling `--json` wants the current state on every poll. Quiet mode exists to
    // keep a human's log readable and has no business emptying a machine-readable reply.
    let f = Fixture::new();
    let out = f.stdout(&["--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["sources"][0]["source"], "codex");
}

#[test]
fn an_error_still_speaks_on_an_otherwise_silent_run() {
    let f = Fixture::new();
    let out = Command::new(env!("CARGO_BIN_EXE_cs"))
        .args(["archive", "--config", "/nonexistent/config.toml"])
        .env("HOME", &f.home)
        .output()
        .unwrap();

    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("/nonexistent/config.toml"),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}
