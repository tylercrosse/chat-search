//! One conversation as blocks — what a reader draws, and what a query hit inside it.
//!
//! Read from `message` rows, so `role`, `kind` and `on_head_path` are read rather than guessed
//! back out of prose. fast-resume cannot do this — it flattens messages into one string and
//! re-derives structure by sniffing sigils, which loses the role of every paragraph after the
//! first.
//!
//! Folding is the whole design (docs/TUI-DESIGN.md §8). Prose is 24% of this corpus;
//! `tool_call` and `tool_result` are 33% each and `reasoning` 8% (measured 2026-07-30 over
//! 168k messages). Rendering all of it verbatim is a file dump with a conversation buried in it.
//!
//! **This module holds the rules, never the rendering.** Which messages are drawn, which fold by
//! default, and which matches may claim to be the reason a conversation ranked are answers every
//! client has to give identically — the TUI, `cs show --json`, and anything reading that JSON.
//! How any of it *looks* — a sigil, a colour, a column width — belongs to the client, which is
//! why nothing here returns a style. The one rule that used to live on the wrong side of that
//! line ([`Block::mark_kind`], formerly returning a `ratatui::Style`) is the reason the line is
//! now stated.

use rusqlite::Connection;

use crate::highlight::{self, Span};

/// How much of every message is shown by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// Prose in full, everything else collapsed.
    Full,
    /// One line per message — the whole conversation as a map.
    Outline,
}

impl Density {
    /// How a message of this kind folds when the reader has not said otherwise.
    ///
    /// The *default* only. An explicit per-message fold always beats it, and holding that
    /// override is the client's job because it is session state, not a property of the
    /// conversation.
    pub fn default_fold(self, kind: &str) -> Fold {
        match (self, kind) {
            (Density::Outline, _) => Fold::Collapsed,
            (Density::Full, "prose") => Fold::Expanded,
            (Density::Full, _) => Fold::Collapsed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fold {
    Collapsed,
    Expanded,
}

/// Which claim a match inside a block is entitled to make.
///
/// Not a colour. A client maps this to whatever its medium can carry — the TUI spends a text
/// modifier on it so a monochrome terminal still tells them apart (§7 forbids encoding anything
/// in colour alone), and a GUI may well choose differently. Core states the claim; the client
/// decides how loudly to say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    /// This word is among the reasons the conversation ranked.
    Ranked,
    /// This word matched here but carries no postings, so `cs search` will not return the
    /// conversation for it and `cs explain` reports zero hits.
    Unranked,
}

/// One message of a conversation, with any matches in it already located.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Block {
    pub msg_id: String,
    pub role: String,
    pub kind: String,
    pub seq: i64,
    pub on_path: bool,
    pub text: String,
    /// A subagent strand. Worth marking rather than hiding: it is part of what happened, but
    /// it is not the conversation you were having.
    pub is_sidechain: bool,
    /// This result reports a failure. Set by the importer from the source's own signal.
    pub is_error: bool,
    pub thread_key: String,
    /// Byte offsets into `text` of the terms this message matched, from the same matcher that
    /// ranked it ([`crate::highlight`]).
    ///
    /// **UTF-8 byte offsets**, which is what Rust indexes with and what a JSON client must
    /// convert from: a JavaScript string is UTF-16 and Swift's `String` is not integer-indexed
    /// at all. Emitted in the source encoding rather than pre-converted for one client, because
    /// core cannot know which client is reading and a silently re-based offset is worse than one
    /// that has to be converted deliberately.
    ///
    /// Held on the block rather than recomputed at render time because locating them means
    /// tokenizing the message — ~25 µs fixed plus ~60 ns per byte through the scratch table —
    /// and a renderer runs on every frame, so marking there would pay that on every wheel notch
    /// and every keystroke. Computed once per `(conversation, query)` in [`load`], which is §8's
    /// "build once, not per frame" with the cache key made structural: the whole set is thrown
    /// away when either half of the key changes.
    pub marks: Vec<Span>,
}

impl Block {
    /// Whether this message is drawn at all.
    ///
    /// A *successful* `tool_result` is omitted rather than collapsed: the result is a blob
    /// whose existence the call already implies, so a line reading `↳ 1.2 KB` spends a row
    /// repeating what `⚙ Read(schema.rs)` just said.
    ///
    /// A failed one is kept, because "the tool broke here" is recognition information in a
    /// way a successful result is not — it is often the thing that makes a conversation the
    /// one you were looking for. `Message::is_error` comes from each source's own signal
    /// (Claude Code's `is_error`, Codex's `metadata.exit_code`), never from the text.
    pub fn drawn(&self) -> bool {
        self.kind != "tool_result" || self.is_error
    }

    /// Which claim a match inside this block is entitled to.
    ///
    /// Everything drawn gets marked, because marking runs over a scratch table that indexes
    /// whatever text it is handed and knows nothing about the corpus index. For most kinds those
    /// two agree. For one they do not: a `reasoning` block carries no postings
    /// ([`crate::Kind::is_indexed`]), so a word highlighted in it is a word `cs search` will not
    /// find. Marking that exactly like a prose hit states that the match is why the conversation
    /// is on screen, which it cannot be.
    ///
    /// Derived from `Kind::ALL` rather than testing for `"reasoning"`, so this answer follows
    /// the indexing rule instead of restating a snapshot of it (chat-search-8mb).
    pub fn mark_kind(&self) -> MarkKind {
        let unranked =
            crate::Kind::ALL.iter().any(|k| !k.is_indexed() && k.as_str() == self.kind);
        if unranked { MarkKind::Unranked } else { MarkKind::Ranked }
    }
}

/// Read one conversation's messages, with `terms` located in each.
///
/// Head path only. A message edited away is still searchable and still indexed, but returning it
/// inline without saying so would present an abandoned branch as the conversation — see the
/// off-path toggle in §8, which is not built yet.
///
/// `terms` come from [`crate::Query::marking_terms`] — not from splitting a query here, so a
/// caller marks exactly what the ranker matched, trailing prefix star included. An empty slice
/// marks nothing, which is the honest state for a query that was never run.
pub fn load(conn: &Connection, conv_id: &str, terms: &[String]) -> rusqlite::Result<Vec<Block>> {
    // `seq` is per *thread*, not per conversation — ADR 4 makes a conversation a DAG and
    // `thread_key` carries which strand a message belongs to. Ordering by `seq` alone therefore
    // interleaves every thread at once: on a real 9-thread conversation it put all nine opening
    // user turns first, then all nine first replies, which reads as nonsense. Main thread before
    // sidechains, each strand contiguous. `idx_message_thread` is (conv_id, thread_key, seq),
    // which exists for exactly this.
    let mut stmt = conn.prepare_cached(
        "SELECT id, role, kind, seq, on_head_path, text, is_sidechain, thread_key, is_error
         FROM message WHERE conv_id = ?1 AND on_head_path = 1
         ORDER BY is_sidechain, thread_key, seq",
    )?;
    let mut blocks = stmt
        .query_map(rusqlite::params![conv_id], |r| {
            Ok(Block {
                msg_id: r.get(0)?,
                role: r.get(1)?,
                kind: r.get(2)?,
                seq: r.get(3)?,
                on_path: r.get::<_, i64>(4)? != 0,
                text: r.get(5)?,
                is_sidechain: r.get::<_, i64>(6)? != 0,
                thread_key: r.get(7)?,
                is_error: r.get::<_, i64>(8)? != 0,
                marks: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Only what is drawn: a successful `tool_result` is never rendered, so locating its matches
    // would buy nothing, and this corpus is a third such messages by count.
    //
    // Deliberately *not* asked of the corpus index, even though `fts_prose` now holds postings
    // for every one of these rows and could mark them without an insert (chat-search-6eb.30).
    // The TUI's final token is always a prefix (`the*`, `commits*`), and fts5 answers a prefix by
    // walking its vocabulary for every term that starts that way — per query, across all 180k
    // messages. Against a scratch table holding this one conversation that expansion is free,
    // which is why `spans_many` stays the right call here and why `highlight::spans_for` routes a
    // prefix the same way.
    //
    // Measured on the corpus's longest conversation (1,479 drawn blocks, 942 KB): marking
    // everything through the scratch table is a flat 110–126 ms whatever the query, while asking
    // the index which messages match first is 64 ms for `borrow checker`, 242 ms for `commits`
    // and 17.9 *seconds* for `the`. Predictable beats occasionally faster.
    if !terms.is_empty() {
        // One trip through the highlighter for the whole conversation. Marking cost is per
        // *call*, not per byte — the same 937 KB took 224 ms as 1,468 calls and 53 ms as one —
        // so the number of messages, not their size, was what made opening a long conversation
        // slow.
        let marks = {
            let drawn: Vec<&str> =
                blocks.iter().filter(|b| b.drawn()).map(|b| b.text.as_str()).collect();
            highlight::spans_many(&drawn, terms)
        };
        for (block, found) in blocks.iter_mut().filter(|b| b.drawn()).zip(marks) {
            block.marks = found;
        }
    }
    Ok(blocks)
}

/// Distinct strands in this conversation.
pub fn thread_count(blocks: &[Block]) -> usize {
    let mut keys: Vec<&str> = blocks.iter().map(|b| b.thread_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.len()
}

/// What `cs show --json` emits (ADR 12).
///
/// Lives here rather than in the CLI because it *is* the contract, and a contract assembled at
/// the print site is one nothing can test. Every field is either read from the index or derived
/// by a rule in this module — nothing on the wire is computed twice.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Transcript {
    /// Contract version. Bumped when a field changes meaning, which is the change a client
    /// cannot detect any other way — a rename breaks loudly and a new field is ignored, but a
    /// field that quietly starts meaning something else breaks silently.
    pub v: u32,
    pub conv_id: String,
    /// What the blocks were marked against, after the query grammar had its say. Emitted so a
    /// client can show *why* something is highlighted without re-parsing the query.
    pub terms: Vec<String>,
    /// Distinct strands (ADR 4). More than one means this conversation is a DAG and the
    /// messages arrive main-thread-first, each strand contiguous.
    pub threads: usize,
    /// Messages on the head path — the length of `messages`.
    pub count: usize,
    /// How many of those a reader draws. The difference is successful tool results.
    pub drawn: usize,
    /// How to read `marks`. `"utf8-bytes"` today; a client indexing UTF-16 must convert.
    /// Named on the wire rather than only in documentation because it is the one place the
    /// reader's language leaks into the contract, and a silently wrong offset highlights the
    /// wrong word rather than failing.
    pub mark_offsets: &'static str,
    pub messages: Vec<WireBlock>,
}

/// One block on the wire: everything stored, plus the two answers core owes a client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WireBlock {
    #[serde(flatten)]
    pub block: Block,
    /// [`Block::drawn`], answered here so no client re-derives it. It was derived twice
    /// already — once in Rust and once in the interface prototype's JavaScript, both citing
    /// §8 — which is what this whole module exists to stop.
    pub drawn: bool,
    /// [`Block::mark_kind`], same reason.
    pub mark_kind: MarkKind,
}

impl Transcript {
    pub fn of(conv_id: &str, terms: &[String], blocks: Vec<Block>) -> Self {
        Transcript {
            v: 1,
            conv_id: conv_id.to_string(),
            terms: terms.to_vec(),
            threads: thread_count(&blocks),
            count: blocks.len(),
            drawn: blocks.iter().filter(|b| b.drawn()).count(),
            mark_offsets: "utf8-bytes",
            messages: blocks
                .into_iter()
                .map(|block| WireBlock {
                    drawn: block.drawn(),
                    mark_kind: block.mark_kind(),
                    block,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: &str, role: &str, id: &str, text: &str) -> Block {
        Block {
            msg_id: id.into(),
            role: role.into(),
            kind: kind.into(),
            seq: 0,
            on_path: true,
            text: text.into(),
            is_sidechain: false,
            thread_key: "main".into(),
            is_error: false,
            marks: Vec::new(),
        }
    }

    #[test]
    fn a_successful_tool_result_is_not_drawn_and_a_failed_one_is() {
        // The whole reason `is_error` exists. A successful result is implied by its call and
        // costs a row to repeat; a failed one is often what makes a conversation findable.
        let mut ok = block("tool_result", "tool", "a", "3 files");
        let mut bad = block("tool_result", "tool", "b", "error: no such file or directory");
        ok.is_error = false;
        bad.is_error = true;
        assert!(!ok.drawn(), "the result is not a row, not even a collapsed one");
        assert!(bad.drawn(), "the failure stays");
        assert!(block("prose", "assistant", "c", "here is what I found").drawn());
    }

    #[test]
    fn full_density_expands_prose_and_collapses_the_rest() {
        assert_eq!(Density::Full.default_fold("prose"), Fold::Expanded);
        assert_eq!(Density::Full.default_fold("reasoning"), Fold::Collapsed);
        assert_eq!(Density::Full.default_fold("tool_call"), Fold::Collapsed);
    }

    #[test]
    fn outline_collapses_everything_including_prose() {
        for kind in ["prose", "reasoning", "tool_call", "tool_result"] {
            assert_eq!(Density::Outline.default_fold(kind), Fold::Collapsed, "{kind}");
        }
    }

    #[test]
    fn which_kinds_mark_quietly_follows_the_indexing_rule_rather_than_a_copy_of_it() {
        // If `Kind::is_indexed` changes its mind, this changes with it. A `== "reasoning"` test
        // here would be a second copy of that rule, quietly drifting from the first.
        for kind in crate::Kind::ALL {
            let b = block(kind.as_str(), "assistant", "a", "the borrow rules");
            let expected =
                if kind.is_indexed() { MarkKind::Ranked } else { MarkKind::Unranked };
            assert_eq!(b.mark_kind(), expected, "{kind:?}");
        }
    }

    #[test]
    fn threads_are_counted_by_strand_not_by_message() {
        // Found by running it against a real 9-thread conversation: `seq` restarts at 0 per
        // thread, so ordering by it alone put all nine opening user turns first and all nine
        // first replies after them. The SQL does the ordering; this pins the count the header
        // reports off the same field.
        let b = |thread: &str, side: bool, seq: i64, id: &str| Block {
            seq,
            is_sidechain: side,
            thread_key: thread.into(),
            ..block("prose", "user", id, "x")
        };
        let blocks = vec![
            b("main", false, 0, "m0"),
            b("main", false, 1, "m1"),
            b("sub-a", true, 0, "a0"),
            b("sub-a", true, 1, "a1"),
        ];
        assert_eq!(thread_count(&blocks), 2);
        assert_eq!(thread_count(&[]), 0);
    }

    #[test]
    fn a_block_serialises_without_dragging_any_presentation_with_it() {
        // The point of the move, asserted rather than assumed. If a style, a width or a colour
        // ever creeps back onto `Block`, this is where it surfaces — every field here has to be
        // something a client in any language can act on.
        let mut b = block("prose", "user", "m1", "the borrow checker");
        b.marks = vec![Span { start: 4, end: 10 }];
        let v = serde_json::to_value(&b).expect("a block is plain data");
        let obj = v.as_object().expect("an object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "is_error",
                "is_sidechain",
                "kind",
                "marks",
                "msg_id",
                "on_path",
                "role",
                "seq",
                "text",
                "thread_key"
            ]
        );
        assert_eq!(obj["marks"], serde_json::json!([{ "start": 4, "end": 10 }]));
    }

    #[test]
    fn the_wire_form_answers_drawn_and_mark_kind_so_no_client_re_derives_them() {
        // The reason the JSON is worth emitting at all. A client that has to work out `drawn`
        // from `kind` and `is_error` is a second copy of §8's rule, which is the duplication
        // this module was created to delete — so both answers travel with the message.
        let mut ok = block("tool_result", "tool", "r", "3 files");
        ok.is_error = false;
        let t = Transcript::of(
            "claude-code:abc",
            &["borrow".to_string()],
            vec![block("prose", "user", "a", "the borrow checker"), ok, block("reasoning", "assistant", "b", "hm")],
        );
        assert_eq!((t.count, t.drawn, t.threads, t.v), (3, 2, 1, 1));
        assert_eq!(t.mark_offsets, "utf8-bytes");

        let v = serde_json::to_value(&t).expect("the transcript is plain data");
        let msgs = v["messages"].as_array().expect("an array");
        assert_eq!(msgs[0]["drawn"], true);
        assert_eq!(msgs[0]["mark_kind"], "ranked");
        assert_eq!(msgs[1]["drawn"], false, "a successful result travels, flagged as undrawn");
        assert_eq!(msgs[2]["mark_kind"], "unranked", "reasoning carries no postings");
        // Flattened, so a client reads one object per message rather than unwrapping a nesting
        // that exists only because Rust needed somewhere to put two computed fields.
        assert_eq!(msgs[0]["kind"], "prose");
        assert_eq!(msgs[0]["msg_id"], "a");
    }

    #[test]
    fn marks_are_byte_offsets_into_the_text_as_emitted() {
        // The one place the client's language leaks into the contract: these are UTF-8 byte
        // offsets, and a client indexing UTF-16 has to convert. Pinned with a multi-byte string
        // so a change to character offsets fails here rather than silently mis-highlighting.
        let text = "héllo borrow wörld";
        let mut b = block("prose", "user", "m1", text);
        b.marks = highlight::spans(text, &["borrow".to_string()]);
        let m = b.marks.first().expect("the fixture has to actually match");
        assert_eq!(&text[m.start..m.end], "borrow");
        assert_eq!(m.start, 7, "byte offsets: the accented character is two bytes");
    }
}
