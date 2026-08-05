//! What `cs status` says about every source the machine knows about (chat-search-a7k.29).
//!
//! Driven through the real binary because the point of the bead is that the answer *reaches a
//! client*. The join is unit-tested in `cs_core::inventory`; what cannot be tested there is
//! that the config half arrives at all — the states below come from three places (the config
//! file, the filesystem, the index) and only the binary has all three.

use std::path::PathBuf;
use std::process::Command;

use cs_core::model::{Conversation, Kind, Message, Role, Titles};

/// One configured source with an empty directory, one configured source whose directory is
/// gone, one candidate detected under `HOME`, and an index holding rows for a source the
/// config has never heard of. Every state, on one machine.
struct Fixture {
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("cs-inventory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        // A candidate `cs init` never adopted. Detection is `~`-relative, so this is only
        // reachable because every `cs` below runs with HOME pointed here.
        std::fs::create_dir_all(home.join(".claude").join("projects")).unwrap();
        std::fs::create_dir_all(home.join("archive")).unwrap();

        let f = Fixture { home };
        std::fs::write(
            f.config(),
            format!(
                "archive_root = \"{}\"\n\
                 machine_alias = \"test-box\"\n\
                 log_queries = false\n\
                 \n\
                 [[sources]]\n\
                 id = \"codex\"\n\
                 path = \"{}\"\n\
                 include = [\"**/*.jsonl\"]\n\
                 \n\
                 [[sources]]\n\
                 id = \"gemini-cli\"\n\
                 path = \"{}\"\n\
                 include = [\"**/*.json\"]\n",
                f.home.join("archive").display(),
                f.home.join("sessions").display(),
                f.home.join("uninstalled").display(),
            ),
        )
        .unwrap();

        // Written where `cfg.default_db()` resolves, because `cs status` reports the
        // configured index rather than one named on the command line.
        let mut conn = cs_core::open(f.db().to_str().unwrap()).unwrap();
        cs_core::write_conversations(&mut conn, corpus().iter()).unwrap();
        f
    }

    fn config(&self) -> PathBuf {
        self.home.join("config.toml")
    }

    fn db(&self) -> PathBuf {
        self.home.join("archive").join("index.db")
    }

    fn status(&self, extra: &[&str]) -> std::process::Output {
        let out = Command::new(env!("CARGO_BIN_EXE_cs"))
            .args(["--config", self.config().to_str().unwrap(), "status"])
            .args(extra)
            .env("HOME", &self.home)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        out
    }

    /// The inventory entry for one source id, or `None` if it is not in the answer at all.
    fn source(&self, id: &str) -> Option<serde_json::Value> {
        let body: serde_json::Value =
            serde_json::from_slice(&self.status(&["--json"]).stdout).unwrap();
        body["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

#[test]
fn a_configured_source_with_no_rows_is_not_the_same_as_one_nobody_configured() {
    // The bead in one test. Both hold nothing; one is a capture path that has not produced
    // anything yet or has a broken importer, the other is an agent the user does not run.
    let f = Fixture::new();
    let empty = f.source("codex").expect("a configured source is in the inventory");
    assert_eq!(empty["coverage"], "live");
    assert_eq!(empty["conversations"], 0);

    let candidate = f.source("claude-code").expect("a detected candidate is in the inventory");
    assert_eq!(candidate["coverage"], "unconfigured");
    assert_eq!(candidate["conversations"], 0);
}

#[test]
fn a_configured_source_whose_directory_is_gone_is_reported_rather_than_dropped() {
    let f = Fixture::new();
    let gone = f.source("gemini-cli").unwrap();
    assert_eq!(gone["coverage"], "missing");
}

#[test]
fn a_source_only_the_index_remembers_keeps_its_count() {
    // `chatgpt-export` archived and indexed, then dropped from the config. Its conversations
    // are still searchable, so a client that listed only configured sources would offer no way
    // to reach them.
    let f = Fixture::new();
    let retired = f.source("chatgpt-export").expect("an indexed source is in the inventory");
    assert_eq!(retired["coverage"], "retired");
    assert_eq!(retired["conversations"], 2);
    // Nothing on this machine claims it, so there is no path to report.
    assert_eq!(retired["path"], serde_json::Value::Null);
}

#[test]
fn the_readable_table_names_the_empty_source_rather_than_omitting_it() {
    // The half that made this invisible: a table built from the index alone has no row to
    // print for a source that produced nothing, so the failure looks like a clean run.
    let f = Fixture::new();
    let text = String::from_utf8(f.status(&[]).stdout).unwrap();
    let row = text.lines().find(|l| l.contains("codex")).expect("codex has a row");
    let fields: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(&fields[..3], ["live", "codex", "0"], "{row}");
}

fn corpus() -> Vec<Conversation> {
    vec![
        conv("chatgpt-export", "one"),
        conv("chatgpt-export", "two"),
    ]
}

fn conv(source: &str, native_id: &str) -> Conversation {
    Conversation {
        source: source.into(),
        native_id: native_id.into(),
        titles: Titles { first_user: Some("hello".into()), ..Default::default() },
        cwd: None,
        git_branch: None,
        model: None,
        surface: None,
        forked_from_native_id: None,
        head_native_id: None,
        messages: vec![Message {
            native_id: "m1".into(),
            parent_native_id: None,
            thread_key: "t".into(),
            is_sidechain: false,
            is_error: false,
            seq: 0,
            role: Role::User,
            kind: Kind::Prose,
            ts: Some(1_785_000_000_000),
            text: "hello".into(),
        }],
    }
}
