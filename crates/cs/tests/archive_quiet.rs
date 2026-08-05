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
    /// Where an export-shaped source is unpacked, when the config declares one.
    exports: PathBuf,
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
        Self::build(|_| {
            if missing {
                "\n[[sources]]\nid = \"gemini-cli\"\npath = \"/nonexistent/gemini\"\n".into()
            } else {
                String::new()
            }
        })
    }

    /// The ordinary fixture: every server-side surface already configured.
    fn build(extra: impl FnOnce(&PathBuf) -> String) -> Self {
        Self::build_with(true, extra)
    }

    /// `server_side` declares the three surfaces no detection can find (chat-search-a7k.22),
    /// pointed at the export directory.
    ///
    /// Every test here wants them configured for the same reason [`Fixture::run`] empties
    /// `HOME`: an unconfigured surface is a standing report, and a quiet-mode test that has one
    /// standing is a test of that report instead. `false` is for the tests that *are* about it.
    ///
    /// The ids are the real ones and not invented names. Their absence from the archiver's
    /// candidate list is the property under test in two directions at once — it is what makes
    /// `chatgpt-export` export-shaped for the staleness nag (chat-search-a7k.10), and what makes
    /// all three undetectable here. An invented id would prove neither.
    fn build_with(server_side: bool, extra: impl FnOnce(&PathBuf) -> String) -> Self {
        let home = std::env::temp_dir().join(format!("cs-quiet-{}", uuid::Uuid::new_v4()));
        let source = home.join("sessions");
        let exports = home.join("exports");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&exports).unwrap();

        // Disjoint globs, so a file written for one surface is not captured twice and reported
        // as two stale sources.
        let surfaces = |exports: &PathBuf| {
            [
                ("chatgpt-export", "**/conversations-*.json"),
                ("claude-ai", "**/claude-ai-*.json"),
                ("google-takeout", "**/MyActivity.json"),
            ]
            .iter()
            .map(|(id, glob)| {
                format!(
                    "\n[[sources]]\nid = \"{id}\"\npath = \"{}\"\ninclude = [\"{glob}\"]\n",
                    exports.display(),
                )
            })
            .collect::<String>()
        };

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
            ) + &if server_side { surfaces(&exports) } else { String::new() }
                + &extra(&home),
        )
        .unwrap();

        Fixture { home, config, source, exports }
    }

    /// Append to the config the way a person acting on a printed block would.
    fn append_config(&self, toml: &str) {
        let mut text = std::fs::read_to_string(&self.config).unwrap();
        text.push('\n');
        text.push_str(toml);
        std::fs::write(&self.config, text).unwrap();
    }

    fn write(&self, rel: &str, body: &str) {
        self.write_into(&self.source, rel, body);
    }

    /// An unpacked export, backdated. Age is read off the file's mtime and not off when the
    /// archiver saw it, so backdating the file is what produces a stale export in a test that
    /// finishes in under a second.
    fn write_export(&self, rel: &str, days_old: u64) {
        self.write_into(&self.exports, rel, "{\"conversations\":[]}\n");
        let when =
            std::time::SystemTime::now() - std::time::Duration::from_secs(days_old * 24 * 60 * 60);
        let f = std::fs::File::options().write(true).open(self.exports.join(rel)).unwrap();
        f.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when)).unwrap();
    }

    fn write_into(&self, root: &PathBuf, rel: &str, body: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// Drop the throttle state of the exempt reports so the next run is due again, without
    /// touching anything the archiver would call an event. This is how a test reaches the case
    /// that actually matters — a warning standing alone under quiet mode — in two runs rather
    /// than in a day. Asserted present, because a silently absent file would turn every test
    /// that calls this into a test of the throttle instead of a test of the warning.
    fn expire_the_warnings(&self) {
        let dir = self.home.join("archive/raw/test-box");
        for state in [".staleness.json", ".drift.json", ".unreachable.json"] {
            let p = dir.join(state);
            if p.exists() {
                std::fs::remove_file(&p).unwrap();
            }
        }
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
fn an_export_that_stopped_is_named_on_an_otherwise_silent_run() {
    // The bead in one test (chat-search-a7k.10). An export unpacked a month ago and nothing
    // since: the run captures nothing, so quiet mode removes the table, and what is left has to
    // be the nag — naming the source and the age, because "something is stale" without saying
    // what or how badly is a line you learn to skip.
    let f = Fixture::new();
    f.write_export("conversations-2026-07-04.json", 30);
    f.stdout(&[]); // the capture run, which prints its table
    f.expire_the_warnings();

    let out = f.stdout(&[]);
    assert!(out.contains("chatgpt-export"), "the source is not named: {out:?}");
    assert!(out.contains("30 days"), "the age is not given: {out:?}");
    assert!(!out.contains("unchanged"), "the table came back with it:\n{out}");
    // Same contract as the drift report: with no table above it there is nothing to separate
    // from, and a lone leading newline is exactly the byte quiet mode exists to remove.
    assert!(!out.starts_with('\n'), "leading blank line: {out:?}");
}

#[test]
fn a_fresh_export_is_not_nagged_about() {
    // The threshold has to be a threshold. An export taken today is the state this whole check
    // is trying to produce, so it must be silent — otherwise the nag is unconditional and the
    // reader learns nothing from its presence.
    let f = Fixture::new();
    f.write_export("conversations-2026-08-04.json", 0);
    f.stdout(&[]); // the capture run, which prints its table
    f.expire_the_warnings();

    assert_eq!(f.stdout(&[]), "");
    // Nothing was reported, so nothing was recorded: a fresh export leaves no throttle state to
    // wait out, and falling behind later is news immediately rather than a day later.
    assert!(!f.home.join("archive/raw/test-box/.staleness.json").exists());
}

#[test]
fn a_live_tool_source_is_never_nagged_however_idle_it_is() {
    // The case that decides the design. `codex` here is 200 days idle, which on a real machine
    // is `gemini-cli`: a tool the user simply stopped using. It is not export-shaped — it is in
    // the archiver's candidate list, so something writes to it when it is used — and there is no
    // export to go and re-run. Nagging would be a daily instruction to do something impossible.
    let f = Fixture::new();
    f.write("proj/a.jsonl", "{\"one\":1}\n");
    let when = std::time::SystemTime::now()
        - std::time::Duration::from_secs(200 * 24 * 60 * 60);
    let p = f.source.join("proj/a.jsonl");
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
        .unwrap();
    f.stdout(&[]);

    assert_eq!(f.stdout(&[]), "", "a watched tool directory was nagged about");
}

#[test]
fn the_nag_does_not_repeat_on_every_run() {
    // 288 runs a day under launchd. A warning that prints on all of them is wallpaper, and the
    // export it names would still be there tomorrow either way.
    let f = Fixture::new();
    f.write_export("conversations-2026-07-04.json", 30);
    assert!(f.stdout(&[]).contains("chatgpt-export"), "the first sighting is news");
    assert_eq!(f.stdout(&[]), "", "and the second is not");
}

#[test]
fn the_nag_is_in_json_whether_or_not_it_was_due_to_print() {
    // Same rule as the drift report: the throttle keeps a human's log readable and has no
    // business emptying a machine-readable reply for the 24 h after a sighting.
    let f = Fixture::new();
    f.write_export("conversations-2026-07-04.json", 30);
    f.stdout(&[]);

    let v: serde_json::Value = serde_json::from_str(&f.stdout(&["--json"])).unwrap();
    assert_eq!(v["stale"][0]["source"], "chatgpt-export");
    assert_eq!(v["stale"][0]["days"], 30);
}

#[test]
fn the_drift_report_and_the_nag_do_not_run_into_each_other() {
    // Two exempt blocks below a suppressed table, which is the arrangement the blank-line rule
    // was not originally written for: each separates itself from what printed above it, and
    // neither may open the output with a stray newline.
    let f = Fixture::with_missing_source(true);
    f.write_export("conversations-2026-07-04.json", 30);
    f.stdout(&[]);
    f.expire_the_warnings();

    let out = f.stdout(&[]);
    assert!(out.contains("missing"), "{out}");
    assert!(out.contains("chatgpt-export"), "{out}");
    assert!(!out.starts_with('\n'), "leading blank line: {out:?}");
    assert!(!out.contains("\n\n\n"), "the two blocks stacked their separators:\n{out}");
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
fn a_surface_no_detection_can_find_is_named_on_the_first_run() {
    // chat-search-a7k.22 in one test. A config that watches every agent on the disk and nothing
    // else is what `cs init` writes on a second machine; the run captures nothing, so quiet mode
    // removes the table, and what is left has to name the surfaces and say what they are worth.
    // Without the share it is a line about plumbing, and a line about plumbing gets skipped.
    let f = Fixture::build_with(false, |_| String::new());
    let out = f.stdout(&[]);

    for id in ["chatgpt-export", "claude-ai", "google-takeout"] {
        assert!(out.contains(id), "{id} is not named: {out}");
    }
    assert!(out.contains("ChatGPT") && out.contains("claude.ai"), "{out}");
    assert!(out.contains("69%"), "the share is not given: {out}");
    assert!(!out.contains("unchanged"), "the table came back with it:\n{out}");
    assert!(!out.starts_with('\n'), "leading blank line: {out:?}");
}

#[test]
fn the_unreachable_report_does_not_repeat_on_every_run() {
    // The one standing condition here that may never resolve — nothing on the machine can make
    // it false, only a person can — which is exactly why it must not become wallpaper.
    let f = Fixture::build_with(false, |_| String::new());
    assert!(f.stdout(&[]).contains("chatgpt-export"), "the first sighting is news");
    assert_eq!(f.stdout(&[]), "", "and the second is not");
}

#[test]
fn pasting_the_block_it_printed_ends_the_report_for_that_surface() {
    // The loop closing, end to end and through the real binary: the block is taken from stdout,
    // pasted into the config the way a person would, and the surface stops being offered while
    // the others keep being. A block that `Config::load` rejected, or one whose id did not match
    // what `pending` looks for, would fail here rather than in a year of daily reminders.
    let f = Fixture::build_with(false, |_| String::new());
    let out = f.stdout(&[]);

    let block: String = out
        .lines()
        .skip_while(|l| !l.trim_start().starts_with("[[sources]]"))
        .map(|l| format!("{}\n", l.strip_prefix("      ").unwrap_or(l)))
        .collect();
    // Only the ChatGPT block, so the report is proven to shrink rather than vanish.
    let chatgpt: String = block.split("[[sources]]").take(2).collect::<Vec<_>>().join("[[sources]]");
    f.append_config(chatgpt.trim_start());
    f.expire_the_warnings();

    let after = f.stdout(&[]);
    assert!(!after.contains("unreachable   chatgpt-export"), "still offered: {after}");
    assert!(after.contains("unreachable   claude-ai"), "the rest went with it: {after}");
}

#[test]
fn the_unreachable_report_is_in_json_whether_or_not_it_was_due_to_print() {
    // Same rule as the other two: the throttle keeps a human's log readable and has no business
    // emptying a machine-readable reply for the 24 h after a sighting.
    let f = Fixture::build_with(false, |_| String::new());
    f.stdout(&[]);

    let v: serde_json::Value = serde_json::from_str(&f.stdout(&["--json"])).unwrap();
    let ids: Vec<&str> =
        v["unreachable"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["chatgpt-export", "claude-ai", "google-takeout"]);
}

#[test]
fn all_three_standing_reports_stack_without_running_into_each_other() {
    // Three exempt blocks below a suppressed table, which is one more than the blank-line rule
    // was written for: each separates itself from what printed above it, none may open the
    // output with a stray newline, and none may double the separator of the one before.
    let f = Fixture::build_with(false, |home| {
        format!(
            "\n[[sources]]\nid = \"gemini-cli\"\npath = \"/nonexistent/gemini\"\n\
             \n[[sources]]\nid = \"chatgpt-export\"\npath = \"{}\"\n\
             include = [\"**/conversations-*.json\"]\n",
            home.join("exports").display(),
        )
    });
    f.write_export("conversations-2026-07-04.json", 30);
    f.stdout(&[]);
    f.expire_the_warnings();

    let out = f.stdout(&[]);
    assert!(out.contains("missing       gemini-cli"), "drift is gone: {out}");
    assert!(out.contains("stale         chatgpt-export"), "the nag is gone: {out}");
    assert!(out.contains("unreachable   claude-ai"), "the surfaces are gone: {out}");
    assert!(!out.starts_with('\n'), "leading blank line: {out:?}");
    assert!(!out.contains("\n\n\n"), "the blocks stacked their separators:\n{out}");
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
