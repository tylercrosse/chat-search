//! What `cs search --json` promises, checked against the document that promises it.
//!
//! [`docs/JSON-CONTRACT.md`] is the client-facing statement of this contract (ADR 12, ADR 14).
//! This test reads that file's own tables rather than restating them, so the answer lives in
//! exactly one place — a copy of the field list in Rust would be a second place, and an answer
//! living nowhere a client can read it is the defect being fixed. Adding a field to `Hit` or
//! `Group` fails here until the table gains a row for it.
//!
//! Nullability is the half worth pinning hardest. `chat-search-me9.27`: the Swift spike typed
//! `title` as a non-optional `String`, passed every hand test, and threw at `results[54]` of a
//! `--limit 60` query, because 11 of 3,059 conversations are untitled and not one of them is
//! near the top of anything. So both directions are asserted — a key the document calls `never`
//! is null nowhere in the response, and a key it calls nullable **is** null somewhere in a
//! fixture shaped like the real corpus. Without the second half the document would quietly
//! drift into describing states the code stopped producing, which is the same failure one
//! remove.
//!
//! Driven through the real binary, because the contract is the bytes on stdout. Calling
//! `search_grouped` and serializing the result would miss the envelope entirely, and the
//! envelope is where `grouped` and `unapplied_filters` live.
//!
//! [`docs/JSON-CONTRACT.md`]: ../../../docs/JSON-CONTRACT.md

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use cs_core::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;

// --------------------------------------------------------------- the document

/// What the document says about one key, which is three states rather than two: a key that is
/// sometimes absent is a different type to a decoder than one that is sometimes null.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nullability {
    Never,
    Nullable,
    Absent,
}

impl Nullability {
    /// Read the `null?` cell. Deliberately forgiving about prose around the word and unforgiving
    /// about its absence: a cell nobody can classify is a row nobody can act on, so it fails
    /// here rather than being silently skipped.
    fn read(cell: &str) -> Nullability {
        let cell = cell.to_ascii_lowercase();
        if cell.contains("nullable") {
            Nullability::Nullable
        } else if cell.contains("absent") {
            Nullability::Absent
        } else if cell.starts_with("never") {
            Nullability::Never
        } else {
            panic!("docs/JSON-CONTRACT.md: cannot read the nullability cell {cell:?}");
        }
    }
}

fn contract() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/JSON-CONTRACT.md");
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// The field table under one heading, as `key -> nullability`.
///
/// `heading` is matched as a prefix so a section can gain a subtitle without breaking this.
/// Only rows that open with a backticked key are read, which skips the header and rule rows
/// and every prose table elsewhere in the file.
fn documented(heading: &str) -> BTreeMap<String, Nullability> {
    let doc = contract();
    let after = doc
        .split_once(heading)
        .unwrap_or_else(|| panic!("docs/JSON-CONTRACT.md has no section starting {heading:?}"))
        .1;
    let section = after.split("\n## ").next().unwrap();

    let table: BTreeMap<String, Nullability> = section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            (cells[1].trim_matches('`').to_string(), Nullability::read(cells[3]))
        })
        .collect();

    assert!(!table.is_empty(), "docs/JSON-CONTRACT.md: no field table under {heading:?}");
    table
}

fn keys_where(
    table: &BTreeMap<String, Nullability>,
    want: impl Fn(Nullability) -> bool,
) -> BTreeSet<String> {
    table.iter().filter(|(_, n)| want(**n)).map(|(k, _)| k.clone()).collect()
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected an object, got {value}"))
        .keys()
        .cloned()
        .collect()
}

// ---------------------------------------------------------------- the fixture

/// An index whose conversations are the *causes* of every state the document describes, not
/// merely values that produce them.
///
/// Each one is modelled on a group of real rows, so a change that makes a state impossible at
/// the source fails this test with the reason visible: the untitled conversation is untitled
/// because every candidate title was harness machinery, exactly as the 11 real ones are.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!("cs-json-contract-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = Fixture { dir };

        std::fs::write(
            f.config(),
            format!(
                "archive_root = \"{}\"\n\
                 machine_alias = \"test-box\"\n\
                 # A search must not append to a real query log from a test run.\n\
                 log_queries = false\n",
                f.dir.display()
            ),
        )
        .unwrap();

        // `open`, not the retired `open_fresh`: chat-search-me9.28 moved rebuilds behind
        // `IndexBuild`, which assembles a sibling and swaps it in. There is nothing to clear
        // here — the fixture owns a fresh temp directory — so the deletion `open_fresh` did
        // first has no work to do, and going through a build would test the build rather than
        // the contract this file is about.
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

    fn search(&self, query: &str, extra: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_cs"))
            .args(["search", query, "--json", "--limit", "50"])
            .args(["--db", self.db().to_str().unwrap()])
            .args(["--config", self.config().to_str().unwrap()])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

const TERM: &str = "borrow";

fn msg(seq: i64, role: Role, kind: Kind, ts: Option<i64>, text: &str) -> Message {
    Message {
        native_id: format!("m{seq}"),
        parent_native_id: (seq > 0).then(|| format!("m{}", seq - 1)),
        thread_key: "main".into(),
        is_sidechain: false,
        is_error: false,
        seq,
        role,
        kind,
        ts,
        text: text.into(),
    }
}

fn conv(source: &str, native_id: &str, titles: Titles, messages: Vec<Message>) -> Conversation {
    Conversation {
        source: source.into(),
        native_id: native_id.into(),
        titles,
        cwd: None,
        git_branch: None,
        model: None,
        surface: None,
        forked_from_native_id: None,
        head_native_id: None,
        messages,
    }
}

fn titled(title: &str) -> Titles {
    Titles { first_user: Some(title.into()), ..Default::default() }
}

fn corpus() -> Vec<Conversation> {
    let t = 1_785_000_000_000;

    // An ordinary conversation with everything present, and the only one carrying a `cwd`. Its
    // successful tool result is what makes the `kind_runs` lengths sum to less than `msg_count`.
    let mut ordinary = conv(
        "codex",
        "ordinary",
        titled("why does the borrow checker reject this"),
        vec![
            msg(0, Role::User, Kind::Prose, Some(t), "why does the borrow checker reject this"),
            msg(1, Role::Assistant, Kind::ToolCall, Some(t + 1), "read src/lib.rs"),
            msg(2, Role::Tool, Kind::ToolResult, Some(t + 2), "fn main() {}"),
            msg(3, Role::Assistant, Kind::Prose, Some(t + 3), "an em-dash — then borrow checker"),
        ],
    );
    ordinary.cwd = Some("/tmp/proj".into());

    // Untitled: every title candidate was machinery. Modelled on the gemini-cli row, which has
    // no user turn at all — the agent said something searchable and the person never did, so
    // `Titles::resolve` has nothing to return.
    let untitled = conv(
        "codex",
        "only-machinery",
        Titles::default(),
        vec![
            msg(0, Role::User, Kind::Prose, Some(t), "/status"),
            msg(1, Role::System, Kind::Prose, Some(t + 1), "borrow limit reached"),
        ],
    );

    // No timestamps anywhere, so `ended_at` has no maximum to take and every hit is undated.
    // Never seen on this corpus and permitted by every importer, which is exactly the shape a
    // client types non-optional on the evidence of a `SELECT`.
    let undated = conv(
        "codex",
        "undated",
        titled("undated session"),
        vec![msg(0, Role::User, Kind::Prose, None, "the borrow checker again")],
    );

    // Opened and abandoned: a titled conversation with no messages, which is where the two real
    // null `ended_at`s come from. It matches no query, so only the no-query list ever shows it.
    let abandoned = conv(
        "chatgpt-export",
        "abandoned",
        Titles { custom: Some("New chat".into()), ..Default::default() },
        vec![],
    );

    // A source with no known way to reopen a conversation, which is `destinations: []` — an
    // empty array that means something, and not a null.
    let unreachable = conv(
        "gemini-cli",
        "no-destination",
        titled("gemini session"),
        vec![msg(0, Role::Assistant, Kind::Prose, Some(t), "a borrow of the same value")],
    );

    vec![ordinary, untitled, undated, abandoned, unreachable]
}

/// Every object the three shapes of response produce, flattened: envelopes are excluded, and
/// hits are yielded from both the nested and the flat shape because the document claims they
/// are the same object in both places.
fn rows(f: &Fixture) -> (Vec<Value>, Vec<Value>) {
    let mut groups = Vec::new();
    let mut hits = Vec::new();

    for query in ["", TERM] {
        for group in f.search(query, &[]).get("results").unwrap().as_array().unwrap() {
            hits.extend(group.get("hits").unwrap().as_array().unwrap().iter().cloned());
            groups.push(group.clone());
        }
    }
    hits.extend(f.search(TERM, &["--flat"]).get("results").unwrap().as_array().unwrap().clone());

    assert!(!groups.is_empty() && !hits.is_empty(), "the fixture returned nothing to check");
    (groups, hits)
}

// ------------------------------------------------------------------ the pins

#[test]
fn the_binary_emits_exactly_the_keys_the_contract_documents() {
    let f = Fixture::new();
    let (groups, hits) = rows(&f);

    for (label, table, rows) in [
        ("Group", documented("## `Group`"), groups),
        ("Hit", documented("## `Hit`"), hits),
    ] {
        let expected: BTreeSet<String> = table.keys().cloned().collect();
        for row in rows {
            assert_eq!(
                keys(&row),
                expected,
                "{label} and docs/JSON-CONTRACT.md disagree about which keys exist"
            );
        }
    }
}

#[test]
fn the_envelope_carries_grouped_and_flat_drops_it() {
    // The one key the document calls *absent* rather than null, and the reason a client cannot
    // model both shapes with one non-optional field (chat-search-me9.32).
    let f = Fixture::new();
    let table = documented("## The envelope");
    let always = keys_where(&table, |n| n != Nullability::Absent);
    let absent_when_flat = keys_where(&table, |n| n == Nullability::Absent);

    assert!(!absent_when_flat.is_empty(), "the envelope table stopped documenting an absent key");
    assert_eq!(keys(&f.search(TERM, &[])), table.keys().cloned().collect::<BTreeSet<_>>());
    assert_eq!(keys(&f.search(TERM, &["--flat"])), always);
}

#[test]
fn a_key_the_contract_calls_never_null_is_null_nowhere() {
    let f = Fixture::new();
    let (groups, hits) = rows(&f);

    for (label, table, rows) in [
        ("Group", documented("## `Group`"), groups),
        ("Hit", documented("## `Hit`"), hits),
    ] {
        for key in keys_where(&table, |n| n == Nullability::Never) {
            for row in &rows {
                assert!(
                    !row[&key].is_null(),
                    "{label}.{key} is documented as never null and came back null:\n{row:#}"
                );
            }
        }
    }
}

#[test]
fn every_key_the_contract_calls_nullable_is_null_somewhere() {
    // The half that stops the document rotting. A field made non-null at the source — `""` for a
    // missing title, say — would leave every other assertion here passing while the document
    // told a client to handle a state that can no longer happen.
    let f = Fixture::new();
    let (groups, hits) = rows(&f);

    for (label, table, rows) in [
        ("Group", documented("## `Group`"), groups),
        ("Hit", documented("## `Hit`"), hits),
    ] {
        for key in keys_where(&table, |n| n == Nullability::Nullable) {
            assert!(
                rows.iter().any(|row| row[&key].is_null()),
                "{label}.{key} is documented as nullable and nothing in the fixture produced a \
                 null. Either the fixture stopped covering it or the field stopped being nullable \
                 — in the second case docs/JSON-CONTRACT.md is now wrong."
            );
        }
    }
}

#[test]
fn an_empty_array_is_not_a_null() {
    let f = Fixture::new();
    let browse = f.search("", &[]);
    let results = browse["results"].as_array().unwrap();
    let by_id = |id: &str| {
        results.iter().find(|g| g["conv_id"] == id).unwrap_or_else(|| panic!("no {id} in the list"))
    };

    // Three arrays, three different reasons to be empty, and none of them is missing data.
    assert_eq!(by_id("gemini-cli:no-destination")["destinations"], serde_json::json!([]));
    assert_eq!(by_id("chatgpt-export:abandoned")["kind_runs"], serde_json::json!([]));
    for group in results {
        assert_eq!(group["hits"], serde_json::json!([]), "an empty query matches nothing");
        assert_eq!(group["match_seqs"], serde_json::json!([]));
    }
}

#[test]
fn a_run_is_a_band_and_a_length_and_counts_only_what_is_drawn() {
    let f = Fixture::new();
    let ordinary = f
        .search("", &[])["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["conv_id"] == "codex:ordinary")
        .unwrap()
        .clone();

    let drawn: u64 = ordinary["kind_runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| {
            let run = run.as_array().expect("a run is a two-element array, not an object");
            assert_eq!(run.len(), 2, "[band, length]");
            assert!(run[0].is_string(), "the band is a string");
            run[1].as_u64().expect("the length is an integer")
        })
        .sum();

    // The successful tool result is stored, searchable and not drawn, so the strip is one
    // shorter than the conversation. A client drawing `match_seqs` on this strip is reading two
    // coordinate spaces as one (chat-search-me9.25).
    assert_eq!(ordinary["msg_count"], 4);
    assert_eq!(drawn, 3, "a successful tool result occupies no position on the strip");
}

#[test]
fn a_span_is_a_pair_of_utf8_byte_offsets_into_the_snippet_it_marks() {
    // The sibling defect the Swift spike found: read as `Character` offsets these mis-highlight
    // every snippet containing an em-dash, and this corpus is made of em-dashes. Nothing on the
    // wire says which they are (chat-search-me9.33), so the fixture puts a multi-byte character
    // in front of the match and this asserts the difference is real rather than incidental.
    let f = Fixture::new();
    let hits = f.search(TERM, &["--flat"]);
    let marked = hits["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["snippet"].as_str().unwrap().contains('—'))
        .expect("no snippet in the fixture carries a multi-byte character");

    let snippet = marked["snippet"].as_str().unwrap();
    let spans = marked["snippet_spans"].as_array().unwrap();
    assert!(!spans.is_empty(), "a located match marks something: {snippet}");

    for span in spans {
        let (start, end) = (span["start"].as_u64().unwrap(), span["end"].as_u64().unwrap());
        assert_eq!(
            &snippet[start as usize..end as usize].to_ascii_lowercase(),
            TERM,
            "byte-slicing {snippet:?} at [{start}, {end}) did not land on the match"
        );
        assert_ne!(
            start as usize,
            snippet.chars().take_while(|c| *c != 'b').count(),
            "the fixture no longer distinguishes byte offsets from character offsets"
        );
    }
}

#[test]
fn a_destination_is_tagged_by_kind_and_its_payload_follows_from_that() {
    let f = Fixture::new();
    let results = f.search("", &[]);

    for group in results["results"].as_array().unwrap() {
        for destination in group["destinations"].as_array().unwrap() {
            match destination["kind"].as_str().expect("every destination is tagged") {
                "terminal" => {
                    let argv = destination["argv"].as_array().expect("terminal carries an argv");
                    assert!(!argv.is_empty(), "an empty argv names no program");
                    assert!(argv.iter().all(Value::is_string), "argv is already split into words");
                }
                "web" => assert!(destination["url"].is_string(), "web carries a url"),
                other => panic!("undocumented destination kind {other:?}"),
            }
        }
    }
}
