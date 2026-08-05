//! What reaches `queries.jsonl`, and — the half that keeps it readable — what does not.
//!
//! docs/TUI-DESIGN.md §6 gives an interactive client three moments and only two events: a
//! keystroke records **nothing**, `Enter` on a row records a `Pick`, and quitting with a
//! non-blank query and no pick records a `Search`. That third row is the abandonment signal,
//! and until `cs abandon` there was no way for a client outside this process to emit it — so
//! the log recorded only the searches that worked.
//!
//! Driven through the real binary, because the whole point is the side effect on a file. A test
//! that called `commands::abandon` would still pass if the subcommand were never wired up, which
//! is exactly the failure a client hits: it spawns `cs abandon`, gets exit 0 from clap refusing
//! to parse, and records nothing forever.

use std::path::PathBuf;
use std::process::Command;

use cs_core::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;

/// An index with three conversations in it and a query log beside it, both under a temp root.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    /// `log_queries` is the config switch a client cannot see, so both settings get a fixture.
    fn new(log_queries: bool) -> Fixture {
        let dir = std::env::temp_dir().join(format!("cs-query-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = Fixture { dir };
        std::fs::write(
            f.config(),
            format!(
                "archive_root = \"{}\"\nmachine_alias = \"test-box\"\nlog_queries = {}\n",
                f.dir.display(),
                log_queries
            ),
        )
        .unwrap();
        let mut conn = cs_core::open(f.db().to_str().unwrap()).unwrap();
        cs_core::write_conversations(&mut conn, corpus().iter()).unwrap();
        f
    }

    fn config(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    fn db(&self) -> PathBuf {
        self.dir.join("index.db")
    }

    /// `archive_root` is the temp directory, so this is where `Config::query_log` resolves to.
    fn log(&self) -> PathBuf {
        self.dir.join("queries.jsonl")
    }

    fn cs(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cs"));
        cmd.args(["--config", self.config().to_str().unwrap()])
            .args(args)
            .args(["--db", self.db().to_str().unwrap()]);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn abandon(&self, query: &str) -> std::process::Output {
        self.cs(&["abandon", query], &[])
    }

    /// The conversations this query returns, best first, without recording that it was asked.
    ///
    /// What `shown` is checked against, rather than a ranking spelled out here: the claim is
    /// that an abandonment names the list the ranking would have offered, and an expected order
    /// written into the test would turn every future ranking change into a failure of this file.
    fn ranked(&self, query: &str) -> Vec<Value> {
        let out = self.cs(&["search", query, "--json"], &[("CS_LOG_QUERIES", "0")]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let body: Value = serde_json::from_slice(&out.stdout).unwrap();
        body["results"].as_array().unwrap().iter().map(|g| g["conv_id"].clone()).collect()
    }

    /// Every event in the log, oldest first. A missing file is an empty history rather than a
    /// failure, because "nothing was recorded" is the assertion half of these tests make.
    fn events(&self) -> Vec<Value> {
        let Ok(body) = std::fs::read_to_string(self.log()) else {
            return Vec::new();
        };
        body.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l:?}")))
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn giving_up_on_a_query_is_recorded_as_one_search_over_the_list_it_showed() {
    let f = Fixture::new(true);
    let out = f.abandon("borrow checker");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let events = f.events();
    assert_eq!(events.len(), 1, "{events:?}");
    let e = &events[0];
    assert_eq!(e["kind"], "search");
    assert_eq!(e["q"], "borrow checker", "the text that was given up on, not a normalised form");
    // The list is recomputed on this side, so the event says what the ranking offers for the
    // finished query rather than what some client's typeahead happened to be showing.
    let ranked = f.ranked("borrow checker");
    assert!(ranked.len() > 1, "a one-row fixture would not show the order was kept");
    assert_eq!(e["shown"], Value::Array(ranked.clone()));
    assert_eq!(e["n"], ranked.len());
    assert!(e["ms"].as_f64().is_some(), "an abandoned need still costs what it cost to answer");
}

#[test]
fn nothing_is_recorded_for_a_query_there_was_no_need_behind() {
    // `Mode::Empty`'s three shapes. A window closed on an empty box is the ordinary end of a
    // session, not a statement that the ranking failed, and recording one would hand the eval
    // set a need nobody had.
    let f = Fixture::new(true);
    for query in ["", "   ", "??"] {
        let out = f.abandon(query);
        assert!(out.status.success(), "{query:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    assert!(f.events().is_empty(), "{:?}", f.events());
}

#[test]
fn a_query_that_starts_with_a_negation_is_carried_rather_than_read_as_flags() {
    // The defect `cs pick` already had: `-agent:codex` is one of the DSL's two negation
    // spellings and clap reads a leading `-` as a bundle of short flags, so the abandonment of
    // exactly the queries a facet rail produces would never have been recorded.
    let f = Fixture::new(true);
    let out = f.abandon("-agent:codex borrow");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let events = f.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["q"], "-agent:codex borrow");
    // The filter was applied and not searched as text: the two codex rows are excluded.
    assert_eq!(events[0]["shown"], serde_json::json!(["chatgpt-export:elsewhere"]));
}

#[test]
fn a_driven_run_records_no_abandonment_either() {
    // `CS_LOG_QUERIES=0` wraps a whole measurement session, subprocesses included. It has to
    // reach this verb too: a benchmark quits with its last phrase still in the box, so the one
    // thing a measurement run would append is an abandonment nobody abandoned.
    let f = Fixture::new(true);
    let out = f.cs(&["abandon", "borrow checker"], &[("CS_LOG_QUERIES", "0")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(f.events().is_empty(), "{:?}", f.events());
}

#[test]
fn a_config_that_records_nothing_is_not_overridden_by_a_client_asking_to_record() {
    let f = Fixture::new(false);
    let out = f.abandon("borrow checker");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(f.events().is_empty(), "{:?}", f.events());
}

#[test]
fn typing_appends_nothing() {
    // The other half of §6's table, and the rule `cs abandon` exists to work around rather than
    // to relax: a typeahead client fires one `cs search --prefix` per character, and a line each
    // would bury the handful of real queries under every prefix of every one of them.
    let f = Fixture::new(true);
    for query in ["b", "bo", "bor", "borr", "borrow"] {
        let out = f.cs(&["search", query, "--prefix", "--json"], &[]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    assert!(f.events().is_empty(), "{:?}", f.events());

    // The same query without `--prefix` is a finished search somebody ran, and that one counts.
    let out = f.cs(&["search", "borrow", "--json"], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(f.events().len(), 1);
}

// ---------------------------------------------------------------- the fixture

fn msg(seq: i64, role: Role, text: &str) -> Message {
    Message {
        native_id: format!("m{seq}"),
        parent_native_id: (seq > 0).then(|| format!("m{}", seq - 1)),
        thread_key: "main".into(),
        is_sidechain: false,
        is_error: false,
        seq,
        role,
        kind: Kind::Prose,
        ts: Some(1_785_000_000_000 + seq),
        model: None,
        text: text.into(),
    }
}

fn conv(source: &str, native_id: &str, title: &str, messages: Vec<Message>) -> Conversation {
    Conversation {
        source: source.into(),
        native_id: native_id.into(),
        titles: Titles { first_user: Some(title.into()), ..Default::default() },
        cwd: None,
        git_branch: None,
        declared_model: None,
        surface: None,
        forked_from_native_id: None,
        head_native_id: None,
        messages,
    }
}

/// Two conversations one query reaches and one it reaches only with the `agent:` filter dropped,
/// so `shown` is an ordering a test can state rather than a single row that would pass whatever
/// the ranking did.
fn corpus() -> Vec<Conversation> {
    vec![
        conv("codex", "ordinary", "why does the borrow checker reject this", vec![
            msg(0, Role::User, "why does the borrow checker reject this"),
            msg(1, Role::Assistant, "the borrow checker wants a shorter lifetime"),
        ]),
        conv("codex", "undated", "borrow again", vec![msg(0, Role::User, "the borrow checker")]),
        conv("chatgpt-export", "elsewhere", "a borrow of the same value", vec![
            msg(0, Role::User, "a borrow of the same value"),
        ]),
    ]
}
