//! What was searched for, and what was opened.
//!
//! The eval set in `eval` measures the ranking against queries written down in advance, and
//! the trouble with a written-down query is that nobody had the need it describes. Asked to
//! invent a query and then say which result is "the one I meant", the honest answer is
//! usually that several are fine and none is the one, because there was no one.
//!
//! A real search does not have that problem. The query is whatever was actually typed, and
//! the conversation opened afterwards is the answer, with nobody grading anything. This is
//! implicit relevance feedback, and it is the cheapest ground truth available here.
//!
//! It has one well-known bias worth naming: a conversation can only be opened if it was
//! shown, so a good result the ranking buried never appears as an answer. That is exactly
//! the hole [`crate::eval`]'s pooled judging fills, so the two are complementary rather than
//! alternatives — harvested queries say what to ask, pooled judging says what else was there.
//!
//! Never rebuildable, unlike everything else next to it. `index.db` is a pure function of the
//! archive and can be deleted at will; this cannot be recovered from anything and wants
//! backing up. It is the first authored data this project keeps, and a precursor to the
//! `library.db` split.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::Path;

/// Conversation ids kept per event.
///
/// Enough to reconstruct what the ranking offered at the time, which is what makes a later
/// tuning run able to ask "would this change have moved the one you picked". Capped because
/// a broad prefix query can match thousands and the tail is not evidence of anything.
pub const MAX_SHOWN: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A search that returned. No selection followed it, at least not one that was recorded.
    Search {
        ts: i64,
        q: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        /// Conversations returned, best first, truncated to [`MAX_SHOWN`].
        shown: Vec<String>,
        /// Total returned before truncation.
        n: usize,
        ms: f64,
    },
    /// A conversation was opened off the back of a search. The interesting one.
    Pick {
        ts: i64,
        q: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        conv_id: String,
        /// 1-based rank of `conv_id` in `shown`. `None` means it was not in the list at all,
        /// which is worth keeping: it says the conversation was reached some other way.
        rank: Option<usize>,
        shown: Vec<String>,
        n: usize,
    },
    /// A span of the log that was machine-driven — a benchmark, a smoke test — rather than
    /// typed by somebody who wanted an answer.
    ///
    /// Nothing inside a search event separates a query typed to measure latency from one typed
    /// to find something, and no rule over the events can recover the difference afterwards:
    /// both are ordinary queries and both go unpicked. Only the person who ran the benchmark
    /// knows, so this is where they say so — once, in the log, rather than as a date constant
    /// inside whichever harvester reads it next. [`fold`] drops what a span covers.
    ///
    /// Appended like everything else, because the log is never rewritten. An exclusion is
    /// therefore a line that can be read, argued with and deleted, which is the shape ADR 3
    /// gives authored data. It is also why `CS_LOG_QUERIES=0` is a convenience rather than the
    /// mechanism: forgetting to set it stays recoverable.
    Driven {
        /// When the exclusion was declared, not when the traffic it covers happened.
        ts: i64,
        /// Half-open `[from, until)`, so consecutive spans tile without an event falling into
        /// both or neither.
        from: i64,
        until: i64,
        /// What was being measured. Required, because an exclusion with no reason is precisely
        /// the silent carry this exists to prevent.
        why: String,
    },
}

impl Event {
    pub fn ts(&self) -> i64 {
        match self {
            Event::Search { ts, .. } | Event::Pick { ts, .. } | Event::Driven { ts, .. } => *ts,
        }
    }

    /// What was searched for, or `None` for an event that is not a search at all.
    pub fn query(&self) -> Option<&str> {
        match self {
            Event::Search { q, .. } | Event::Pick { q, .. } => Some(q),
            Event::Driven { .. } => None,
        }
    }

    /// The source filter in force. Part of what identifies a search: the same text under a
    /// different filter is a different question, asked of a different corpus.
    pub fn source(&self) -> Option<&str> {
        match self {
            Event::Search { source, .. } | Event::Pick { source, .. } => source.as_deref(),
            Event::Driven { .. } => None,
        }
    }

    /// Whether this event falls inside a declared driven span.
    fn is_driven(&self, spans: &[(i64, i64)]) -> bool {
        spans.iter().any(|(from, until)| self.ts() >= *from && self.ts() < *until)
    }
}

/// Trim a result list to what is worth keeping.
pub fn truncate_shown(ids: Vec<String>) -> Vec<String> {
    let mut ids = ids;
    ids.truncate(MAX_SHOWN);
    ids
}

/// Append one event.
///
/// Failure is deliberately not propagated by callers: a search that cannot write its log
/// line should still return results. Losing a log line costs a data point, while failing the
/// search costs the thing the user asked for.
pub fn append(path: &Path, event: &Event) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", serde_json::to_string(event).map_err(std::io::Error::other)?)
}

/// Every event, oldest first. Unreadable lines are counted rather than fatal, so one bad
/// line cannot cost the whole history.
pub fn load(path: &Path) -> std::io::Result<(Vec<Event>, usize)> {
    let mut out = Vec::new();
    let mut skipped = 0usize;
    let Ok(file) = std::fs::File::open(path) else {
        return Ok((out, 0));
    };
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(&line) {
            Ok(e) => out.push(e),
            Err(_) => skipped += 1,
        }
    }
    Ok((out, skipped))
}

/// A query and the conversations picked for it, most-searched first.
///
/// The shape an eval set wants: `q` is a need somebody actually had, and each conv_id under
/// it is an answer somebody actually accepted.
#[derive(Debug, Clone, Serialize)]
pub struct Need {
    pub q: String,
    pub searches: usize,
    /// Distinct conversations opened for this query, most-picked first.
    pub picked: Vec<(String, usize)>,
    /// Ranks the picks came from. A tail of high numbers means the ranking is making you
    /// scroll for something it could have put first.
    pub ranks: Vec<usize>,
    pub last_ts: i64,
}

/// How soon the next search has to arrive for this one to have gone unread.
///
/// Measured against the real log rather than chosen. Of 2,072 prefix-adjacent pairs in
/// `~/.chat-archive/queries.jsonl` on 2026-08-04, the slowest one inside a typed ladder was
/// 1,328 ms apart and the next one up was 10.9 s — so the boundary sits in the middle of a gap
/// eight times wider than the number, and moving it anywhere in that range changes nothing.
///
/// What it means, rather than what it measures: two seconds is not enough time to read a
/// result list and decide it was wrong. A search whose successor arrived inside it was never
/// answered, because nobody looked.
pub const UNREAD_MS: i64 = 2_000;

/// The log, folded into the needs behind it, and what had to be set aside to get there.
///
/// Returned as one value rather than a bare `Vec<Need>` because the discards are the point:
/// a fold that quietly drops nine events in ten is indistinguishable from a broken one, and
/// [`crate::eval`]'s harvest has to be able to say what it did not harvest.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Folded {
    pub needs: Vec<Need>,
    /// Search events that turned out to be on the way somewhere — see [`UNREAD_MS`].
    pub keystrokes: usize,
    /// Events covered by a declared [`Event::Driven`] span.
    pub driven: usize,
    /// How many spans were declared.
    pub spans: usize,
    /// Events with no query at all: the recent list, browsed rather than searched.
    pub browsed: usize,
}

/// Fold the log into one entry per need.
///
/// Three things happen before the grouping, and each of them is a claim about what the events
/// mean rather than a tidy-up:
///
/// 1. **A declared driven span is not evidence of anything.** Its events are dropped whole.
/// 2. **A search nobody read is a keystroke, not a search.** `l`, `la`, `lau` on the way to
///    `launchd` are one need typed slowly, not seven needs; so are the same query run three
///    times in 200 ms to take a median. Both collapse forward into what followed them, which
///    keeps the abandonment signal `chat-search-6eb.21` wants — the query somebody gave up on
///    is the *last* one they typed, and that one survives.
/// 3. **A pick with no query is not a relevance judgement.** It is the recent list being
///    browsed, and treating it as one would hand the eval set a grade-3 for a query that was
///    never asked.
///
/// What survives is grouped on exact query text. Normalising case or whitespace would merge
/// searches that were typed differently, and how a need gets typed is part of what is being
/// measured — the ranking sees the literal string too.
pub fn fold(events: &[Event]) -> Folded {
    use std::collections::HashMap;

    let spans: Vec<(i64, i64)> = events
        .iter()
        .filter_map(|e| match e {
            Event::Driven { from, until, .. } => Some((*from, *until)),
            _ => None,
        })
        .collect();
    let mut out = Folded { spans: spans.len(), ..Folded::default() };

    // Sorted rather than trusted. One machine appends in order, but the log is authored data
    // that will eventually be merged across machines (ADR 3), and the keystroke rule below
    // reads adjacency — two interleaved histories would make neighbours of events that were
    // hours and a continent apart.
    let mut ordered: Vec<&Event> = events.iter().filter(|e| e.query().is_some()).collect();
    ordered.sort_by_key(|e| e.ts());

    let kept: Vec<&Event> = ordered
        .into_iter()
        .filter(|e| {
            let driven = e.is_driven(&spans);
            out.driven += usize::from(driven);
            !driven
        })
        .collect();

    let mut by_q: HashMap<&str, Need> = HashMap::new();
    for (i, e) in kept.iter().enumerate() {
        if kept.get(i + 1).is_some_and(|next| absorbs(e, next)) {
            out.keystrokes += 1;
            continue;
        }
        let q = e.query().unwrap_or_default();
        if q.is_empty() {
            out.browsed += 1;
            continue;
        }
        let n = by_q.entry(q).or_insert_with(|| Need {
            q: q.to_string(),
            searches: 0,
            picked: Vec::new(),
            ranks: Vec::new(),
            last_ts: 0,
        });
        n.last_ts = n.last_ts.max(e.ts());
        n.searches += 1;
        if let Event::Pick { conv_id, rank, .. } = e {
            if let Some(r) = rank {
                n.ranks.push(*r);
            }
            match n.picked.iter_mut().find(|(c, _)| c == conv_id) {
                Some(slot) => slot.1 += 1,
                None => n.picked.push((conv_id.clone(), 1)),
            }
        }
    }

    out.needs = by_q.into_values().collect();
    for n in &mut out.needs {
        n.picked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    // Most-searched first, ties by recency, then text so the order is deterministic.
    out.needs.sort_by(|a, b| {
        b.searches
            .cmp(&a.searches)
            .then_with(|| b.last_ts.cmp(&a.last_ts))
            .then_with(|| a.q.cmp(&b.q))
    });
    out
}

/// Whether `later` is what `e` was on the way to, so `e` never stood on its own.
///
/// Three conditions, and each rules out a way of being wrong. `e` must be a search, because a
/// pick is a decision and folding one away would discard the judgement it carries. `later`
/// must repeat or extend `e`'s text under the same filter, because anything else is a
/// different question. And a blank `e` never absorbs, since the empty string is a prefix of
/// every query there is — browsing the recent list and then searching for something would
/// otherwise read as one continuous act of typing.
fn absorbs(e: &Event, later: &Event) -> bool {
    let Event::Search { q, .. } = e else { return false };
    if q.is_empty() {
        return false;
    }
    let Some(text) = later.query() else { return false };
    later.ts() - e.ts() <= UNREAD_MS && later.source() == e.source() && text.starts_with(q.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("cs-qlog-{}.jsonl", uuid::Uuid::new_v4()))
    }

    fn search(q: &str, ts: i64) -> Event {
        Event::Search { ts, q: q.into(), source: None, shown: vec!["a".into()], n: 1, ms: 1.0 }
    }

    fn pick(q: &str, conv: &str, rank: Option<usize>, ts: i64) -> Event {
        Event::Pick {
            ts,
            q: q.into(),
            source: None,
            conv_id: conv.into(),
            rank,
            shown: vec!["a".into(), "b".into()],
            n: 2,
        }
    }

    /// Far enough apart that the keystroke rule is out of the picture, for the tests that are
    /// about something else. `UNREAD_MS` is two seconds; a minute a step is unambiguous.
    fn apart(n: i64) -> i64 {
        n * 60_000
    }

    #[test]
    fn events_round_trip_through_the_file() {
        let p = tmp();
        append(&p, &search("borrow checker", 1)).unwrap();
        append(&p, &pick("borrow checker", "codex:a", Some(2), 2)).unwrap();
        let (got, skipped) = load(&p).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1], pick("borrow checker", "codex:a", Some(2), 2));
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_missing_log_is_an_empty_history_rather_than_an_error() {
        // Nothing has been searched yet is the normal state on a fresh machine.
        let (got, skipped) = load(Path::new("/nonexistent/queries.jsonl")).unwrap();
        assert!(got.is_empty());
        assert_eq!(skipped, 0);
    }

    #[test]
    fn one_unreadable_line_does_not_cost_the_rest_of_the_history() {
        let p = tmp();
        append(&p, &search("a", 1)).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"{ not json\n\n")
            .unwrap();
        append(&p, &search("b", 2)).unwrap();
        let (got, skipped) = load(&p).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(skipped, 1, "the blank line is not a failure, the garbage one is");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_need_collects_every_conversation_accepted_for_the_same_query() {
        // The shape an eval set wants: one query, and the answers a person actually took.
        let events = vec![
            search("borrow checker", apart(1)),
            pick("borrow checker", "codex:a", Some(1), apart(2)),
            pick("borrow checker", "codex:b", Some(4), apart(3)),
            pick("borrow checker", "codex:a", Some(1), apart(4)),
            pick("fts5", "codex:z", Some(2), apart(5)),
        ];
        let needs = fold(&events).needs;
        assert_eq!(needs.len(), 2);

        let bc = &needs[0];
        assert_eq!(bc.q, "borrow checker", "most-searched query comes first");
        assert_eq!(bc.searches, 4);
        assert_eq!(bc.picked, vec![("codex:a".into(), 2), ("codex:b".into(), 1)]);
        assert_eq!(bc.ranks, vec![1, 4, 1]);
        assert_eq!(bc.last_ts, apart(4));
    }

    #[test]
    fn queries_that_differ_only_in_spelling_stay_separate() {
        // The ranking sees the literal string, so merging "FTS5" into "fts5" would hide a
        // real difference in what the tokenizer and the prefix rule were handed.
        let events = [search("fts5", apart(1)), search("FTS5", apart(2)), search("fts5 ", apart(3))];
        let needs = fold(&events).needs;
        assert_eq!(needs.len(), 3);
        assert!(needs.iter().all(|n| n.searches == 1));
    }

    #[test]
    fn a_pick_that_was_never_in_the_result_list_is_kept_without_a_rank() {
        // Reached from history or by scrolling past the cap. Still a real answer to a real
        // query, and dropping it would quietly bias the log toward things that ranked well.
        let needs = fold(&[pick("borrow checker", "codex:elsewhere", None, 1)]).needs;
        assert_eq!(needs[0].picked, vec![("codex:elsewhere".into(), 1)]);
        assert!(needs[0].ranks.is_empty());
    }

    #[test]
    fn a_query_typed_one_character_at_a_time_is_one_need_rather_than_seven() {
        // The ladder that made `cs needs` unreadable: `launchd` typed into a client that
        // searches per keystroke arrived as seven separate queries, and would have reached the
        // eval set as seven, clearing its "20+ distinct queries" bar on fragments.
        let ladder: Vec<Event> = ["l", "la", "lau", "laun", "launc", "launch", "launchd"]
            .iter()
            .enumerate()
            .map(|(i, q)| search(q, i as i64 * 100))
            .collect();
        let folded = fold(&ladder);
        assert_eq!(folded.keystrokes, 6);
        assert_eq!(folded.needs.len(), 1);
        assert_eq!(folded.needs[0].q, "launchd", "the longest one is the one that was meant");
        assert_eq!(folded.needs[0].searches, 1);
    }

    #[test]
    fn a_ladder_that_ends_in_a_pick_keeps_the_pick_and_its_rank() {
        // The half that must not collapse. A pick is a decision, and folding one into anything
        // would throw away the only relevance judgement the log ever gets for free.
        let folded = fold(&[
            search("deep", 0),
            search("deep lea", 200),
            pick("deep learning", "codex:a", Some(2), 400),
        ]);
        assert_eq!(folded.keystrokes, 2);
        assert_eq!(folded.needs.len(), 1);
        assert_eq!(folded.needs[0].picked, vec![("codex:a".into(), 1)]);
        assert_eq!(folded.needs[0].ranks, vec![2]);
    }

    #[test]
    fn the_same_query_run_again_to_take_a_median_is_one_search() {
        // How a latency measurement looks from in here: three identical runs 100 ms apart.
        // Nobody retypes a query three times in a third of a second, so counting them as three
        // searches would rank a benchmark above every real need in the table.
        let folded = fold(&[search("the", 0), search("the", 100), search("the", 200)]);
        assert_eq!(folded.keystrokes, 2);
        assert_eq!(folded.needs[0].searches, 1);
    }

    #[test]
    fn a_query_retyped_later_in_the_day_is_the_same_need_searched_twice() {
        // The other side of `UNREAD_MS`: coming back to a query an hour later means the first
        // attempt was read and rejected, and both attempts are evidence.
        let folded = fold(&[search("beads", 0), search("beads", apart(60))]);
        assert_eq!(folded.keystrokes, 0);
        assert_eq!(folded.needs[0].searches, 2);
    }

    #[test]
    fn typing_on_after_reading_the_results_is_two_needs_rather_than_one() {
        // `bead` searched, read, then narrowed to `beads` a minute later. The first query was
        // answered by a list somebody looked at, so it stands on its own — which is what keeps
        // the ladder rule from swallowing a genuine narrowing.
        let folded = fold(&[search("bead", 0), search("beads", apart(1))]);
        assert_eq!(folded.keystrokes, 0);
        assert_eq!(folded.needs.len(), 2);
    }

    #[test]
    fn a_ladder_typed_under_one_source_filter_does_not_absorb_one_typed_under_another() {
        // Same text, different corpus, different question. The filter is part of the query
        // even when it arrives beside it rather than inside it.
        let codex = Event::Search {
            ts: 100,
            q: "borrow checker".into(),
            source: Some("codex".into()),
            shown: vec![],
            n: 0,
            ms: 1.0,
        };
        let folded = fold(&[search("borrow", 0), codex]);
        assert_eq!(folded.keystrokes, 0);
        assert_eq!(folded.needs.len(), 2);
    }

    #[test]
    fn browsing_the_recent_list_never_becomes_a_relevance_judgement() {
        // A pick with no query says a conversation was wanted; it does not say which of a
        // ranked list answered a question, because no question was asked. Handing it to the
        // eval set as a grade-3 would judge the ranking against a query nobody typed.
        let folded = fold(&[
            pick("", "codex:a", Some(2), apart(1)),
            pick("", "codex:b", None, apart(2)),
            pick("beads", "codex:c", Some(1), apart(3)),
        ]);
        assert_eq!(folded.browsed, 2);
        assert_eq!(folded.needs.len(), 1, "only the one that was searched for");
        assert_eq!(folded.needs[0].q, "beads");
    }

    #[test]
    fn a_blank_query_is_not_a_prefix_of_whatever_was_searched_for_next() {
        // The empty string starts every query there is, so without the guard in `absorbs` a
        // browse followed by a search would read as one act of typing and the pick made from
        // the recent list would vanish into the search that followed it.
        let folded = fold(&[pick("", "codex:a", Some(1), 0), search("beads", 500)]);
        assert_eq!(folded.browsed, 1);
        assert_eq!(folded.needs.len(), 1);
    }

    #[test]
    fn a_declared_driven_span_takes_its_traffic_out_of_the_needs_entirely() {
        // A benchmark's queries are indistinguishable from real ones by shape — they are
        // ordinary text and they go unpicked, which is also what an abandoned search looks
        // like. So the exclusion is authored rather than inferred, and it is the span that
        // says so, not a rule.
        let events = vec![
            search("borrow checker", apart(1)),
            search("borrow checker", apart(2)),
            search("beads", apart(10)),
            Event::Driven {
                ts: apart(20),
                from: apart(1),
                until: apart(3),
                why: "typeahead latency, chat-search-6eb.38".into(),
            },
        ];
        let folded = fold(&events);
        assert_eq!(folded.driven, 2);
        assert_eq!(folded.spans, 1);
        assert_eq!(folded.needs.len(), 1);
        assert_eq!(folded.needs[0].q, "beads");
    }

    #[test]
    fn a_driven_span_is_half_open_so_two_of_them_tile() {
        // Same reason `date:` windows are half-open: an event on the boundary has to belong to
        // exactly one span, or excluding a morning and an afternoon separately double-counts
        // the instant between them.
        let spanned = |from, until| Event::Driven { ts: 0, from, until, why: "measuring".into() };
        let folded = fold(&[
            search("a", 100),
            search("b", 200),
            search("c", 300),
            spanned(100, 200),
            spanned(200, 300),
        ]);
        assert_eq!(folded.driven, 2, "100 and 200 are covered, 300 is past the end");
        assert_eq!(folded.needs.len(), 1);
        assert_eq!(folded.needs[0].q, "c");
    }

    #[test]
    fn a_span_declared_on_one_machine_survives_the_round_trip_through_the_file() {
        // It has to: the exclusion travels with the log rather than living in whatever code
        // reads it, which only works if an older reader does not choke on the line.
        let p = tmp();
        let span = Event::Driven { ts: 3, from: 1, until: 2, why: "smoke test".into() };
        append(&p, &span).unwrap();
        let (got, skipped) = load(&p).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(got, vec![span]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn events_are_folded_in_time_order_rather_than_file_order() {
        // One machine appends in order, but the log is authored data headed for a merge across
        // machines (ADR 3), and the keystroke rule reads adjacency. Two histories concatenated
        // would otherwise make neighbours of events hours apart.
        let folded = fold(&[search("launchd", 300), search("launch", 200), search("laun", 100)]);
        assert_eq!(folded.keystrokes, 2);
        assert_eq!(folded.needs[0].q, "launchd");
    }

    #[test]
    fn a_long_result_list_is_capped_before_it_is_written() {
        let ids: Vec<String> = (0..100).map(|i| format!("c:{i}")).collect();
        let kept = truncate_shown(ids);
        assert_eq!(kept.len(), MAX_SHOWN);
        assert_eq!(kept[0], "c:0", "the top of the list is the part worth keeping");
    }
}
