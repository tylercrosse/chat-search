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
}

impl Event {
    pub fn ts(&self) -> i64 {
        match self {
            Event::Search { ts, .. } | Event::Pick { ts, .. } => *ts,
        }
    }

    pub fn query(&self) -> &str {
        match self {
            Event::Search { q, .. } | Event::Pick { q, .. } => q,
        }
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

/// Fold the log into one entry per distinct query.
///
/// Queries are grouped on their exact text. Normalising case or whitespace would merge
/// searches that were typed differently, and how a need gets typed is part of what is being
/// measured — the ranking sees the literal string too.
pub fn needs(events: &[Event]) -> Vec<Need> {
    use std::collections::HashMap;
    let mut by_q: HashMap<&str, Need> = HashMap::new();
    for e in events {
        let n = by_q.entry(e.query()).or_insert_with(|| Need {
            q: e.query().to_string(),
            searches: 0,
            picked: Vec::new(),
            ranks: Vec::new(),
            last_ts: 0,
        });
        n.last_ts = n.last_ts.max(e.ts());
        match e {
            Event::Search { .. } => n.searches += 1,
            Event::Pick { conv_id, rank, .. } => {
                n.searches += 1;
                if let Some(r) = rank {
                    n.ranks.push(*r);
                }
                match n.picked.iter_mut().find(|(c, _)| c == conv_id) {
                    Some(slot) => slot.1 += 1,
                    None => n.picked.push((conv_id.clone(), 1)),
                }
            }
        }
    }
    let mut out: Vec<Need> = by_q.into_values().collect();
    for n in &mut out {
        n.picked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }
    // Most-searched first, ties by recency, then text so the order is deterministic.
    out.sort_by(|a, b| {
        b.searches
            .cmp(&a.searches)
            .then_with(|| b.last_ts.cmp(&a.last_ts))
            .then_with(|| a.q.cmp(&b.q))
    });
    out
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
            search("borrow checker", 1),
            pick("borrow checker", "codex:a", Some(1), 2),
            pick("borrow checker", "codex:b", Some(4), 3),
            pick("borrow checker", "codex:a", Some(1), 4),
            pick("fts5", "codex:z", Some(2), 5),
        ];
        let needs = needs(&events);
        assert_eq!(needs.len(), 2);

        let bc = &needs[0];
        assert_eq!(bc.q, "borrow checker", "most-searched query comes first");
        assert_eq!(bc.searches, 4);
        assert_eq!(bc.picked, vec![("codex:a".into(), 2), ("codex:b".into(), 1)]);
        assert_eq!(bc.ranks, vec![1, 4, 1]);
        assert_eq!(bc.last_ts, 4);
    }

    #[test]
    fn queries_that_differ_only_in_spelling_stay_separate() {
        // The ranking sees the literal string, so merging "FTS5" into "fts5" would hide a
        // real difference in what the tokenizer and the prefix rule were handed.
        let needs = needs(&[search("fts5", 1), search("FTS5", 2), search("fts5 ", 3)]);
        assert_eq!(needs.len(), 3);
        assert!(needs.iter().all(|n| n.searches == 1));
    }

    #[test]
    fn a_pick_that_was_never_in_the_result_list_is_kept_without_a_rank() {
        // Reached from history or by scrolling past the cap. Still a real answer to a real
        // query, and dropping it would quietly bias the log toward things that ranked well.
        let needs = needs(&[pick("borrow checker", "codex:elsewhere", None, 1)]);
        assert_eq!(needs[0].picked, vec![("codex:elsewhere".into(), 1)]);
        assert!(needs[0].ranks.is_empty());
    }

    #[test]
    fn a_long_result_list_is_capped_before_it_is_written() {
        let ids: Vec<String> = (0..100).map(|i| format!("c:{i}")).collect();
        let kept = truncate_shown(ids);
        assert_eq!(kept.len(), MAX_SHOWN);
        assert_eq!(kept[0], "c:0", "the top of the list is the part worth keeping");
    }
}
