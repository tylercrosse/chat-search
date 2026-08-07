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
//! Driven through the real binary, because the contract is the bytes on stdout. Since
//! `chat-search-me9.36.2` the CLI serializes a `cs_core::Answer` rather than assembling an
//! envelope, so this could in principle serialize one here — but the thing being pinned is what
//! a client receives, and only the binary can be wrong about that in the ways that have
//! actually happened.
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

    fn timeline(&self, query: &str, extra: &[&str]) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_cs"))
            .args(["timeline", query, "--json"])
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
/// A word only the fixture's tool call says, so a tools search and a prose search over this
/// corpus are demonstrably two different sets rather than the same one asked twice.
const TOOL_TERM: &str = "src";

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
        model: None,
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
        declared_model: None,
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
    // The only row here that names a model, and one message of the four names it. The label is
    // the *last* message that named one, so the two after this declaring nothing must not blank
    // it — and the rest of the fixture is what makes the field null in the same reply.
    ordinary.messages[1].model = Some("gpt-5-codex".into());

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

    // Two strands rather than one, which is 45 conversations in 4,426 and the only state in
    // which `thread_count` is worth a client drawing. The second strand is rooted rather than
    // chained: a `thread_key` names a parallel stream (ADR 4), not a child of the message
    // before it, so both leaves are on the head path and both messages are searchable.
    let mut forked = conv(
        "claude-code",
        "two-threads",
        titled("the borrow checker, on two branches"),
        vec![
            msg(0, Role::User, Kind::Prose, Some(t), "does the borrow checker allow this"),
            msg(1, Role::User, Kind::Prose, Some(t + 1), "and on the subagent's own strand"),
        ],
    );
    forked.messages[1].thread_key = "sidechain".into();
    forked.messages[1].parent_native_id = None;
    forked.messages[1].is_sidechain = true;

    vec![ordinary, untitled, undated, abandoned, unreachable, forked]
}

/// Every row object the two replies produce, kept apart by shape. Envelopes are excluded —
/// they have their own pin.
///
/// `Match` and `Hit` are no longer the same object: a nested match states message fields only,
/// because the group above it has already named the conversation, while a flat hit names its
/// own parent because there is no parent row to name it. So each is collected from the one
/// reply that emits it, and the document has a table for each.
fn shapes(f: &Fixture) -> Vec<(&'static str, BTreeMap<String, Nullability>, Vec<Value>)> {
    let mut groups = Vec::new();
    let mut matches = Vec::new();

    for query in ["", TERM] {
        for group in f.search(query, &[]).get("results").unwrap().as_array().unwrap() {
            matches.extend(group.get("matches").unwrap().as_array().unwrap().iter().cloned());
            groups.push(group.clone());
        }
    }
    let hits = f.search(TERM, &["--flat"]).get("hits").unwrap().as_array().unwrap().clone();

    assert!(
        !groups.is_empty() && !matches.is_empty() && !hits.is_empty(),
        "the fixture returned nothing to check"
    );
    vec![
        ("Group", documented("## `Group`"), groups),
        ("Match", documented("## `Match`"), matches),
        ("Hit", documented("## `Hit`"), hits),
    ]
}

// ------------------------------------------------------------------ the pins

#[test]
fn the_binary_emits_exactly_the_keys_the_contract_documents() {
    let f = Fixture::new();

    for (label, table, rows) in shapes(&f) {
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
fn the_two_envelopes_are_two_shapes_rather_than_one_with_a_discriminator() {
    // What `chat-search-me9.32` was filed to decide, decided by deletion. `grouped` was present
    // in one shape and absent in the other, so a decoder modelling both as one envelope had to
    // type it optional and then branch on it to learn whether `results` held conversations or
    // messages. Now `results` is always conversations, `--flat` answers under `hits`, and
    // neither envelope has a key that is sometimes missing.
    let f = Fixture::new();
    let grouped = documented("## The envelope");
    let flat = documented("## The `--flat` envelope");

    assert_eq!(keys(&f.search(TERM, &[])), grouped.keys().cloned().collect::<BTreeSet<_>>());
    assert_eq!(keys(&f.search(TERM, &["--flat"])), flat.keys().cloned().collect::<BTreeSet<_>>());

    // The three-state machinery stays and currently describes no key at all. That is the
    // assertion: a sometimes-absent key is a second type for a decoder, and this says one has
    // not crept back in.
    for (which, table) in [("grouped", &grouped), ("flat", &flat)] {
        assert!(
            keys_where(table, |n| n == Nullability::Absent).is_empty(),
            "the {which} envelope documents a sometimes-absent key again"
        );
        assert!(!table.contains_key("grouped"), "the {which} discriminator is back");
    }
}

#[test]
fn a_one_shot_search_reports_a_settled_total_rather_than_a_floor() {
    // Why `cs search` pays for the second counting pass before printing: a one-shot caller has
    // no later idle moment to spend it in, so the only caller left holding a floor is
    // `--prefix`, which is one process per keystroke and whose total nobody is reading yet.
    let f = Fixture::new();

    let answer = f.search(TERM, &[]);
    assert_eq!(answer["settled"], true);
    assert_eq!(answer["count"], answer["total"], "nothing was truncated at --limit 50");

    // And the no-query browse list counts the corpus it is filtered to rather than the set some
    // ranking pass happened to reach, because it ranked nothing.
    let browse = f.search("", &[]);
    assert_eq!(browse["settled"], true);
    assert_eq!(browse["total"], corpus().len());
    assert!(
        browse["total"].as_u64() > answer["total"].as_u64(),
        "the fixture no longer holds a conversation the term misses"
    );
}

#[test]
fn the_timeline_emits_exactly_the_keys_the_contract_documents() {
    // The third client seam, pinned the same way as the other two. `cs facets` is not, which is
    // the difference worth naming rather than copying: a rail is chips carrying opaque strings,
    // and this reply is *numbers a client does arithmetic on* — a key that moved or a bar that
    // stopped being counted the documented way draws a wrong picture rather than failing.
    let f = Fixture::new();

    let envelope = documented("## The timeline");
    assert_eq!(keys(&f.timeline(TERM, &[])), envelope.keys().cloned().collect::<BTreeSet<_>>());

    let bucket = documented("## `Bucket`");
    let drawn = f.timeline(TERM, &[]);
    let bars = drawn["buckets"].as_array().unwrap();
    assert!(!bars.is_empty(), "the fixture has dated conversations and so has an axis");
    for bar in bars {
        assert_eq!(keys(bar), bucket.keys().cloned().collect::<BTreeSet<_>>());
    }

    for key in keys_where(&envelope, |n| n == Nullability::Never) {
        assert!(!drawn[&key].is_null(), "timeline.{key} is documented never null and was null");
    }
    // Both nullable keys, null here and not null below — the half that stops the document
    // describing a state the code stopped producing.
    assert!(drawn["window"].is_null(), "this query names no date:");
    assert!(drawn["drag"].is_null(), "and nothing was dragged");
}

#[test]
fn a_drawer_asked_the_way_the_list_was_reports_the_list_s_own_total() {
    // `chat-search-me9.8.25`: `cs timeline` took `--prefix` and no other search knob, so a
    // client searching tool traffic drew a prose distribution beneath it — a picture of a
    // different query, under a `total` contradicting the footer above it. Driven through the
    // binary rather than through `cs_core`, because the whole of what was missing was the flag
    // reaching `SearchOptions`, and only the binary can be wrong about that.
    let f = Fixture::new();

    for flags in [&[][..], &["--tools"][..], &["--tools", "--include-off-path"][..]] {
        for query in [TERM, TOOL_TERM] {
            assert_eq!(
                f.timeline(query, flags)["total"],
                f.search(query, flags)["total"],
                "{query:?} {flags:?}: the drawer and the list are describing different sets"
            );
        }
    }

    // And the two readings are two sets rather than one asked twice: each term is in exactly
    // one of the tables. Without this the assertions above would hold against a drawer that
    // ignored the flag entirely.
    assert_eq!(f.search(TOOL_TERM, &["--tools"])["total"], 1, "the one tool call that says it");
    assert_eq!(f.search(TOOL_TERM, &[])["total"], 0, "and no prose does");
    assert_eq!(f.search(TERM, &["--tools"])["total"], 0, "nor the other way about");
    assert!(f.search(TERM, &[])["total"].as_u64().unwrap() > 0);

    // `--include-off-path` is along for parity and this fixture holds no branch edited away, so
    // what it pins here is that the flag is accepted and routed to the same place on both
    // sides. That it selects a *wider* set lives in `cs_core::timeline`'s own tests, where a
    // fixture can carry a superseded sibling without disturbing every count in this file.
}

#[test]
fn a_drag_comes_back_as_query_text_rather_than_as_a_token_to_splice() {
    // The scrubber's whole contract. A client hands over two instants and gets the finished
    // line; nothing on this side assembles a `date:` token, which is what keeps
    // `Window::value_in`'s rounding and midnight rules in one place.
    let f = Fixture::new();
    const DAY: i64 = 86_400_000;
    let (a, b) = (1_700_000_000_000i64, 1_700_000_000_000i64 + 30 * DAY);
    let dragged = f.timeline(TERM, &["--drag", &format!("{a}..{b}")]);

    let drag = &dragged["drag"];
    assert!(!drag.is_null(), "a drag was asked about");
    assert!(drag["value"].as_str().unwrap().contains(".."), "a half-open span, spelled");
    let rewritten = drag["query"].as_str().unwrap();
    assert!(rewritten.contains("date:"), "the filter is in the text: {rewritten:?}");
    assert!(rewritten.contains(TERM), "and the free text is left where it was");

    // And the round trip: put that text back in the box and the window comes back out of it,
    // which is the proof there is no filter state living beside the query.
    let after = f.timeline(rewritten, &[]);
    assert_eq!(after["window"]["value"], drag["value"]);
    assert!(!after["window"]["from"].is_null() && !after["window"]["until"].is_null());
}

#[test]
fn a_key_the_contract_calls_never_null_is_null_nowhere() {
    let f = Fixture::new();

    for (label, table, rows) in shapes(&f) {
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

    for (label, table, rows) in shapes(&f) {
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
        assert_eq!(group["matches"], serde_json::json!([]), "an empty query matches nothing");
        assert_eq!(group["match_seqs"], serde_json::json!([]));
    }
}

#[test]
fn a_row_says_which_model_it_ended_on_and_how_many_strands_it_holds() {
    // Both were columns of the index that no client could see (`chat-search-me9.8.14`), and
    // both are summaries rather than raw values — which is the part a shape pin cannot check.
    // `model` is the last message that named one, not the last message's own field, and
    // `thread_count` is a count of strands, not of messages.
    let f = Fixture::new();
    let results = f.search("", &[]);
    let by_id = |id: &str| {
        results["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["conv_id"] == id)
            .unwrap_or_else(|| panic!("no {id} in the list"))
            .clone()
    };

    let ordinary = by_id("codex:ordinary");
    assert_eq!(ordinary["model"], "gpt-5-codex", "the rollup keeps the only message that said");
    assert_eq!(ordinary["thread_count"], 1, "four messages, one strand");
    assert_eq!(by_id("claude-code:two-threads")["thread_count"], 2, "two messages, two strands");
    // Neither one strand nor several: the conversation that holds no messages at all. A client
    // drawing a fork mark above 1 shows nothing here, which is right for both reasons.
    assert_eq!(by_id("chatgpt-export:abandoned")["thread_count"], 0);
    assert!(
        by_id("gemini-cli:no-destination")["model"].is_null(),
        "nothing in it named a model, which is 1,300 of 4,426 rows"
    );
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
    // every snippet containing an em-dash, and this corpus is made of em-dashes. The envelope
    // now names the encoding rather than leaving a client to infer it (chat-search-me9.33), so
    // the fixture puts a multi-byte character in front of the match and this asserts both that
    // the difference between the two readings is real and that the wire names the right one —
    // a value that travelled beside offsets it does not describe would be worse than silence.
    let f = Fixture::new();
    let hits = f.search(TERM, &["--flat"]);
    assert_eq!(hits["mark_offsets"], "utf8-bytes");
    let marked = hits["hits"]
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
