//! What the corpus is made of, counted: which sources exist and how much of each one the index
//! holds (chat-search-a7k.29), which directories it holds, and how far back it goes
//! (chat-search-1ld).
//!
//! All three are *censuses*, and that word is doing work: they count the whole index rather than
//! the answer to a query. A rail built from what matched can only list what is already in front
//! of the reader — which is how a source with nothing indexed becomes invisible, and how a
//! project you have not touched this month stops existing.
//!
//! The index can only answer *which sources produced rows*, and that is not the question a
//! client asks. A configured source holding nothing and a source nobody ever configured are
//! the same absence in `conversation`, and they are opposite problems: the first is an
//! importer that threw or an archive run that never happened, the second is a tool the user
//! does not use. Drawn from the index alone, the broken one is invisible — you search, get
//! nothing, and conclude you used a different agent (chat-search-me9.14).
//!
//! `cs_archive::drift` already knows the missing half. It reports it as a printed line during
//! `cs archive`, which no other crate can read, so both clients that need it — the TUI's
//! facet bar and `cs status` — would each have to join it against the index themselves. That
//! is the shape of the local-date bug: one rule, derived in three places, wrong in two.
//!
//! So the join lives here, once, and returns a value.
//!
//! **This crate still does not read config.** The caller supplies [`Watched`] — what the
//! config names and what is on this disk — and gets the index counts folded in. Pointing
//! `cs-core` at a config would run the dependency backwards and drag `toml` and the globs
//! into the search crate (`config.rs:99`, docs/TUI-DESIGN.md §1), and `cs-tui` could not use
//! the result at all: it depends on `cs-core` and nothing else, so it has no way to reach a
//! type that lives in `cs-archive`. Facts in, one answer out, is the only arrangement that
//! serves both clients without either of them learning where the config lives.

use rusqlite::Connection;
use std::collections::BTreeMap;

/// What the config and this disk say about one source, before the index is consulted.
///
/// Assembled by whoever can see both — `cs` builds it from `Config::sources` and
/// `cs_archive::drift::detect`. A detected-but-unconfigured location is one of these with
/// `configured: false`, which is how a candidate accruing uncaptured conversations reaches a
/// client at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    /// Permanent, and part of every conversation id it produced (ADR 16). The join key.
    pub id: String,
    /// A `[[sources]]` entry names it, so `cs archive` captures it.
    pub configured: bool,
    /// Its directory is on this disk right now.
    pub present: bool,
}

/// Where a source stands with the config and this disk: the four cells of
/// configured × present, named.
///
/// Named rather than left as two booleans for the same reason [`crate::IndexState`] is —
/// clients branch on the name, and a state derived at each call site is a state derived
/// differently at each call site. The count is deliberately not part of it: "configured and
/// holding nothing" is [`Coverage::Live`] with zero conversations, and collapsing those two
/// facts into one would make an empty source indistinguishable from an unconfigured one all
/// over again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Configured and its directory is here. Capture is set up; whether it has *worked* is
    /// the conversation count beside this, not this.
    Live,
    /// Configured, but the directory is gone — uninstalled, moved, or an external volume
    /// that is not mounted. What was already archived is safe; nothing new is arriving.
    Missing,
    /// Its directory is here and no `[[sources]]` entry claims it. Conversations are accruing
    /// uncaptured right now, which is what chat-search-a7k.12 exists to say out loud.
    Unconfigured,
    /// Neither configured nor on this disk, yet the index holds rows for it. A source that
    /// was captured once and has since fallen out of the config — the bead's own example is
    /// `chatgpt-export` sitting complete in the archive with 2,011 conversations while the
    /// config had lost the entry. Searchable, and never growing again.
    Retired,
}

impl Coverage {
    /// The four cells, in the only order they can be read: config first, then disk.
    pub fn of(configured: bool, present: bool) -> Self {
        match (configured, present) {
            (true, true) => Coverage::Live,
            (true, false) => Coverage::Missing,
            (false, true) => Coverage::Unconfigured,
            (false, false) => Coverage::Retired,
        }
    }

    /// The name this state travels under. Stable; the prose above it is not.
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::Live => "live",
            Coverage::Missing => "missing",
            Coverage::Unconfigured => "unconfigured",
            Coverage::Retired => "retired",
        }
    }

    /// Whether a `[[sources]]` entry names it.
    pub fn configured(self) -> bool {
        matches!(self, Coverage::Live | Coverage::Missing)
    }

    /// Whether its directory is on this disk.
    pub fn present(self) -> bool {
        matches!(self, Coverage::Live | Coverage::Unconfigured)
    }
}

/// One source, and everything a client needs to draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCoverage {
    pub id: String,
    pub coverage: Coverage,
    /// Conversations the index holds for it. Zero is an answer rather than an absence — read
    /// beside [`SourceCoverage::coverage`] it separates a broken importer from a tool that
    /// was never watched.
    pub conversations: i64,
}

/// The full inventory: everything watched, unioned with everything the index holds.
///
/// A read failure is returned rather than swallowed. Callers differ on what to do with it —
/// the facet bar drops the census and keeps searching, `cs status` says so — and a function
/// that decided for them would be making that call in the wrong place.
pub fn of(conn: &Connection, watched: &[Watched]) -> rusqlite::Result<Vec<SourceCoverage>> {
    Ok(join(watched, &counts(conn)?))
}

/// Conversations per source, as the index has them. Sources with no rows are simply absent —
/// which is the entire reason [`join`] exists.
fn counts(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT source, COUNT(*) FROM conversation GROUP BY source")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// The join itself, over supplied inputs.
///
/// Split from [`of`] for the same reason `drift::diff` is split from `drift::detect`: the
/// interesting cases are a source in one input and not the other, and building an index to
/// reach them would test SQLite rather than the rule.
///
/// Ordered by id, so the facet bar never reshuffles between runs and two clients listing the
/// same corpus list it the same way.
pub fn join(watched: &[Watched], indexed: &[(String, i64)]) -> Vec<SourceCoverage> {
    let mut out: BTreeMap<&str, SourceCoverage> = BTreeMap::new();
    for w in watched {
        // First entry wins. `Config::validate` rejects a duplicate id before it can reach
        // here, and a second one would be a second permanent namespace for one directory
        // (ADR 16) rather than something to merge.
        out.entry(&w.id).or_insert_with(|| SourceCoverage {
            id: w.id.clone(),
            coverage: Coverage::of(w.configured, w.present),
            conversations: 0,
        });
    }
    for (id, n) in indexed {
        out.entry(id)
            .or_insert_with(|| SourceCoverage {
                id: id.clone(),
                coverage: Coverage::Retired,
                conversations: 0,
            })
            .conversations = *n;
    }
    out.into_values().collect()
}

// ---- the other two facets' censuses (chat-search-1ld) ---------------------------------------
//
// Neither has a config half, so neither needs the caller to supply anything: a directory is only
// ever a fact of the index, and a span is arithmetic. They are here rather than beside the
// projection for the same reason `of` is — `cs_core::facets` reads no connection, so that the
// join can be tested over inputs a test can state rather than an index it has to build.

/// One directory the index holds conversations for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirCoverage {
    /// The `cwd` as recorded, which is what `dir:` selects on and what a client shortens for
    /// display. Never a derived project name: `chat-search-6eb.26` measured basename collapsing
    /// 100 directories to 90, seven of them called `i`, and the nearest-`.git`-ancestor
    /// alternative reads the live filesystem and so breaks ADR 1.
    pub path: String,
    pub conversations: i64,
}

/// The directories a rail can offer, and the shape of what it leaves out.
///
/// The default is what an index nobody can read holds: nothing, and nothing left out. Unlike a
/// source census there is no config half to fall back on, so this is the whole of the honest
/// answer in that case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirCensus {
    /// The busiest directories, most conversations first and ties broken by path so that the
    /// rail does not reshuffle between two runs over one index.
    pub busiest: Vec<DirCoverage>,
    /// Distinct directories in the index, `busiest` included. What says whether the rail is the
    /// whole list or the head of one.
    pub indexed: i64,
    /// Conversations recording no directory at all. Two thirds of this corpus, because only the
    /// agent sources have a working directory to record — so a `dir:` rail that did not carry
    /// this number would read as a filter over everything (`chat-search-6eb.26`).
    pub undirected: i64,
}

/// The directory census: the busiest `limit` of them, and the totals that place them.
///
/// `limit` is the caller's because the rail is a rail and not a list: the tail of this
/// distribution is per-conversation scratch directories, which are worth counting and not worth
/// drawing. Zero means every directory.
pub fn dirs(conn: &Connection, limit: usize) -> rusqlite::Result<DirCensus> {
    let sql = format!(
        "SELECT cwd, COUNT(*) FROM conversation
          WHERE cwd IS NOT NULL AND cwd <> ''
          GROUP BY cwd
          ORDER BY COUNT(*) DESC, cwd{}",
        if limit == 0 { String::new() } else { format!("\n          LIMIT {limit}") }
    );
    let mut stmt = conn.prepare(&sql)?;
    let busiest = stmt
        .query_map([], |r| Ok(DirCoverage { path: r.get(0)?, conversations: r.get(1)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // `''` counted with NULL rather than as a directory: an importer that recorded an empty
    // string knows no more about where the conversation ran than one that recorded nothing.
    let (indexed, undirected) = conn.query_row(
        "SELECT COUNT(DISTINCT nullif(cwd, '')),
                COUNT(*) FILTER (WHERE cwd IS NULL OR cwd = '')
           FROM conversation",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;

    Ok(DirCensus { busiest, indexed, undirected })
}

/// How many conversations fall in one of the spans a rail offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateCoverage {
    /// The `date:` value this counts, as a client would click it — one of
    /// [`crate::query::DATE_SPANS`].
    pub value: &'static str,
    pub conversations: i64,
}

/// The recency census: every span in [`crate::query::DATE_SPANS`], counted against one clock.
///
/// The spans nest rather than partition, so these do not sum to the corpus and are not meant to:
/// each is the answer to "how many are there if I go back this far", which is the question the
/// rail asks. Counted in one statement against one `now_ms`, so that no two rows can be answered
/// either side of a midnight.
///
/// The predicate is `search`'s — `ended_at IS NOT NULL` and a half-open `[from, until)` — because
/// a count that did not match the filter it labels is a chip promising rows the click cannot
/// produce. An undated conversation is in no span, and so is in nothing this counts.
pub fn dates(conn: &Connection, now_ms: i64) -> rusqlite::Result<Vec<DateCoverage>> {
    let mut columns = Vec::new();
    let mut binds: Vec<i64> = Vec::new();
    for (value, _) in crate::query::DATE_SPANS {
        // A span that cannot resolve counts nothing rather than counting everything. The values
        // are constants, so this is unreachable short of a clock outside chrono's range — and
        // `0` beside a live chip is the reading that does not overstate.
        let Some(window) =
            crate::query::DateSpec::parse(value).and_then(|spec| spec.window(now_ms))
        else {
            columns.push("0".to_string());
            continue;
        };
        let mut bounds = vec!["ended_at IS NOT NULL".to_string()];
        for (bound, edge) in [(window.from, ">="), (window.until, "<")] {
            if let Some(at) = bound {
                binds.push(at);
                bounds.push(format!("ended_at {edge} ?{}", binds.len()));
            }
        }
        columns.push(format!("COUNT(*) FILTER (WHERE {})", bounds.join(" AND ")));
    }

    let sql = format!("SELECT {} FROM conversation", columns.join(", "));
    conn.query_row(&sql, rusqlite::params_from_iter(binds.iter()), |r| {
        crate::query::DATE_SPANS
            .iter()
            .enumerate()
            .map(|(i, (value, _))| Ok(DateCoverage { value, conversations: r.get(i)? }))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Conversation, Kind, Message, Role, Titles};

    fn watched(id: &str, configured: bool, present: bool) -> Watched {
        Watched { id: id.into(), configured, present }
    }

    #[test]
    fn a_configured_source_with_no_rows_is_not_the_same_as_one_nobody_configured() {
        // The whole bead in one assertion: both hold zero conversations, and a client that
        // could only see the index would draw them identically — or, worse, draw neither.
        let inventory = join(
            &[watched("claude-code", true, true), watched("copilot-cli", false, true)],
            &[],
        );
        assert_eq!(inventory[0].coverage, Coverage::Live);
        assert_eq!(inventory[1].coverage, Coverage::Unconfigured);
        assert!(inventory.iter().all(|s| s.conversations == 0));
    }

    #[test]
    fn a_source_the_index_holds_but_nothing_watches_is_still_in_the_inventory() {
        // `chatgpt-export`: 2,011 conversations archived, then the config lost the entry.
        // Dropping it would hide a searchable third of the corpus from the facet bar.
        let inventory = join(&[watched("codex", true, true)], &[("chatgpt-export".into(), 2011)]);
        assert_eq!(inventory.len(), 2);
        assert_eq!(inventory[0].id, "chatgpt-export");
        assert_eq!(inventory[0].coverage, Coverage::Retired);
        assert_eq!(inventory[0].conversations, 2011);
    }

    #[test]
    fn a_configured_source_whose_directory_vanished_keeps_its_rows() {
        let inventory = join(&[watched("gemini-cli", true, false)], &[("gemini-cli".into(), 112)]);
        assert_eq!(inventory[0].coverage, Coverage::Missing);
        assert_eq!(inventory[0].conversations, 112);
    }

    #[test]
    fn the_inventory_is_ordered_by_id_whichever_side_each_source_came_from() {
        let inventory = join(
            &[watched("codex", true, true), watched("claude-code", true, true)],
            &[("gemini-cli".into(), 1), ("codex".into(), 2)],
        );
        let ids: Vec<_> = inventory.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["claude-code", "codex", "gemini-cli"]);
    }

    #[test]
    fn every_state_can_say_what_it_is_made_of() {
        for (configured, present) in [(true, true), (true, false), (false, true), (false, false)] {
            let c = Coverage::of(configured, present);
            assert_eq!((c.configured(), c.present()), (configured, present), "{}", c.as_str());
        }
    }

    #[test]
    fn the_counts_come_from_the_index_and_the_states_from_the_caller() {
        let mut conn = crate::index::open(":memory:").unwrap();
        crate::index::write_conversations(&mut conn, &[conv("codex", "a"), conv("codex", "b")])
            .unwrap();

        let inventory = of(
            &conn,
            &[watched("codex", true, true), watched("claude-code", true, true)],
        )
        .unwrap();
        assert_eq!(inventory.len(), 2);
        assert_eq!((inventory[1].id.as_str(), inventory[1].conversations), ("codex", 2));
        // The one the index cannot see at all, which is the point.
        assert_eq!((inventory[0].id.as_str(), inventory[0].conversations), ("claude-code", 0));
        assert_eq!(inventory[0].coverage, Coverage::Live);
    }

    // ---- the other two censuses (chat-search-1ld) ----

    /// Noon on the day `1_785_000_000_000` falls on, locally. A fixed instant would put "today"
    /// a few minutes wide on a machine running these at 00:01, and a test that fails once a day
    /// is a test nobody believes the rest of the time.
    fn noon() -> i64 {
        crate::time::local_day_start(1_785_000_000_000).unwrap() + 12 * 3_600_000
    }

    fn indexed(convs: &[Conversation]) -> rusqlite::Connection {
        let mut conn = crate::index::open(":memory:").unwrap();
        crate::index::write_conversations(&mut conn, convs).unwrap();
        conn
    }

    #[test]
    fn the_directory_census_leads_with_the_busiest_and_says_what_it_left_out() {
        // A rail is a rail: `chat-search-6eb.26` found a large share of this corpus's directories
        // are per-conversation scratch dirs, so the tail is worth counting and not worth drawing.
        let conn = indexed(&[
            in_dir("a", Some("/home/t/web-app")),
            in_dir("b", Some("/home/t/web-app")),
            in_dir("c", Some("/home/t/api")),
            in_dir("d", Some("/home/t/scratch")),
        ]);
        let census = dirs(&conn, 2).unwrap();
        assert_eq!(
            census.busiest,
            [
                DirCoverage { path: "/home/t/web-app".into(), conversations: 2 },
                DirCoverage { path: "/home/t/api".into(), conversations: 1 },
            ],
            "ties break by path, so two runs over one index draw the same rail"
        );
        assert_eq!(census.indexed, 3, "the one it did not draw is still counted");
        assert_eq!(dirs(&conn, 0).unwrap().busiest.len(), 3, "no limit is every directory");
    }

    #[test]
    fn a_conversation_that_records_no_directory_is_counted_rather_than_dropped() {
        // Two thirds of the real corpus. A rail that reported only the directories would read as
        // a filter over everything, when `dir:` cannot reach a ChatGPT conversation at all.
        let conn = indexed(&[
            in_dir("a", Some("/home/t/web-app")),
            in_dir("b", None),
            // An importer that recorded an empty string knows no more than one that recorded
            // nothing, so it is not a third state and certainly not a directory.
            in_dir("c", Some("")),
        ]);
        let census = dirs(&conn, 0).unwrap();
        assert_eq!(census.indexed, 1);
        assert_eq!(census.undirected, 2);
    }

    #[test]
    fn the_spans_nest_rather_than_partition_the_corpus() {
        // Which is why they do not sum to it: each answers "how many are there if I go back this
        // far", and today is inside this week is inside this month.
        let now = noon();
        let conn = indexed(&[
            ended("a", now - 3_600_000),
            ended("b", now - 3 * 86_400_000),
            ended("c", now - 20 * 86_400_000),
            ended("d", now - 200 * 86_400_000),
        ]);
        let counts: Vec<i64> = dates(&conn, now).unwrap().iter().map(|d| d.conversations).collect();
        assert_eq!(counts, [1, 2, 3, 1], "today ⊂ week ⊂ month, and older is the rest");
        assert_eq!(
            dates(&conn, now).unwrap().iter().map(|d| d.value).collect::<Vec<_>>(),
            ["today", "week", "month", ">1mo"],
            "in the order a rail draws them"
        );
    }

    #[test]
    fn a_conversation_with_no_end_is_in_no_span_at_all() {
        // `search`'s own reading: the filter is `ended_at IS NOT NULL AND …`, so a count that
        // swept an undated conversation into "older" would label a chip with rows its click
        // cannot return.
        let now = noon();
        let conn = indexed(&[undated("a"), ended("b", now - 200 * 86_400_000)]);
        let counts: Vec<i64> = dates(&conn, now).unwrap().iter().map(|d| d.conversations).collect();
        assert_eq!(counts, [0, 0, 0, 1]);
    }

    fn in_dir(native_id: &str, cwd: Option<&str>) -> Conversation {
        Conversation { cwd: cwd.map(str::to_string), ..conv("codex", native_id) }
    }

    fn ended(native_id: &str, ts: i64) -> Conversation {
        let mut c = conv("codex", native_id);
        c.messages[0].ts = Some(ts);
        c
    }

    fn undated(native_id: &str) -> Conversation {
        let mut c = conv("codex", native_id);
        c.messages[0].ts = None;
        c
    }

    fn conv(source: &str, native_id: &str) -> Conversation {
        Conversation {
            source: source.into(),
            native_id: native_id.into(),
            titles: Titles::default(),
            cwd: None,
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
                ts: Some(1_700_000_000_000),
                text: "hello".into(),
            }],
        }
    }
}
