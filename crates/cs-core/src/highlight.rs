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
//! # Two tables that can answer, and how the cost splits between them
//!
//! [`spans_for`] is the entry point. Underneath it are [`spans_indexed`], which asks the corpus
//! index — whose postings for this row already exist — and [`spans_many`], which re-indexes the
//! result set into a per-thread scratch table and asks that. Being able to ask the corpus index
//! at all is new: it needs `fts_prose` and `fts_tools` declared `content='message'` rather than
//! contentless, because an fts5 auxiliary function has to reconstruct the text it is marking
//! and a contentless table has nothing to reconstruct it from (chat-search-6eb.30).
//!
//! Neither route wins outright. Which is cheaper turns on whether the query carries a prefix
//! term, and [`spans_for`] holds that rule together with the measurements behind it. In short:
//! a prefix costs the corpus index a fresh vocabulary walk *per row* and costs a 150-message
//! scratch table nothing, while without one the index answers from postings it already has and
//! the scratch table is paying to build them a second time.
//!
//! Declaring `prefix='2 3 4'` on the fts tables would make that walk a doclist lookup and looks
//! like it should retire the branch. It does not, and it was measured rather than assumed
//! (chat-search-6eb.38, ADR 6): a prefix longer than the longest configured length still walks,
//! so the rule would become "is this prefix longer than the number in `schema.rs`" — the same
//! branch, now reading a declaration this module has no other reason to know about. At
//! `prefix='2 3 4'` the index route wins `pro`* 4.7 ms against 8.1 and loses `commit`* 11.0
//! against 2.4. It also costs the index 26% of its size and buys no wall clock here at all,
//! since the scratch table already answers a prefix in the same time.
//!
//! The scratch table costs ~86 µs to create, so it is built once per thread and reused; it does
//! not grow (`page_count` is flat at 63 after 40,000 calls). Its marginal cost is ~25 µs fixed
//! plus ~60 ns per byte inserted.
//!
//! What neither route escapes is the text. Saying which words of a message matched means
//! tokenizing that message, and at ~28 ns/byte a candidate set of 60–680 KB costs 2–18 ms
//! however it is asked. So marking is no longer why a keystroke is slow — at `limit 50 ×
//! MAX_HITS 3` it went from 10.7–29.1 ms to 2.4–17.7 ms — but it is not free, and it will not
//! become free while a snippet has to be anchored on a real match.

use rusqlite::{params, OptionalExtension};
use std::cell::RefCell;

/// Must stay identical to the tokenizer in `schema::DDL` — the entire premise here is that
/// this table and `fts_prose` reduce a word the same way. `tokenizer_matches_the_index`
/// fails if the two drift.
const TOKENIZER: &str = "porter unicode61 remove_diacritics 2";

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

/// Locate the matches for a whole result set, by whichever of the two routes fts5 answers
/// faster.
///
/// One answer, two ways of getting it. [`spans_indexed`] asks the corpus index, which already
/// holds every one of these rows; [`spans_many`] re-indexes them into a scratch table and asks
/// that. Same tokenizer, same expression, same `highlight()`, so they agree by construction —
/// `either_route_gives_the_answer_the_scratch_table_gives` is the standing check.
///
/// **A prefix term is what decides it**, and it decides it by two orders of magnitude. fts5
/// answers `"the"*` by walking its vocabulary for every term beginning with `the` and merging
/// their doclists, and it redoes that on every query — so against the 180k-message corpus index
/// it costs 5.4 ms *per row*, while against a scratch table holding only this result set the
/// vocabulary is 150 messages and the same expansion is free. With no prefix term there is
/// nothing to expand, and the position reverses: the index answers from postings it already
/// has, where the batch is paying to re-index text fts5 indexed at `cs index` time.
///
/// Measured over the candidate sets `search_grouped` really builds at `limit 50 × nested 3`,
/// against the 180k-message index (release):
///
/// | query | rows | KB | indexed | batched | per row, as now |
/// | --- | ---: | ---: | ---: | ---: | ---: |
/// | `borrow` typeahead | 69 | 678 | 7.7 ms | **17.7 ms** | 24.6 ms |
/// | `learning` typeahead | 150 | 377 | 17.2 ms | **14.6 ms** | 23.0 ms |
/// | `deep learning` typeahead | 132 | 371 | 17.3 ms | **11.8 ms** | 28.2 ms |
/// | `pro` typeahead | 150 | 192 | 361.8 ms | **6.3 ms** | 19.0 ms |
/// | `the` typeahead | 150 | 219 | 753.0 ms | **11.7 ms** | 23.2 ms |
/// | `commit` typeahead | 150 | 62 | 10.3 ms | **2.4 ms** | 12.6 ms |
/// | `borrow` exact | 69 | 678 | **7.3 ms** | 17.5 ms | 27.3 ms |
/// | `learning` exact | 150 | 389 | **6.9 ms** | 12.1 ms | 27.5 ms |
/// | `deep learning` exact | 132 | 374 | **7.8 ms** | 13.6 ms | 26.7 ms |
/// | `pro` exact | 133 | 432 | **5.3 ms** | 13.1 ms | 29.0 ms |
/// | `the` exact | 150 | 185 | **4.2 ms** | 9.5 ms | 19.4 ms |
/// | `commit` exact | 150 | 62 | **2.8 ms** | 3.0 ms | 10.7 ms |
///
/// Bold is what this picks, and `borrow` typeahead is the one it gets wrong — 678 KB over 69
/// messages, where the batch's insert costs more than the expansion it avoids. Deciding on the
/// bytes as well would need the texts measured before the route is chosen, and the rule is not
/// worth a second input for one query out of six.
///
/// What is left after this is the text itself. Every route has to tokenize each candidate to
/// say which of its words matched, and that floor is ~28 ns/byte — so a result set of 678 KB
/// cannot be marked in much under 18 ms by any of them (chat-search-6eb.30).
pub fn spans_for(
    conn: &rusqlite::Connection,
    field: crate::Field,
    rows: &[(i64, &str)],
    terms: &[String],
) -> Vec<Vec<Span>> {
    if rows.iter().all(|(_, t)| t.is_empty()) || terms.is_empty() {
        return rows.iter().map(|_| Vec::new()).collect();
    }
    if terms.iter().any(|t| t.ends_with('*')) {
        let texts: Vec<&str> = rows.iter().map(|&(_, text)| text).collect();
        return spans_many(&texts, terms);
    }
    rows.iter().map(|&(rowid, text)| spans_indexed(conn, field, rowid, text, terms)).collect()
}

/// [`spans`] for a row the index already holds, asked of the index itself.
///
/// Identical answer to [`spans`] — same tokenizer, same expression, same `highlight()` — for a
/// fraction of the cost, because the postings are already written. The scratch table has to
/// re-index the message before it can say anything about it; here fts5 walks a doclist it
/// built at `cs index` time.
///
/// Reach for [`spans_for`] rather than this: a prefix term makes this route the *slow* one, for
/// the reason set out there.
///
/// `text` is what `rowid` holds, and is needed for two things that are not the matching:
/// choosing delimiters that do not occur in it, and being the string the returned offsets
/// point into. It is not checked against the index — external content makes `message.text`
/// literally the content fts5 reads, so they are the same string unless the index is stale,
/// which is [`crate::index::ensure_current`]'s job rather than this one's.
///
/// An empty list means this row does not match, which is a real answer and not a failure to
/// look; the caller renders it the same way it renders [`spans`] finding nothing.
pub fn spans_indexed(
    conn: &rusqlite::Connection,
    field: crate::Field,
    rowid: i64,
    text: &str,
    terms: &[String],
) -> Vec<Span> {
    if terms.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let Some((open, close)) = sentinels(text) else {
        return Vec::new();
    };
    // The table name is interpolated because fts5 will not take an alias for `MATCH` or for an
    // auxiliary function's first argument. It comes from an enum, so there is nothing to
    // escape; everything a caller supplies is bound.
    let table = field.table();
    let sql = format!(
        "SELECT highlight({table}, 0, char(?1), char(?2)) FROM {table}
          WHERE rowid = ?3 AND {table} MATCH ?4"
    );
    let marked = conn.prepare_cached(&sql).and_then(|mut q| {
        q.query_row(params![open, close, rowid, any_expr(terms)], |r| r.get::<_, String>(0))
            .optional()
    });

    match marked {
        Ok(Some(m)) => marked_spans(&m, open, close),
        // No row means the row does not match the expression, which is the honest answer.
        Ok(None) => Vec::new(),
        // As in [`spans`]: the realistic error is a MATCH expression fts5 will not parse, and
        // "cannot locate the match" is exactly what that is. Empty is what callers already
        // render for it, and it never becomes a silent head-of-text.
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
    snippet_at(text, &spans(text, terms), width)
}

/// [`snippet`] for a caller that has already located the matches.
///
/// `marks` are byte offsets into `text` — one row of [`spans_for`]'s answer, in practice, which
/// is the point: locating a match and windowing around it are separate jobs, and only the first
/// of them needs a database.
pub fn snippet_at(text: &str, marks: &[Span], width: usize) -> (String, Vec<Span>) {
    // A snippet is one line, so the whitespace is collapsed first — and everything below
    // indexes the flattened text, since that is what the offsets have to point into.
    let (flat, hits) = flatten(text, marks);

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

/// `text` with each run of whitespace collapsed to one space, and `marks` — byte offsets into
/// `text` — re-expressed as offsets into the collapsed string.
///
/// Translated rather than re-located, because the marks come from the index and the index
/// holds the message as it was written, newlines and all. Re-running the match against the
/// flattened copy would be a second opinion about what matched, and the point of this module
/// is that there is only one.
///
/// An endpoint that lands inside a collapsed run maps to the end of the word before it — the
/// only place it can go once the run it pointed into is gone.
fn flatten(text: &str, marks: &[Span]) -> (String, Vec<Span>) {
    let mut flat = String::with_capacity(text.len());
    let mut moved: Vec<usize> = Vec::with_capacity(marks.len() * 2);
    // Endpoints ascend, so one walk of the text resolves all of them. `<=` rather than `==`
    // so an endpoint that somehow fell inside a character is still consumed here, where the
    // `debug_assert` below can see it, rather than silently pairing with the next span's.
    let mut ends = marks.iter().flat_map(|s| [s.start, s.end]).peekable();
    let mut pending_space = false;
    for (i, ch) in text.char_indices() {
        if ch.is_whitespace() {
            while ends.next_if(|&e| e <= i).is_some() {
                moved.push(flat.len());
            }
            // Nothing is emitted for leading whitespace, which is what `split_whitespace`
            // does and what the offsets below therefore have to agree with.
            pending_space = !flat.is_empty();
            continue;
        }
        // The separator goes in *before* the endpoints are resolved, so a mark that opens a
        // word points at the word and not at the space in front of it.
        if pending_space {
            flat.push(' ');
            pending_space = false;
        }
        while ends.next_if(|&e| e <= i).is_some() {
            moved.push(flat.len());
        }
        flat.push(ch);
    }
    for _ in ends {
        moved.push(flat.len()); // an endpoint at the very end of the text
    }

    debug_assert_eq!(moved.len(), marks.len() * 2, "a mark endpoint fell inside a character");
    let hits = moved
        .chunks_exact(2)
        .map(|p| Span { start: p[0], end: p[1] })
        // A mark covering only whitespace cannot survive the collapse, and an empty span would
        // read downstream as a match at that position.
        .filter(|s| s.start < s.end)
        .collect();
    (flat, hits)
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
    // Content-carrying, where `fts_prose` reads its content back out of `message`. Either
    // satisfies `highlight()`, which needs *some* content to reconstruct from; what this one
    // has that the index does not is any text at all, which is the whole reason it exists.
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
        crate::Query::exact(q).marking_terms()
    }

    /// The terms a query still being typed sends, whose final token is a prefix — which is what
    /// routes [`spans_for`] to the scratch table.
    fn typed(q: &str) -> Vec<String> {
        crate::Query::typeahead(q).marking_terms()
    }

    /// One prose message per text, in the real schema, with postings written the way the
    /// indexer writes them. Rowids come out 1..n in order.
    fn indexed(texts: &[&str]) -> rusqlite::Connection {
        let conn = crate::open(":memory:").unwrap();
        conn.execute("INSERT INTO conversation(id, source, native_id) VALUES ('c','codex','c')", [])
            .unwrap();
        for (i, text) in texts.iter().enumerate() {
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
                 VALUES (?1, 'c', 'main', ?2, 'user', 'prose', 1700000000000, ?3)",
                params![format!("m{i}"), i as i64, text],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            conn.execute("INSERT INTO fts_prose(rowid, text) VALUES (?1, ?2)", params![rowid, text])
                .unwrap();
        }
        conn
    }

    #[test]
    fn either_route_gives_the_answer_the_scratch_table_gives() {
        // `spans_for` picks between the corpus index and a scratch table on cost alone, so a
        // mark that depended on which was asked would make the same query mark different words
        // depending on how it was typed. Both ask fts5 with the same tokenizer and the same
        // expression, and this is what holds them to it.
        let texts = [
            "Commit current changes",
            "nothing of interest here",
            "the café was closed",
            "commit, commits and committing",
            "I learned something about\nthe borrow checker",
        ];
        let conn = indexed(&texts);
        let rows: Vec<(i64, &str)> =
            texts.iter().enumerate().map(|(i, t)| (i as i64 + 1, *t)).collect();

        for t in ["commits", "cafe", "learning", "elephant", "commit café", "borrow checker"]
            .into_iter()
            .map(terms)
            // A prefix term sends `spans_for` down the other branch, and has to arrive at the
            // same place.
            .chain(["comm", "lea", "caf", "borrow check"].into_iter().map(typed))
        {
            let alone: Vec<Vec<Span>> = texts.iter().map(|x| spans(x, &t)).collect();
            let via_index: Vec<Vec<Span>> = rows
                .iter()
                .map(|&(rowid, x)| spans_indexed(&conn, crate::Field::Prose, rowid, x, &t))
                .collect();
            assert_eq!(via_index, alone, "{t:?} through the index");
            assert_eq!(
                spans_for(&conn, crate::Field::Prose, &rows, &t),
                alone,
                "{t:?} through spans_for"
            );
        }
    }

    #[test]
    fn a_row_the_expression_does_not_match_is_marked_nothing_rather_than_guessed_at() {
        // The index route can only be asked about a row that is in the index, so the failure
        // mode it has to avoid is inventing a span for a row that did not match — which would
        // put a mark on a word the ranker never counted.
        let conn = indexed(&["the borrow checker", "nothing of interest"]);
        assert!(!spans_indexed(&conn, crate::Field::Prose, 1, "the borrow checker", &terms("borrow"))
            .is_empty());
        assert!(spans_indexed(&conn, crate::Field::Prose, 2, "nothing of interest", &terms("borrow"))
            .is_empty());
        // A rowid the table does not hold is the same answer, not a panic.
        assert!(spans_indexed(&conn, crate::Field::Prose, 99, "the borrow checker", &terms("borrow"))
            .is_empty());
        // Asked of the wrong index, where these rows have no postings at all.
        assert!(spans_indexed(&conn, crate::Field::Tools, 1, "the borrow checker", &terms("borrow"))
            .is_empty());
    }

    #[test]
    fn a_mark_taken_before_the_whitespace_collapse_still_points_at_its_word_after_it() {
        // The offsets come from an index holding the message as it was written; a snippet is one
        // line. Nothing else would notice if the translation between the two were short by the
        // bytes a collapsed run used to occupy.
        let text = "first line\n\n  second   café  line";
        let marks = spans(text, &terms("cafe"));
        assert_eq!(&text[marks[0].start..marks[0].end], "café");
        let (flat, moved) = flatten(text, &marks);
        assert_eq!(flat, "first line second café line");
        assert_eq!(&flat[moved[0].start..moved[0].end], "café");

        // A mark can straddle whitespace, because `highlight()` returns adjacent matching
        // tokens as one run — so an endpoint has to survive the run under it disappearing.
        let text = "commit\n\n\ncommits";
        let (flat, moved) = flatten(text, &spans(text, &terms("commit")));
        assert_eq!(flat, "commit commits");
        assert_eq!(moved.first().unwrap().start, 0, "the run still opens at the first word");
        assert_eq!(moved.last().unwrap().end, flat.len(), "and still closes at the last");

        // Leading and trailing whitespace is dropped entirely, which shifts everything left.
        let text = "\n\n  the café  \n";
        let (flat, moved) = flatten(text, &spans(text, &terms("cafe")));
        assert_eq!(flat, "the café");
        assert_eq!(&flat[moved[0].start..moved[0].end], "café");
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
