//! What `cs search --offset` promises, through the real binary (chat-search-me9.8.33).
//!
//! One claim carries the whole feature: **a page is a window on the ranking, not a new answer.**
//! Ask for the first ten and then for the ten after them, and you hold the same twenty rows in
//! the same order as a single ask for twenty. A client appends rather than re-sorts, so if that
//! is ever untrue the list on screen stops being a ranking and nothing on it says so.
//!
//! Driven through the binary rather than through `cs_core::answer` because the offset crosses a
//! process boundary in the only surface that uses it: the macOS app spawns one `cs` per page, and
//! what it can rely on is the bytes on stdout.
//!
//! The corpus here is uniform on purpose — one term, one hit each, distinct timestamps — so a
//! disagreement between two pages is a paging bug rather than a ranking one. What a fixture this
//! size cannot reach is the ranking's scan ceiling, which is where paging was genuinely hard: see
//! `SearchOptions::offset` for why the ceiling ignores the offset, and `docs/JSON-CONTRACT.md` for
//! the numbers off the real archive that decided it.

use std::path::PathBuf;
use std::process::Command;

use cs_core::model::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;

/// Enough conversations to page three times over at `--limit 10` and still have a tail.
const CORPUS: usize = 35;
const TERM: &str = "borrow";
const DAY: i64 = 86_400_000;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!("cs-paging-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = Fixture { dir };
        std::fs::write(
            f.config(),
            format!(
                "archive_root = \"{}\"\n\
                 machine_alias = \"test-box\"\n\
                 log_queries = false\n",
                f.dir.display()
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

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_cs"))
            .args(["search"])
            .args(args)
            .args(["--db", self.db().to_str().unwrap()])
            .args(["--config", self.config().to_str().unwrap()])
            .output()
            .unwrap()
    }

    fn page(&self, query: &str, limit: usize, offset: usize) -> Value {
        let out = self.run(&[
            query,
            "--json",
            "--limit",
            &limit.to_string(),
            "--offset",
            &offset.to_string(),
        ]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// The conversation ids of a page, in the order it returned them.
fn ids(page: &Value) -> Vec<String> {
    page["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["conv_id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn two_pages_are_two_windows_on_one_ranking() {
    // The whole feature in one assertion. `Grouping.swift` in the macOS app gathers on the
    // premise that "nothing is re-ranked — the rows arrive best first and that order IS the
    // answer", which is only safe while this holds: a page that re-ranked would let a group grow
    // upward and move rows above the reader's scroll position.
    let f = Fixture::new();
    let whole = ids(&f.page(TERM, 20, 0));
    let first = ids(&f.page(TERM, 10, 0));
    let second = ids(&f.page(TERM, 10, 10));

    assert_eq!(first.len(), 10);
    assert_eq!(second.len(), 10);
    assert_eq!([first, second].concat(), whole, "two pages are not the one ranking");
    // Said the other way round, which is the property a client actually depends on: no row
    // arrives twice and none is stepped over.
    let paged: Vec<String> = (0..4).flat_map(|p| ids(&f.page(TERM, 10, p * 10))).collect();
    assert_eq!(paged.len(), CORPUS, "paging to the end returned a different number of rows");
    let unique: std::collections::BTreeSet<&String> = paged.iter().collect();
    assert_eq!(unique.len(), CORPUS, "a conversation arrived on two pages");
}

#[test]
fn a_page_says_how_deep_it_is_and_still_counts_the_whole_match() {
    // `count` is the page, `total` is the answer, and `offset` is which page — three numbers a
    // client draws as one sentence. `total` in particular must not shrink as a reader pages: the
    // status strip prints "rows in hand, of every conversation the query selects", and a total
    // that counted only what was left below the page would make that sentence walk downwards.
    let f = Fixture::new();
    let first = f.page(TERM, 10, 0);
    let deep = f.page(TERM, 10, 20);

    assert_eq!(first["offset"], 0, "an unpaged reply says so rather than omitting the key");
    assert_eq!(deep["offset"], 20);
    assert_eq!(first["count"], 10);
    assert_eq!(deep["count"], 10);
    assert_eq!(first["total"], CORPUS, "every conversation holds the term");
    assert_eq!(deep["total"], first["total"], "the total is of the match, not of what is left");
    assert_eq!(deep["settled"], true);
}

#[test]
fn paging_past_the_end_is_an_empty_page_rather_than_a_refusal() {
    // What the bottom of the list feels like from the client's side: the request succeeds, the
    // page is empty, and `total` still says how many there were. A refusal here would make the
    // ordinary end of a list indistinguishable from an index that had gone away, and the app
    // draws those two states very differently.
    let f = Fixture::new();
    let past = f.page(TERM, 10, CORPUS + 5);

    assert_eq!(past["count"], 0);
    assert_eq!(past["results"].as_array().unwrap().len(), 0);
    assert_eq!(past["total"], CORPUS);
    assert_eq!(past["offset"], CORPUS + 5);
}

#[test]
fn the_browse_list_pages_on_the_same_flag_as_the_ranking() {
    // An empty query is answered out of the recent list rather than the ranker, and that is a
    // routing decision `cs_core::answer` makes for itself — a client cannot see which branch it
    // got. So the flag has to work on both, or paging would silently stop at sixty rows for the
    // one query anybody types first, which is none.
    let f = Fixture::new();
    let whole = ids(&f.page("", 20, 0));
    let first = ids(&f.page("", 10, 0));
    let second = ids(&f.page("", 10, 10));

    assert_eq!([first, second].concat(), whole, "the browse list pages differently");
    // And its total is still the corpus rather than the page, which is the one arithmetic a
    // short page at depth can get wrong: the recent branch reads a short list as its own total.
    assert_eq!(f.page("", 10, 30)["total"], CORPUS);
    assert_eq!(f.page("", 10, 30)["count"], 5, "the tail is what is left, not a full page");
}

#[test]
fn flat_refuses_to_page_rather_than_paging_over_an_order_with_ties_in_it() {
    // `--flat` orders messages by score with no tiebreak, so two messages that score identically
    // may swap between calls — under an offset that is one message on both pages and another on
    // neither. Refused rather than ignored: a flag that silently does nothing is how a client
    // ends up drawing page one four times and calling it four pages.
    let f = Fixture::new();
    let out = f.run(&[TERM, "--json", "--flat", "--limit", "10", "--offset", "10"]);

    assert!(!out.status.success(), "--flat --offset was answered rather than refused");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--offset"), "the refusal does not name the flag: {err}");
}

fn corpus() -> Vec<Conversation> {
    // Distinct timestamps, one term each: the recency divisor then orders them strictly, so the
    // ranking has no ties and the browse list has none either. A fixture with ties would be
    // testing the tiebreak rather than the paging.
    (0..CORPUS)
        .map(|i| Conversation {
            source: "codex".into(),
            native_id: format!("c{i:02}"),
            titles: Titles { first_user: Some(format!("row {i}")), ..Default::default() },
            cwd: None,
            git_branch: None,
            declared_model: None,
            surface: None,
            forked_from_native_id: None,
            head_native_id: None,
            messages: vec![Message {
                native_id: "m0".into(),
                parent_native_id: None,
                thread_key: "t".into(),
                is_sidechain: false,
                is_error: false,
                seq: 0,
                role: Role::User,
                kind: Kind::Prose,
                model: None,
                ts: Some(cs_core::now_ms() - (i as i64 + 1) * DAY),
                text: format!("{TERM} checker, row {i}"),
            }],
        })
        .collect()
}
