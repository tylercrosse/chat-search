//! What `cs init` does to a config that is already there (chat-search-a7k.30).
//!
//! Driven through the real binary, because the promise is about a file on disk surviving a
//! command: a test that called `init()` and read the `Result` would keep passing if the save
//! moved one line earlier and truncated the file before the check ran. The assertions here
//! compare the bytes.
//!
//! `HOME` is a temporary directory in every test, for the reason `archive_quiet.rs` gives —
//! candidate source paths are `~`-relative, so a real one would make detection depend on which
//! agents happen to be installed on the machine running the test, and detection is exactly the
//! thing under test.

use std::path::PathBuf;
use std::process::Output;

struct Fixture {
    home: PathBuf,
    config: PathBuf,
}

impl Fixture {
    /// A config whose only difference from what detection would write is `body`.
    ///
    /// `archive_root` is spelled out as the default under this `HOME` so it never shows up as a
    /// loss of its own, and there is no comment in it for the same reason: a fixture that
    /// differed in two ways would pass whichever assertion it was making for the wrong reason.
    fn with(body: &str) -> Self {
        let home = std::env::temp_dir().join(format!("cs-init-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let config = home.join("config.toml");
        std::fs::write(
            &config,
            format!("archive_root = \"{}\"\n{body}", home.join(".chat-archive").display()),
        )
        .unwrap();
        Fixture { home, config }
    }

    /// An export unpacked somewhere only this file records.
    fn with_export() -> Self {
        Self::with(&format!(
            "\n[[sources]]\nid = \"chatgpt-export\"\npath = \"{}\"\ninclude = [\"**/conversations-*.json\"]\n",
            std::env::temp_dir().join("wherever-i-unpacked-it").display(),
        ))
    }

    /// Make a candidate detectable, so detection has something to offer.
    fn install(&self, rel: &str) {
        std::fs::create_dir_all(self.home.join(rel)).unwrap();
    }

    fn read(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap()
    }

    fn init(&self, args: &[&str]) -> Output {
        std::process::Command::new(env!("CARGO_BIN_EXE_cs"))
            .arg("init")
            .args(["--config", self.config.to_str().unwrap()])
            .args(args)
            .env("HOME", &self.home)
            .output()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_config_carrying_a_hand_added_export_survives_force() {
    // The bug. `--force` built a `Config::default()` and saved it, so the one block no
    // detection can ever put back was deleted by a command that said nothing about it.
    let f = Fixture::with_export();
    let before = f.read();

    let out = f.init(&["--force"]);
    assert!(!out.status.success(), "--force overwrote it: {}", stderr(&out));
    assert_eq!(f.read(), before, "the file was rewritten");

    let err = stderr(&out);
    assert!(err.contains("chatgpt-export"), "{err}");
    assert!(err.contains("refusing to overwrite"), "{err}");
    // The reason, not just the name: an export is unreachable by construction, and the line
    // has to say so or it reads as a bug in detection somebody should wait for a fix to.
    assert!(err.contains("no detection can find this surface"), "{err}");
    // And the way out, since refusing is only half an answer without one.
    assert!(err.contains(".bak"), "{err}");
}

#[test]
fn the_refusal_arrives_before_force_is_reached_for() {
    // `already exists (use --force to overwrite)` was true and is now a trap: --force is the
    // thing that deletes the block. Somebody who types `cs init` by mistake should be told what
    // the flag would cost at the moment they are deciding whether to type it.
    let f = Fixture::with_export();
    let before = f.read();

    let out = f.init(&[]);
    assert!(!out.status.success());
    assert_eq!(f.read(), before);
    assert!(stderr(&out).contains("chatgpt-export"), "{}", stderr(&out));
}

#[test]
fn force_still_re_runs_detection_when_nothing_would_be_lost() {
    // What the flag is for, and it still works: a config that names less than the disk does is
    // overwritten with the source that appeared since it was written.
    let f = Fixture::with("");
    f.install(".codex/sessions");

    let out = f.init(&["--force"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.read().contains("id = \"codex\""), "{}", f.read());
}

#[test]
fn a_config_that_is_already_current_still_needs_the_flag() {
    // The old refusal, unchanged for the case it was written for: nothing would be lost, so the
    // only question left is whether the overwrite was asked for.
    let f = Fixture::with("");
    let out = f.init(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("use --force"), "{}", stderr(&out));
}

#[test]
fn an_edited_source_path_is_not_quietly_reverted() {
    // `codex` is a candidate, so the id survives an overwrite and the path does not. A volume
    // that is not where the candidate list expects it is as unrecoverable as an export.
    let f = Fixture::with(
        "\n[[sources]]\nid = \"codex\"\npath = \"/Volumes/ext/.codex/sessions\"\ninclude = [\"**/*.jsonl\"]\n",
    );
    f.install(".codex/sessions");
    let before = f.read();

    let out = f.init(&["--force"]);
    assert!(!out.status.success(), "{}", stderr(&out));
    assert_eq!(f.read(), before);
    let err = stderr(&out);
    assert!(err.contains("rewritten"), "{err}");
    assert!(err.contains("/Volumes/ext/.codex/sessions"), "{err}");
    assert!(err.contains("would become"), "{err}");
}

#[test]
fn an_alias_is_kept_by_restating_it() {
    // The escape hatch `cs init` already had, now that dropping the alias is refused: the flag
    // that sets it is also the way to say "this one, again".
    let f = Fixture::with("machine_alias = \"laptop\"\n");

    let out = f.init(&["--force"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--machine-alias laptop"), "{}", stderr(&out));

    let out = f.init(&["--force", "--machine-alias", "laptop"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(f.read().contains("machine_alias = \"laptop\""), "{}", f.read());
}

#[test]
fn the_reasons_written_beside_the_values_are_not_thrown_away() {
    // A config whose values detection reproduces exactly, annotated. Everything `cs init` would
    // write is already in it, so the overwrite is harmless in the only terms a `Config` has —
    // and it would still delete the sentence saying why the directory is where it is.
    let f = Fixture::with("# the exports live on the big volume\n");
    let before = f.read();

    let out = f.init(&["--force"]);
    assert!(!out.status.success(), "{}", stderr(&out));
    assert_eq!(f.read(), before);
    assert!(stderr(&out).contains("comments       1 line "), "{}", stderr(&out));
}

#[test]
fn a_config_that_does_not_parse_is_not_overwritten_either() {
    // The one case where the report cannot be produced, and so the one place a merge-based fix
    // would have had to overwrite blind: an unreadable file may still be the only record of
    // where an export was unpacked.
    let f = Fixture::with("[[sources]\nid = broken\n");
    let before = f.read();

    let out = f.init(&["--force"]);
    assert!(!out.status.success());
    assert_eq!(f.read(), before);
    assert!(stderr(&out).contains("until it parses"), "{}", stderr(&out));
}
