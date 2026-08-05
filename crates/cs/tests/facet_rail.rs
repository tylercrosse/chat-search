//! What `cs facets --json` hands a client, through the real binary (chat-search-1ld).
//!
//! The projections are unit-tested in `cs_core::facets` over inputs a test can state. What cannot
//! be tested there is the wiring: three censuses, three projections, and one clock between them,
//! assembled here because this is the only crate that can see a config, a disk and an index at
//! once. A rail whose `dirs` were joined against the `dates` census would serialize perfectly.
//!
//! The round trip is the assertion that matters. A chip carries *query text a click will paste
//! into the box*, so the only way to check it is to paste it back: ask again with what the chip
//! gave, and see the chip in the state it promised.

use std::path::PathBuf;
use std::process::Command;

use cs_core::model::{Conversation, Kind, Message, Role, Titles};
use serde_json::Value;

const DAY: i64 = 86_400_000;

/// Noon today, locally. A conversation timestamped `now - 1h` is in yesterday's span if the test
/// runs at 00:30, and a test that fails once a day is one nobody believes the rest of the time.
fn today() -> i64 {
    cs_core::local_day_start(cs_core::now_ms()).unwrap() + 12 * 3_600_000
}

struct Fixture {
    home: PathBuf,
}

impl Fixture {
    /// `indexed` false leaves the index absent, which is the state a first run is in.
    fn new(indexed: bool) -> Self {
        let home = std::env::temp_dir().join(format!("cs-facets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join("sessions")).unwrap();
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
                 include = [\"**/*.jsonl\"]\n",
                f.home.join("archive").display(),
                f.home.join("sessions").display(),
            ),
        )
        .unwrap();

        if indexed {
            let mut conn = cs_core::open(f.db().to_str().unwrap()).unwrap();
            cs_core::write_conversations(&mut conn, corpus().iter()).unwrap();
        }
        f
    }

    fn config(&self) -> PathBuf {
        self.home.join("config.toml")
    }

    fn db(&self) -> PathBuf {
        self.home.join("archive").join("index.db")
    }

    fn rail(&self, query: &str) -> Value {
        let out = Command::new(env!("CARGO_BIN_EXE_cs"))
            .args(["--config", self.config().to_str().unwrap(), "facets", query, "--json"])
            .env("HOME", &self.home)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

/// The chip for one value of one facet, or `None` if the rail does not offer it.
fn chip<'a>(rail: &'a Value, facet: &str, value: &str) -> Option<&'a Value> {
    rail[facet]["values"].as_array().unwrap().iter().find(|c| c["value"] == value)
}

#[test]
fn every_facet_in_the_grammar_arrives_as_a_section_of_its_own() {
    let f = Fixture::new(true);
    let rail = f.rail("");
    for (facet, keyword) in [("sources", "agent:"), ("dirs", "dir:"), ("dates", "date:")] {
        assert_eq!(rail[facet]["keyword"], keyword, "{facet} names the grammar it writes");
        assert_eq!(rail[facet]["all"]["selected"], true, "an empty query filters on nothing");
    }
    assert_eq!(rail["v"], 1);
}

#[test]
fn a_chip_hands_back_a_query_that_lights_it_and_then_puts_it_out() {
    // The round trip, through two runs of the binary, on the two rails this bead added. A
    // rewriting rule that regressed in `cs_core::query` would serialize perfectly and fail here.
    let f = Fixture::new(true);
    let start = f.rail("borrow checker");
    for (facet, value) in [("dirs", "/home/t/dev/web-app"), ("dates", "today")] {
        let click = chip(&start, facet, value).unwrap()["query"].as_str().unwrap().to_string();
        let after = f.rail(&click);
        let lit = chip(&after, facet, value).unwrap();
        assert_eq!(lit["state"], "include", "{facet} {value} did not light: {click:?}");
        assert_eq!(after[facet]["all"]["selected"], false, "{facet} still reads as All");
        assert_eq!(
            lit["query"].as_str().unwrap().trim(),
            "borrow checker",
            "{facet} {value} does not toggle back off"
        );
    }
}

#[test]
fn one_clock_answers_the_whole_reply() {
    // The spans nest, so a rail assembled from two clocks could report more conversations this
    // week than this month. They are counted in one statement for that reason; this says the
    // command does not undo it by counting twice.
    let f = Fixture::new(true);
    let dates = f.rail("");
    let counts: Vec<i64> = dates["dates"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["conversations"].as_i64().unwrap())
        .collect();
    assert_eq!(counts, [1, 3, 3, 1], "today ⊂ week ⊂ month, and older is the rest");
}

#[test]
fn a_directory_no_dir_token_can_carry_is_counted_and_not_offered() {
    // `chat-search-me9.8.16`: whitespace separates words, so `dir:/home/t/My Documents` is a
    // filter *and* a search term. It is still a directory the index holds, and the count says so
    // — what the rail may not do is hand over a click it cannot prove.
    let f = Fixture::new(true);
    let rail = f.rail("");
    assert!(chip(&rail, "dirs", "/home/t/My Documents").is_none(), "offered a click that lies");
    assert_eq!(rail["dirs"]["indexed"], 2, "and yet the index holds two directories");
    assert_eq!(rail["dirs"]["undirected"], 1, "and one conversation records none at all");
    assert_eq!(chip(&rail, "dirs", "/home/t/dev/web-app").unwrap()["conversations"], 2);
}

#[test]
fn the_rail_answers_on_a_machine_with_no_index_at_all() {
    // The state a client most needs to draw and the one an error would hide. `cs search` refuses
    // here; this must not, because the true rail on a first run is every configured source at
    // zero and four spans that are arithmetic rather than data.
    let f = Fixture::new(false);
    let rail = f.rail("");
    assert_eq!(rail["index_state"], "no_index");
    assert_eq!(rail["dates"]["values"].as_array().unwrap().len(), 4);
    assert!(rail["dates"]["values"].as_array().unwrap().iter().all(|c| c["conversations"] == 0));
    assert_eq!(rail["dirs"]["values"].as_array().unwrap().len(), 0);
    assert_eq!(rail["dirs"]["indexed"], 0);
    assert_eq!(chip(&rail, "sources", "codex").unwrap()["coverage"], "live", "config alone");
}

fn corpus() -> Vec<Conversation> {
    vec![
        conv("a", Some("/home/t/dev/web-app"), today()),
        conv("b", Some("/home/t/dev/web-app"), today() - 200 * DAY),
        // A path no `dir:` token can carry, which is a fact about the grammar rather than about
        // this directory: it is indexed, searchable, and counted.
        conv("c", Some("/home/t/My Documents"), today() - 3 * DAY),
        conv("d", None, today() - 3 * DAY),
    ]
}

fn conv(native_id: &str, cwd: Option<&str>, ts: i64) -> Conversation {
    Conversation {
        source: "codex".into(),
        native_id: native_id.into(),
        titles: Titles { first_user: Some("borrow checker".into()), ..Default::default() },
        cwd: cwd.map(str::to_string),
        git_branch: None,
        declared_model: None,
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
            model: None,
            ts: Some(ts),
            text: "borrow checker".into(),
        }],
    }
}
