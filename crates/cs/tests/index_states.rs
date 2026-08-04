//! What `cs search --json` says about the index it just read (chat-search-me9.28).
//!
//! Driven through the real binary, because the promise is about what reaches a client's
//! decoder: a code it can branch on, on stdout, beside a nonzero exit. A test that called
//! `commands::search` would assert on the shape of a `Result` and would keep passing if the
//! body never made it to stdout — which is the exact defect, since the first non-Rust client
//! to read this contract had to classify index health by grepping English (chat-search-me9.22).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A config pointing at an empty archive, so `cs index` produces a real but empty index in
/// well under a second.
struct Fixture {
    home: PathBuf,
    config: PathBuf,
    db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("cs-states-{}", uuid::Uuid::new_v4()));
        let source = home.join("sessions");
        std::fs::create_dir_all(&source).unwrap();
        let config = home.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "archive_root = \"{}\"\n\
                 machine_alias = \"test-box\"\n\
                 log_queries = false\n\
                 \n\
                 [[sources]]\n\
                 id = \"codex\"\n\
                 path = \"{}\"\n\
                 include = [\"**/*.jsonl\"]\n",
                home.join("archive").display(),
                source.display(),
            ),
        )
        .unwrap();
        Fixture { db: home.join("index.db"), home, config }
    }

    fn cs(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_cs"))
            .args(["--config", self.config.to_str().unwrap()])
            .args(args)
            // Candidate source paths are `~`-relative; a real HOME would drag whichever
            // agents are installed on this machine into the fixture.
            .env("HOME", &self.home)
            .output()
            .unwrap()
    }

    fn index(&self) {
        let out = self.cs(&["index", "--db", self.db.to_str().unwrap()]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    /// `cs search --json`, decoded — which is all a client has.
    fn search_json(&self, db: &Path) -> (bool, serde_json::Value) {
        let out = self.cs(&["search", "borrow", "--json", "--db", db.to_str().unwrap()]);
        let body = String::from_utf8(out.stdout).unwrap();
        let json = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("stdout was not JSON ({e}): {body:?}"));
        (out.status.success(), json)
    }
}

#[test]
fn a_missing_index_is_a_code_on_stdout_and_not_only_a_sentence_on_stderr() {
    let f = Fixture::new();
    let (ok, body) = f.search_json(&f.db);
    assert!(!ok, "a query with no index to read exited zero");
    assert_eq!(body["error"]["code"], "no_index");
}

#[test]
fn a_mistyped_db_path_does_not_become_a_new_empty_index() {
    let f = Fixture::new();
    let typo = f.home.join("indx.db");
    let (ok, body) = f.search_json(&typo);
    assert!(!ok);
    assert_eq!(body["error"]["code"], "no_index");
    // The failure this replaces answered "no results" out of a database it had just created,
    // which is indistinguishable from a corpus that genuinely holds nothing.
    assert!(!typo.exists(), "the query created an index at the path it could not find one at");
}

#[test]
fn a_complete_answer_says_so() {
    let f = Fixture::new();
    f.index();
    let (ok, body) = f.search_json(&f.db);
    assert!(ok, "{body}");
    assert_eq!(body["index_state"], "ready");
}

#[test]
fn a_rebuild_is_named_rather_than_left_for_the_client_to_infer() {
    let f = Fixture::new();
    f.index();
    // Standing in for a build that is mid-flight: what a reader consults is the claim, and
    // holding it is the only thing a running `cs index` does that a test can hold too.
    let claim = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(f.home.join("index.db.building.lock"))
        .unwrap();
    claim.try_lock().unwrap();

    let (ok, body) = f.search_json(&f.db);
    assert!(ok, "a rebuild made a query fail: {body}");
    // Still a real answer — the swap means what came back is the previous index in full — and
    // the state is what tells a client to ask again shortly.
    assert_eq!(body["index_state"], "rebuilding");
    assert!(body["results"].is_array());

    drop(claim);
    let (_, body) = f.search_json(&f.db);
    assert_eq!(body["index_state"], "ready", "the state outlived the build it described");
}

#[test]
fn a_first_build_is_not_reported_as_a_missing_index() {
    let f = Fixture::new();
    let claim = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(f.home.join("index.db.building.lock"))
        .unwrap();
    claim.try_lock().unwrap();

    let (ok, body) = f.search_json(&f.db);
    assert!(!ok);
    // The distinction the old message could not draw: this one says a `cs index` is already
    // running, where `no_index` says to start one.
    assert_eq!(body["error"]["code"], "building");
}

#[test]
fn litter_from_a_killed_build_does_not_report_a_build_forever() {
    let f = Fixture::new();
    f.index();
    // A `^C` mid-rebuild leaves both files behind and runs no destructor. Only the lock tells
    // them apart from a build still going.
    std::fs::write(f.home.join("index.db.building"), b"half an index").unwrap();
    std::fs::write(f.home.join("index.db.building.lock"), b"").unwrap();

    let (ok, body) = f.search_json(&f.db);
    assert!(ok, "{body}");
    assert_eq!(body["index_state"], "ready");
}

#[test]
fn the_state_can_be_asked_for_rather_than_discovered_by_failing_a_query() {
    let f = Fixture::new();
    // `cs status` reads the configured index, not `--db`, so point the config's default at
    // the same file this fixture builds.
    let default_db = f.home.join("archive").join("index.db");
    let state = |f: &Fixture| {
        let out = f.cs(&["status", "--json"]);
        let body: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        body["index"]["state"].as_str().unwrap().to_string()
    };
    assert_eq!(state(&f), "no_index");

    let out = f.cs(&["index", "--db", default_db.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(state(&f), "ready");
}

#[test]
fn a_rebuild_leaves_no_journal_beside_the_index_it_swapped_in() {
    let f = Fixture::new();
    f.index();
    // A `-wal` is found by path rather than by inode, so one left over from the replaced
    // database would be a log applied to a file it did not come from.
    for suffix in ["-wal", "-shm", ".building", ".building.lock"] {
        let mut p = f.db.as_os_str().to_os_string();
        p.push(suffix);
        assert!(!PathBuf::from(p).exists(), "{suffix} survived the swap");
    }
}
