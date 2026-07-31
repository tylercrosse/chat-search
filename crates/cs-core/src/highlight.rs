//! Which words in a message actually matched, and where.
//!
//! Separate from [`crate::search`] because it answers a different question. Search asks
//! *which rows*; this asks *which characters of one row*, and the two must agree — a
//! highlighter that disagrees with the ranker produces the worst possible result, a row
//! present with nothing marked and no visible reason for it being there.
//!
//! That agreement is the whole difficulty. `fts_prose` tokenises with
//! `porter unicode61 remove_diacritics 2` (see `schema.rs`), so `commits` and `commit` are
//! the same term to the ranker while a substring scan finds neither in the other.
//! `search::snippet` did exactly that scan until `chat-search-6eb.20`, and now delegates here.
//!
//! # Why this asks SQLite instead of stemming in Rust
//!
//! The obvious fix is to stem in-process with `rust-stemmers` and compare stems. It does not
//! work: `Algorithm::English` is Porter2/Snowball, and SQLite's `porter` tokenizer is the
//! *original* Porter. Measured over the 30,000 most frequent words of the prose corpus,
//! stemmed both ways, **1,140 words (3.8%) get different stems**, and — the number that
//! actually matters, since highlighting is a question about pairs — of the ~21.7k word pairs
//! the two stemmers group together, **1,425 are grouped by one and not the other**: 669 that
//! SQLite groups and Porter2 does not (query `general` ranks a message saying `generated`,
//! both `gener` to SQLite; Porter2 says `general`/`generat` and marks nothing) and 756 the
//! other way (query `exactly` would mark `exact`, which is *not* why the row ranked — SQLite
//! has `exactli` and `exact`). Same family as the bug being fixed, just quieter.
//!
//! Reimplementing original Porter, unicode61's token boundaries and its `remove_diacritics 2`
//! folding table in Rust is the only way to close that gap, and each of the three is a
//! separate chance to drift from the C the index is built with. So [`spans`] hands the text
//! and the terms to a throwaway in-memory fts5 table declared with the identical tokenizer
//! and lets SQLite's own `highlight()` say where the matches are. It agrees by construction —
//! with the stemmer, the diacritic folding, prefix terms, and whatever the tokenizer string
//! becomes next.
//!
//! # What it costs, and what would make it free
//!
//! A delete, an insert and a match per call, on a connection reused per thread (the scratch
//! table costs ~86 µs to build, so per-call construction would dominate; it does not grow —
//! `page_count` is flat at 63 after 40,000 calls). Release build, real prose messages pulled
//! from the 172k-message index: ~25 µs fixed plus ~60 ns per byte of message, so 57 µs for
//! the `commit` result set (mean 536 B), 139 µs for `learning` (mean 2.2 KB) and 417 µs for
//! `borrow` (mean 11.6 KB, max 104 KB). A 50-message preview is 3–7 ms and a 100-row result
//! list 6–14 ms, both inside the 30 ms keystroke budget (chat-search-me9.1).
//!
//! The TUI is the pressure point, because `limit 50 × MAX_HITS 3` means 150 snippets per
//! keystroke. `search_grouped` on the real index, nested=0 against nested=3: `borrow`
//! 0.3 → 11 ms, `learning` 6.4 → 36 ms, `deep learning` 1.9 → 29 ms, `pro` 30 → 46 ms,
//! `the` 61 → 84 ms. The substring scan this replaces did the same 150 in 0.3–1.9 ms, so this
//! is a real 10–25 ms addition, and `learning` crosses the budget on it.
//!
//! It is paid for the insert, not the matching: on a row already in the table, `highlight()`
//! is 8 µs for a small message and 373 µs for the 104 KB one, against 1.5 ms to insert that
//! same message. Which says where the fix is — declaring `fts_prose` as external content
//! (`content='message', content_rowid='rowid'`) instead of contentless would let `highlight()`
//! run against the index that already holds the postings, with no insert at all, and the
//! ranking query already joins `message` by rowid. That is a schema change and a reindex, so
//! it is chat-search-6eb.30 rather than part of this fix. Correctness first: every one of the
//! 789 rows across those six queries located its match, where the substring scan could not.

use rusqlite::{params, OptionalExtension};
use std::cell::RefCell;

/// Must stay identical to the tokenizer in `schema::DDL` — the entire premise here is that
/// this table and `fts_prose` reduce a word the same way. `tokenizer_matches_the_index`
/// fails if the two drift.
const TOKENIZER: &str = "porter unicode61 remove_diacritics 2";

/// Filter keywords of the query DSL (`chat-search-6eb.11`), which name a facet rather than
/// text to find. `agent:codex learning` must highlight `learning` and neither `agent` nor
/// `codex` — the row was not ranked on those words.
const FILTERS: [&str; 3] = ["agent:", "dir:", "date:"];

/// An fts5 expression matching text that contains **any** of `terms`.
///
/// OR, where the ranker ANDs, for the reason [`spans`] gives: on the message that ranked, every
/// term is present anyway, and OR is what keeps this honest for a caller holding a subset.
fn any_expr(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| match t.strip_suffix('*') {
            Some(stem) => format!("\"{}\"*", stem.replace('"', "\"\"")),
            None => format!("\"{}\"", t.replace('"', "\"\"")),
        })
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// A matched run of `text`, as byte offsets into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// The query's terms, reduced the way the index reduces them.
///
/// Filter keywords are dropped: once the DSL lands (`chat-search-6eb.11`) a query like
/// `agent:codex learning` must not highlight the words "agent" and "codex".
///
/// Reduction stops at case folding and splitting, because the reductions that follow —
/// diacritic folding and stemming — belong to the tokenizer and are applied by [`spans`],
/// which runs the real one. Doing them here in Rust is the trap this module exists to avoid.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in query.to_lowercase().split_whitespace() {
        // A leading `-` is negation in the DSL; either way the keyword is not searched text.
        let bare = word.strip_prefix('-').unwrap_or(word);
        if FILTERS.iter().any(|f| bare.starts_with(f)) {
            continue;
        }
        // Split the same way `search::to_match_expr_opts` does, so a term reaching the index
        // and a term reaching the highlighter came out of one rule.
        for term in bare.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !term.is_empty() && !out.iter().any(|t| t == term) {
                out.push(term.to_string());
            }
        }
    }
    out
}

/// Every span of `text` whose token matches one of `terms`.
///
/// Ascending by `start`, non-overlapping. Empty when nothing matched, which is a fact worth
/// distinguishing from "matched at position 0".
///
/// A term ending in `*` is a prefix term, as in fts5 itself — the typeahead ranks its final
/// token that way (`search::to_match_expr_opts`), so `"lea"*` has to mark `learning` here too
/// or every in-progress word in the TUI reads as unmatched.
///
/// Terms are OR'd. The ranker AND's them, so on the message that ranked they are all present
/// anyway; OR is what keeps this useful for a caller holding a subset, and it never invents a
/// span the ranker would not have counted.
pub fn spans(text: &str, terms: &[String]) -> Vec<Span> {
    if terms.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let Some((open, close)) = sentinels(text) else {
        return Vec::new();
    };
    let expr = any_expr(terms);

    let marked = SCRATCH.with(|cell| -> rusqlite::Result<Option<String>> {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(scratch_table()?);
        }
        let conn = slot.as_ref().expect("just built");
        conn.prepare_cached("DELETE FROM hl")?.execute([])?;
        conn.prepare_cached("INSERT INTO hl(rowid, text) VALUES (1, ?1)")?.execute(params![text])?;
        // Named rather than a tail expression: a `CachedStatement` borrows the connection,
        // which borrows the `RefCell` guard, and a temporary would outlive both.
        let mut q =
            conn.prepare_cached("SELECT highlight(hl, 0, char(?1), char(?2)) FROM hl WHERE hl MATCH ?3")?;
        let found = q.query_row(params![open, close, expr], |r| r.get(0)).optional()?;
        Ok(found)
    });

    match marked {
        Ok(Some(m)) => marked_spans(&m, open, close),
        // No row means the expression did not match this text, which is the honest answer.
        Ok(None) => Vec::new(),
        // The only realistic error is a MATCH expression fts5 will not parse, and "cannot
        // locate the match" is exactly what that is. It surfaces as an empty span list, which
        // callers already have to render as such — it does not become a silent head-of-text.
        Err(_) => Vec::new(),
    }
}

/// [`spans`] for a whole conversation at once, returning one span list per text.
///
/// Identical answers, one trip through the scratch table instead of one per message. That is the
/// whole of it, and it is worth a separate entry point because the cost here is *per call*, not
/// per byte. Measured on the corpus's longest conversation, marking the same 937 KB:
///
/// | calls | time |
/// | ---: | ---: |
/// | 1,468 | 224.3 ms |
/// | 367 | 135.1 ms |
/// | 23 | 68.0 ms |
/// | 1 | 52.7 ms |
///
/// Same bytes throughout, so ~170 ms of that was round trips rather than work. Outside an
/// explicit transaction SQLite wraps every `INSERT` in an implicit one of its own, and a
/// `DELETE` and a `MATCH` were being paid per message on top.
///
/// A text that matched nothing gets an empty list, exactly as [`spans`] would give it, so
/// position in the returned vector is the only thing tying an answer to its input.
pub fn spans_many(texts: &[&str], terms: &[String]) -> Vec<Vec<Span>> {
    let nothing = || texts.iter().map(|_| Vec::new()).collect::<Vec<_>>();
    if terms.is_empty() || texts.is_empty() {
        return nothing();
    }
    // Chosen against every text at once: a delimiter that is free in one message and present in
    // the next would shift that message's offsets and nothing would say so.
    //
    // Falling back rather than giving up, because the pool is shared here and a single text that
    // spends the last free pair would otherwise cost the *whole conversation* its marks, where
    // asking one at a time costs only the message that did it. This is an optimisation; it may
    // not answer worse than the thing it optimises.
    let Some((open, close)) = sentinels_for(texts) else {
        return texts.iter().map(|text| spans(text, terms)).collect();
    };
    let expr = any_expr(terms);

    let marked = SCRATCH.with(|cell| -> rusqlite::Result<Vec<(i64, String)>> {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(scratch_table()?);
        }
        let conn = slot.as_ref().expect("just built");

        conn.execute_batch("BEGIN")?;
        let filled = (|| -> rusqlite::Result<()> {
            conn.prepare_cached("DELETE FROM hl")?.execute([])?;
            let mut ins = conn.prepare_cached("INSERT INTO hl(rowid, text) VALUES (?1, ?2)")?;
            for (i, text) in texts.iter().enumerate() {
                ins.execute(params![i as i64 + 1, text])?;
            }
            Ok(())
        })();
        // Ended whichever way it went. A transaction left open would poison every later call on
        // this thread-local connection, and that failure surfaces somewhere else entirely.
        let ended = conn.execute_batch(if filled.is_ok() { "COMMIT" } else { "ROLLBACK" });
        filled?;
        ended?;

        // Named, not a tail expression, for the same reason [`spans`] names its own: the
        // statement borrows the connection, which borrows the `RefCell` guard, and the rows
        // have to be collected before any of that is released.
        let mut q = conn.prepare_cached(
            "SELECT rowid, highlight(hl, 0, char(?1), char(?2)) FROM hl WHERE hl MATCH ?3",
        )?;
        let found = q
            .query_map(params![open, close, expr], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<(i64, String)>>>()?;
        drop(q);
        Ok(found)
    });

    let mut out = nothing();
    // Only matching rows come back, so anything absent keeps the empty list it started with.
    if let Ok(rows) = marked {
        for (rowid, text) in rows {
            if let Some(slot) = out.get_mut((rowid - 1).max(0) as usize) {
                *slot = marked_spans(&text, open, close);
            }
        }
    }
    out
}

/// A window of `text` around its densest cluster of matches, with spans relative to the
/// window.
///
/// Anchors on the *best* cluster, not the earliest match. Both this codebase and fast-resume
/// took `min(position)`, so in a long message one incidental early mention outranks the
/// passage the topic actually lives in.
///
/// The returned string is at most `width` display columns and carries `…` where it was cut.
/// Columns are counted as `char`s, which is exact for the Latin text this corpus is and off
/// by a column per CJK glyph or emoji; the renderer that cares measures properly
/// (`cs-tui::text`).
///
/// With no terms, or with terms that cannot be located, the window is the head of the text
/// and the span list is **empty**. Empty is the signal: a caller that renders the string
/// without checking is showing a preview, not a match, and [`crate::search::snippet`] labels
/// it as one.
pub fn snippet(text: &str, terms: &[String], width: usize) -> (String, Vec<Span>) {
    // A snippet is one line, so the newlines go first — and everything below indexes the
    // flattened text, since that is what the offsets have to point into.
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let hits = spans(&flat, terms);

    // Char-index form of the matches, which is what the window arithmetic works in. Walked
    // rather than tabulated: a byte-offset-per-char table is 8 bytes per char of the message,
    // and this corpus holds prose messages up to 104 KB. Span endpoints ascend, so one pass
    // resolves all of them.
    let mut ends = hits.iter().flat_map(|s| [s.start, s.end]).peekable();
    let mut marks: Vec<usize> = Vec::with_capacity(hits.len() * 2);
    let mut n = 0;
    for (b, _) in flat.char_indices() {
        while ends.next_if_eq(&b).is_some() {
            marks.push(n);
        }
        n += 1;
    }
    for _ in ends {
        marks.push(n); // an endpoint at the very end of the text
    }
    debug_assert_eq!(marks.len(), hits.len() * 2, "a span endpoint fell inside a character");
    let cs: Vec<(usize, usize)> = marks.chunks_exact(2).map(|p| (p[0], p[1])).collect();

    // `width` has to cover the ellipses too, and whether there are ellipses depends on the
    // window — so place the window, see which edges got cut, re-place with what is left.
    // Shrinking can only add an ellipsis, never remove one, so this settles; the bound is
    // there so a future change cannot turn it into a spin.
    let mut inner = width;
    let (mut start, mut end) = (0, 0);
    for _ in 0..3 {
        (start, end) = place(&cs, n, inner);
        let want = width.saturating_sub((start > 0) as usize + (end < n) as usize);
        if want >= inner {
            break;
        }
        inner = want;
    }
    end = end.min(start + inner);

    let lead = if start > 0 { "…" } else { "" };
    let trail = if end < n { "…" } else { "" };
    let mut cut = flat.char_indices().map(|(b, _)| b).skip(start);
    let wa = cut.next().unwrap_or(flat.len());
    // `next` already consumed char `start`, so the window's last char is `end - start - 1`
    // further on.
    let wb = match end.checked_sub(start + 1) {
        Some(k) => cut.nth(k).unwrap_or(flat.len()),
        None => wa,
    };
    let out = format!("{lead}{}{trail}", &flat[wa..wb]);

    // Clipped to the window rather than dropped: a match the window only half-covers is still
    // the reason the row is here, and half a mark beats none.
    let shown = hits
        .iter()
        .filter_map(|s| {
            let (a, b) = (s.start.max(wa), s.end.min(wb));
            // Lazily: a span entirely before the window makes `b - wa` underflow.
            (a < b).then(|| Span { start: a - wa + lead.len(), end: b - wa + lead.len() })
        })
        .collect();
    (out, shown)
}

/// Where to cut, in chars: the `inner`-wide window holding the most matches.
///
/// Ties go to the earliest window, so a message that says the same thing twice reads from the
/// first time it said it.
fn place(cs: &[(usize, usize)], n: usize, inner: usize) -> (usize, usize) {
    if cs.is_empty() || inner == 0 {
        return (0, inner.min(n));
    }
    // Ends ascend with starts, so the matches that fit alongside `i` are a prefix of what
    // follows it — `take_while` is the whole search.
    let (best, count) = cs
        .iter()
        .enumerate()
        .map(|(i, &(s, _))| (i, cs[i..].iter().take_while(|&&(_, e)| e <= s + inner).count()))
        .max_by_key(|&(i, count)| (count, std::cmp::Reverse(i)))
        .unwrap_or((0, 1));

    let (from, to) = (cs[best].0, cs[best + count.max(1) - 1].1);
    // Centre the cluster in the window; a cluster wider than the window starts at its head.
    let mut start = from.saturating_sub(inner.saturating_sub(to - from) / 2);
    let mut end = (start + inner).min(n);
    // Against the tail there is slack on the left; spend it rather than return a short window.
    if end - start < inner {
        start = end.saturating_sub(inner);
    }
    end = (start + inner).min(n);
    (start, end)
}

thread_local! {
    /// One scratch table per thread, built on first use and reused. Building it costs ~86 µs
    /// against ~25 µs of fixed cost per highlight, so a table per call would dominate. It does
    /// not accumulate: `page_count` is still 63 after 40,000 delete/insert cycles.
    static SCRATCH: RefCell<Option<rusqlite::Connection>> = const { RefCell::new(None) };
}

fn scratch_table() -> rusqlite::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open_in_memory()?;
    // Content-carrying, unlike `fts_prose`: `highlight()` reconstructs from stored content, and
    // that requirement is why the index's own contentless tables cannot answer this (ADR 5).
    conn.execute_batch(&format!(
        "PRAGMA journal_mode=OFF;
         CREATE VIRTUAL TABLE hl USING fts5(text, tokenize=\"{TOKENIZER}\");"
    ))?;
    Ok(conn)
}

/// Two marker bytes that do not occur in `text`.
///
/// `highlight()` communicates by inserting delimiters into the text, so a delimiter the text
/// already contains would be read back as a match boundary and shift every offset after it.
/// Tool output is not guaranteed to be free of control characters, so the pair is chosen
/// rather than assumed.
///
/// Scanned as bytes, not chars: U+0001..U+0006 encode as themselves and no UTF-8
/// continuation byte falls in that range, so the byte scan and the char scan agree — and on
/// the 100 KB messages this corpus does contain, the char scan was measurable on its own.
fn sentinels(text: &str) -> Option<(u8, u8)> {
    sentinels_for(&[text])
}

/// Control bytes that may stand in as delimiters: C0 minus tab, newline and carriage return,
/// which are ordinary text here.
///
/// Every one encodes as itself and no UTF-8 continuation byte falls in the range, so scanning
/// bytes and scanning chars agree — and on the 100 KB messages this corpus contains, the char
/// scan was measurable on its own.
///
/// Wide on purpose. A single message rarely holds any of these, but [`spans_many`] has to find a
/// pair free across *every* message at once, and the six candidates this started with were
/// exhausted by a handful of texts between them. Running out is still possible and still
/// handled; it just stops being likely.
const DELIMITERS: [u8; 28] = [
    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0B, 0x0C, 0x0E, 0x0F, 0x10, 0x11, 0x12,
    0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F,
];

/// [`sentinels`] over several texts, which all have to share one pair — a batch reads its
/// answers back out of one query, so a delimiter free in one text and present in another would
/// silently shift the second one's offsets.
fn sentinels_for(texts: &[&str]) -> Option<(u8, u8)> {
    let mut taken = [false; DELIMITERS.len()];
    for text in texts {
        for &b in text.as_bytes() {
            if let Some(i) = DELIMITERS.iter().position(|&d| d == b) {
                taken[i] = true;
            }
        }
    }
    let mut free = DELIMITERS.iter().enumerate().filter(|(i, _)| !taken[*i]).map(|(_, &d)| d);
    Some((free.next()?, free.next()?))
}

/// Offsets of the delimited runs, expressed in the *undelimited* text.
fn marked_spans(marked: &str, open: u8, close: u8) -> Vec<Span> {
    let mut out = Vec::new();
    // Markers are one byte each, so the original offset of byte `i` is `i` less the markers
    // seen before it.
    let mut dropped = 0usize;
    let mut start = None;
    for (i, &b) in marked.as_bytes().iter().enumerate() {
        if b == open {
            start = Some(i - dropped);
            dropped += 1;
        } else if b == close {
            let at = i - dropped;
            dropped += 1;
            match start.take() {
                Some(s) if at > s => out.push(Span { start: s, end: at }),
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(q: &str) -> Vec<String> {
        query_terms(q)
    }

    #[test]
    fn tokenizer_matches_the_index() {
        // The scratch table only agrees with the ranker while these are the same string, and
        // nothing else would fail if someone changed one of them.
        assert!(
            crate::schema::DDL.matches(&format!("tokenize=\"{TOKENIZER}\"")).count() >= 2,
            "highlight.rs and schema.rs disagree on the tokenizer"
        );
    }

    #[test]
    fn stemmed_and_folded_terms_are_located() {
        // The bug: each of these ranks the row and the old substring scan found none of them.
        for (query, text) in [
            ("commits", "Commit current changes"),
            ("learning", "I learned about it"),
            ("running", "it runs every hour"),
            ("cafe", "we met at the café"),
            ("GENERAL", "the generated code"),
        ] {
            assert!(!spans(text, &terms(query)).is_empty(), "{query:?} in {text:?}");
        }
    }

    #[test]
    fn marking_a_batch_gives_every_text_the_answer_it_would_have_got_alone() {
        // The batch exists only to be faster, so the one thing that must never differ is the
        // answer. Position is all that ties a result to its input, so a text that matched
        // nothing has to hold its place rather than be dropped.
        let texts = [
            "Commit current changes",
            "nothing of interest here",
            "the café was closed",
            "",
            "commit, commits and committing",
            "learned something",
        ];
        for query in ["commits", "cafe", "learning", "elephant", "commit café"] {
            let t = terms(query);
            let batch = spans_many(&texts, &t);
            let alone: Vec<Vec<Span>> = texts.iter().map(|x| spans(x, &t)).collect();
            assert_eq!(batch, alone, "{query:?}");
            assert_eq!(batch.len(), texts.len(), "{query:?} lost a text");
        }
        // Degenerate inputs keep the shape callers index into.
        assert_eq!(spans_many(&texts, &[]).len(), texts.len());
        assert!(spans_many(&[], &terms("commit")).is_empty());
    }

    #[test]
    fn a_batch_shares_one_pair_of_delimiters_across_every_text() {
        // Offsets are read back out of one query, so a delimiter free in one text and present
        // in the next would shift the second one's marks with nothing to say so.
        let texts = ["\u{1}\u{2} café here", "plain café", "\u{3}\u{4}\u{5} café again"];
        let got = spans_many(&texts, &terms("cafe"));
        for (text, marks) in texts.iter().zip(&got) {
            assert_eq!(marks.len(), 1, "{text:?}");
            assert_eq!(&text[marks[0].start..marks[0].end], "café");
        }
    }

    #[test]
    fn a_batch_that_runs_out_of_delimiters_falls_back_instead_of_marking_nothing() {
        // The pool is shared across the batch, so one text holding every candidate would cost
        // the whole conversation its marks — where asking one at a time costs only that text.
        // A batch is an optimisation and may not answer worse than what it optimises.
        let hog: String = DELIMITERS.iter().map(|&b| b as char).collect();
        let texts = [hog.as_str(), "the borrow checker", "borrowing again"];
        assert_eq!(sentinels_for(&texts), None, "the fixture has to actually exhaust the pool");

        let got = spans_many(&texts, &terms("borrow"));
        let alone: Vec<Vec<Span>> = texts.iter().map(|t| spans(t, &terms("borrow"))).collect();
        assert_eq!(got, alone);
        assert!(!got[1].is_empty() && !got[2].is_empty(), "the innocent texts still got marks");
    }

    #[test]
    fn a_failed_batch_leaves_the_scratch_connection_usable() {
        // A transaction left open would poison every later call on this thread's connection,
        // and the damage would surface in whatever ran next rather than here.
        let broken = vec!["\"".to_string()];
        let _ = spans_many(&["commit current changes"], &broken);
        assert!(
            !spans("commit current changes", &terms("commits")).is_empty(),
            "the scratch table did not survive a failed batch",
        );
    }

    #[test]
    fn a_term_that_is_not_there_yields_nothing() {
        assert!(spans("commit current changes", &terms("elephant")).is_empty());
        // Substring, not token: `cat` is in `catastrophe` and is not a term of it.
        assert!(spans("a catastrophe", &terms("cat")).is_empty());
        assert!(spans("anything", &[]).is_empty());
        assert!(spans("", &terms("anything")).is_empty());
    }

    #[test]
    fn spans_point_at_the_right_bytes_through_multibyte_text() {
        let text = "héllo café wörld";
        let got = spans(text, &terms("cafe"));
        assert_eq!(got.len(), 1);
        assert_eq!(&text[got[0].start..got[0].end], "café");
    }

    #[test]
    fn spans_ascend_and_do_not_overlap() {
        let text = "commit the commits after committing each commit again";
        let got = spans(text, &terms("commit"));
        assert_eq!(got.len(), 4, "four tokens stem to `commit`: {got:?}");
        assert!(got.windows(2).all(|w| w[0].end <= w[1].start), "{got:?}");
        assert!(got.iter().all(|s| s.start < s.end && s.end <= text.len()));
    }

    #[test]
    fn filter_tokens_never_become_highlight_terms() {
        // Ranked on `learning` alone; `agent`, `codex`, `~/src` and the date are facets.
        assert_eq!(terms("agent:codex learning"), ["learning"]);
        assert_eq!(terms("-agent:codex -dir:~/src date:2026-07-30 borrow"), ["borrow"]);
        assert_eq!(terms("AGENT:Codex Borrow"), ["borrow"]);
        // A colon that is not a filter keyword is still text.
        assert_eq!(terms("http://example.com/x"), ["http", "example", "com", "x"]);
        assert!(terms("agent:codex").is_empty());
    }

    #[test]
    fn terms_are_deduplicated_in_order() {
        // The order decides tie-breaks nowhere today, but a term list that reshuffles between
        // identical queries makes every downstream assertion flaky.
        assert_eq!(terms("Borrow borrow BORROW checker"), ["borrow", "checker"]);
        assert_eq!(terms("  "), Vec::<String>::new());
        assert_eq!(terms("-"), Vec::<String>::new());
    }

    #[test]
    fn the_window_follows_the_densest_cluster_not_the_first_match() {
        // One passing mention at the top, the actual discussion far below. `min(position)`
        // — what this and fast-resume both did — shows the mention.
        let filler = "lorem ipsum dolor sit amet ".repeat(20);
        let text = format!("a note about borrow {filler} the borrow checker and borrow rules {filler}");
        let (out, marks) = snippet(&text, &terms("borrow"), 60);
        assert!(out.contains("borrow checker"), "anchored on the aside: {out}");
        assert_eq!(marks.len(), 2, "both matches in the window are marked: {out:?} {marks:?}");
        for m in &marks {
            assert_eq!(&out[m.start..m.end], "borrow");
        }
    }

    #[test]
    fn the_window_never_exceeds_its_width() {
        // Rendered into a fixed column; the ellipses count against the budget, not on top.
        let text = "alpha ".repeat(200) + "needle " + &"omega ".repeat(200);
        for width in [8, 20, 60, 160] {
            let (out, _) = snippet(&text, &terms("needle"), width);
            assert!(out.chars().count() <= width, "width {width}: {} chars", out.chars().count());
        }
        // Short text is returned whole, with no ellipsis claiming it was cut.
        let (out, _) = snippet("just this", &terms("this"), 160);
        assert_eq!(out, "just this");
    }

    #[test]
    fn spans_are_relative_to_the_returned_string_ellipsis_included() {
        let text = "x ".repeat(100) + "café " + &"y ".repeat(100);
        let (out, marks) = snippet(&text, &terms("cafe"), 40);
        assert!(out.starts_with('…') && out.ends_with('…'), "{out}");
        assert_eq!(marks.len(), 1);
        assert_eq!(&out[marks[0].start..marks[0].end], "café");
    }

    #[test]
    fn an_unlocatable_match_returns_no_spans_rather_than_a_bogus_one() {
        // The bug's signature: `at` was None, `start` fell back to 0, and the head of the
        // message rendered as though position 0 were the match site.
        let (out, marks) = snippet("commit current changes", &terms("elephant"), 160);
        assert!(marks.is_empty(), "nothing matched, so nothing is marked");
        assert_eq!(out, "commit current changes", "the text is still previewable");

        let (out, marks) = snippet("commit current changes", &[], 160);
        assert!(marks.is_empty() && out == "commit current changes");
    }

    #[test]
    fn newlines_are_flattened_before_the_offsets_are_taken() {
        // Offsets that point into the unflattened text land in the wrong place once the
        // renderer collapses it — so the flattening happens first and everything indexes that.
        let (out, marks) = snippet("first line\n\n  second   café  line", &terms("cafe"), 160);
        assert_eq!(out, "first line second café line");
        assert_eq!(&out[marks[0].start..marks[0].end], "café");
    }

    #[test]
    fn a_prefix_term_marks_what_the_typeahead_ranked() {
        // `to_match_expr_opts(_, true)` sends `"lea"*`, which ranks a message on `learning`;
        // the exact term `lea` is in no message at all, so without this every in-progress
        // word in the TUI would render as unmatched.
        let text = "learning to leave";
        let got = spans(text, &["lea*".to_string()]);
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(&text[got[0].start..got[0].end], "learning");
        assert!(spans(text, &["lea".to_string()]).is_empty(), "exact term, no prefix");
    }

    #[test]
    fn control_characters_in_the_text_do_not_shift_the_offsets() {
        // Tool output is not guaranteed printable, and the marker chars are borrowed from the
        // same range — a collision would silently move every span after it.
        let text = "\u{1}\u{2}\u{3} the café \u{1} here";
        let got = spans(text, &terms("cafe"));
        assert_eq!(got.len(), 1);
        assert_eq!(&text[got[0].start..got[0].end], "café");
    }
}
