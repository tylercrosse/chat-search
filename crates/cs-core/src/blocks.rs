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

/// One fold per [`Band`] — the whole of what a reader sees before they touch anything.
///
/// The vocabulary; [`Density`] is now two named points in it. Keyed on band rather than on
/// [`crate::Kind`] because a user turn and an assistant turn are both `prose`, so no map over
/// kinds can say *the question stays open and the answer folds down*. That is the fidelity the
/// interface prototype has had for some time and the one every client has asked for since
/// (chat-search-me9.41).
///
/// Band is the right key for the same reason it is the right key everywhere else it is already
/// load-bearing: the reader's minimap encodes band as width, and the theme fence spaces the four
/// on a luminance ramp. A fifth axis of fidelity would be a distinction none of those can draw.
///
/// Two levels, not three. Hiding a band outright is a real setting the prototype carries, and it
/// arrives with the per-band controls that make these knobs reachable — chat-search-me9.8.36.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Folds {
    pub user: Fold,
    pub agent: Fold,
    pub reasoning: Fold,
    pub tool: Fold,
}

impl Folds {
    /// The fold this map gives that band.
    pub fn of(self, band: Band) -> Fold {
        match band {
            Band::User => self.user,
            Band::Agent => self.agent,
            Band::Reasoning => self.reasoning,
            Band::Tool => self.tool,
        }
    }
}

/// How much of every message is shown by default — a named point in [`Folds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    /// Prose in full, everything else collapsed.
    Full,
    /// One line per message — the whole conversation as a map.
    Outline,
}

impl Density {
    /// This preset written out per band.
    ///
    /// Public because a client porting the fidelity model needs four knobs to start from and
    /// then move independently. A preset it can only interrogate one band at a time is one it
    /// has to reassemble by asking four questions and hoping it asked all of them.
    pub fn folds(self) -> Folds {
        match self {
            // Both sides of the prose, because at this density the conversation is the thing
            // being read. That the two are now separately expressible is what the map buys; it
            // is not an invitation for this preset to start using it.
            Density::Full => Folds {
                user: Fold::Expanded,
                agent: Fold::Expanded,
                reasoning: Fold::Collapsed,
                tool: Fold::Collapsed,
            },
            Density::Outline => Folds {
                user: Fold::Collapsed,
                agent: Fold::Collapsed,
                reasoning: Fold::Collapsed,
                tool: Fold::Collapsed,
            },
        }
    }

    /// How a message in this band folds when the reader has not said otherwise.
    ///
    /// The *default* only. An explicit per-message fold always beats it, and holding that
    /// override is the client's job because it is session state, not a property of the
    /// conversation.
    pub fn default_fold(self, band: Band) -> Fold {
        self.folds().of(band)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Fold {
    Collapsed,
    Expanded,
}

/// The order a conversation is read in, and so the order every client draws it in.
///
/// `seq` is per *thread*, not per conversation — ADR 4 makes a conversation a DAG and
/// `thread_key` carries which strand a message belongs to. Ordering by `seq` alone therefore
/// interleaves every thread at once: on a real 9-thread conversation it put all nine opening
/// user turns first, then all nine first replies, which reads as nonsense. Main thread before
/// sidechains, each strand contiguous. `idx_message_reading` is this order, and exists for
/// exactly this — the index it replaced claimed the same and could not deliver it, being
/// (conv_id, thread_key, seq) against a sort that leads with `is_sidechain`.
///
/// A shared fragment rather than a comment on one query, because two of them now have to
/// produce the *same* order: [`load`], and the shape `cs search` puts on every row
/// ([`crate::Group::kind_runs`]). A strip built in one order beside a transcript built in the
/// other lines up only by coincidence, and the coincidence holds right up until a conversation
/// has a subagent in it.
pub const READING_ORDER: &str = "ORDER BY is_sidechain, thread_key, seq";

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

/// What a message is, at the resolution a conversation's *shape* is drawn at.
///
/// Coarser than [`crate::Kind`] in one place and finer in another, and both departures are the
/// point. A call and its result merge, because at a strip's resolution they are one stretch of
/// the same traffic — and that traffic is 66–85% of this corpus by message, so keeping them
/// apart would spend the reader's whole strip on the distinction. Prose splits by role,
/// because "you asked" against "it answered" is the one boundary a reader triages on: it is
/// what makes a 900-message agent session legible as a handful of things you asked for rather
/// than as an undifferentiated wall.
///
/// Four is also what fits. The strip is ~200px on the interface prototype and a text cell in
/// the TUI, so a fifth category would be a stripe too thin to see and a legend entry nobody
/// reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    /// Prose from the person. One message, and usually the reason for everything after it.
    User,
    /// Prose from anyone else — the assistant, and the four `system` messages in this corpus,
    /// which are not worth a band of their own.
    Agent,
    /// Thinking. Drawn, never searched: [`crate::Kind::is_indexed`] is why.
    Reasoning,
    /// Calls and their results, failures included.
    Tool,
}

impl Band {
    /// Every variant, so a rule expressed over bands can be *derived* by a caller rather than
    /// restated — the same reason [`crate::Kind::ALL`] exists. [`Folds`] is the first such rule,
    /// and a band it forgot would be a band silently taking someone else's default.
    pub const ALL: [Band; 4] = [Band::User, Band::Agent, Band::Reasoning, Band::Tool];
}

/// Which band a message belongs to.
///
/// Takes the two columns rather than a [`Block`], so `cs search` can answer it for a whole
/// page of conversations without reading their text — the query behind
/// [`crate::Group::kind_runs`] selects three narrow columns, and a `Block` would drag every
/// message body along with them.
///
/// Resolved through [`crate::Kind`] rather than matched as strings, so adding a kind breaks
/// this arm-by-arm instead of quietly defaulting into one of the four (same reason as
/// [`Block::mark_kind`]).
pub fn band(role: &str, kind: &str) -> Band {
    match crate::Kind::ALL.into_iter().find(|k| k.as_str() == kind) {
        Some(crate::Kind::Prose) if role == crate::Role::User.as_str() => Band::User,
        Some(crate::Kind::Prose) => Band::Agent,
        Some(crate::Kind::Reasoning) => Band::Reasoning,
        Some(crate::Kind::ToolCall | crate::Kind::ToolResult) => Band::Tool,
        // A kind no importer writes. It came from somewhere, and something on the far side of
        // the conversation said it, so it reads as the agent rather than disappearing.
        None => Band::Agent,
    }
}

/// Whether a message of this kind is drawn at all — [`Block::drawn`], for a caller holding the
/// two columns it depends on rather than a whole [`Block`].
///
/// The rule lives here and the method delegates, so `cs show` and the shape on a search row
/// cannot come to different conclusions about what a conversation is made of.
pub fn drawn(kind: &str, is_error: bool) -> bool {
    kind != crate::Kind::ToolResult.as_str() || is_error
}

/// A stretch of one band, `["tool", 12]` on the wire.
///
/// A tuple rather than an object because there are a great many of these — a 2,553-message
/// conversation is the corpus's longest and the list is per result row — and `{"band":"tool",
/// "n":12}` is four times the bytes for the same two facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Run(pub Band, pub usize);

/// Run-length encode a conversation's bands, in the order they were read in.
///
/// Compresses hard on exactly the conversations that need it: tool traffic arrives in long
/// stretches, so the agent sessions with hundreds of messages are the ones whose shape is a
/// few dozen runs. Short chatty conversations barely compress, and they are short.
pub fn runs(bands: impl IntoIterator<Item = Band>) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for band in bands {
        match out.last_mut() {
            Some(Run(last, n)) if *last == band => *n += 1,
            _ => out.push(Run(band, 1)),
        }
    }
    out
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
        drawn(&self.kind, self.is_error)
    }

    /// Which band this message is drawn in — [`band`].
    pub fn band(&self) -> Band {
        band(&self.role, &self.kind)
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
/// **The whole sitting when the id is part of one** (chat-search-o1i.8). Google Takeout shredded
/// a chat into one record per turn, and a reader should not have to know which upstream tool
/// happened to serialize a conversation in one piece — so the records come back concatenated,
/// as the one transcript every other source already hands over. The seam is not erased: it is
/// still `Sitting.members` on whatever answer carries this, and every block still names the
/// record it came from in `msg_id`. What it is not is structural.
///
/// Head path only. A message edited away is still searchable and still indexed, but returning it
/// inline without saying so would present an abandoned branch as the conversation — see the
/// off-path toggle in §8, which is not built yet.
///
/// `terms` come from [`crate::Query::marking_terms`] — not from splitting a query here, so a
/// caller marks exactly what the ranker matched, trailing prefix star included. An empty slice
/// marks nothing, which is the honest state for a query that was never run.
pub fn load(conn: &Connection, conv_id: &str, terms: &[String]) -> rusqlite::Result<Vec<Block>> {
    crate::sittings::ensure(conn)?;
    let mut blocks = Vec::new();
    for record in crate::sittings::resolve(conn, conv_id)? {
        blocks.append(&mut read_record(conn, &record)?);
    }

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

/// The transcript read, named so the test that pins its plan reads the statement
/// [`read_record`] runs rather than a copy of it.
///
/// Unlike the shape's query this one selects `text`, so it can never be index-only — what
/// `idx_message_reading` buys here is the ordering, which is otherwise a temp b-tree over
/// every message of the conversation before the first one comes back. The `LEFT JOIN` onto
/// the sitting table arrived after that was measured and does not cost it: the join is on a
/// temp table keyed by `conv_id` and the ordering still comes from the index, which is what
/// the test in [`crate::schema`] is there to keep true.
pub(crate) fn read_record_sql() -> String {
    format!(
        "SELECT m.id, m.role, m.kind, {position}, m.on_head_path, m.text, m.is_sidechain,
                m.thread_key, m.is_error
         FROM message m
         {join}
         WHERE m.conv_id = ?1 AND m.on_head_path = 1
         {READING_ORDER}",
        position = crate::sittings::POSITION,
        join = crate::sittings::OF_MESSAGE,
    )
}

/// One record's messages, unmarked, in [`READING_ORDER`].
///
/// Per record and concatenated by the caller rather than widened into one `IN (...)` query, for
/// the reason [`crate::search`]'s `fill_shape` does the same with bands: the order messages are
/// read in lives in `READING_ORDER`, and an `ORDER BY` that had to interleave the fold as well
/// would be that rule written down twice. The concatenation order is the sitting's, which is
/// what [`crate::sittings::resolve`] already returns.
///
/// `seq` is [`crate::sittings::POSITION`] — the message's place in the *row*, counting from the
/// start of the sitting. Without it the second record's turns would land on 0 and 1 again, and a
/// client lining `Group.match_seqs` up against this transcript would mark the wrong messages.
/// The `LEFT JOIN` finds nothing for an ordinary conversation, so that expression is `seq + 0`
/// and nothing outside the Takeout records moves.
fn read_record(conn: &Connection, conv_id: &str) -> rusqlite::Result<Vec<Block>> {
    let mut stmt = conn.prepare_cached(&read_record_sql())?;
    let blocks = stmt
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
    /// The conversation this is a transcript of — the record that *opened* it when `sitting` is
    /// set, which is the same id `cs search` puts on the row.
    ///
    /// So it is not always the id that was asked for: `cs show` on any member of a sitting
    /// answers for the whole sitting, and echoing the argument back would name one record of
    /// the transcript as though it were all of it. A sitting acquires no id of its own (ADR 16)
    /// and the opener's is real and permanent.
    pub conv_id: String,
    /// What the blocks were marked against, after the query grammar had its say. Emitted so a
    /// client can show *why* something is highlighted without re-parsing the query.
    pub terms: Vec<String>,
    /// Set when these messages are several activity-log records read back as one chat.
    ///
    /// The seam kept as data rather than drawn: those records were separate HTTP requests with
    /// no shared context on Google's side, so the boundary is real information, and a client
    /// that wants to say "31 records, 30-minute gap" has it. A reader who does not care sees
    /// one continuous conversation, which is the point of the fold.
    pub sitting: Option<crate::sittings::Sitting>,
    /// Distinct strands (ADR 4). More than one means this conversation is a DAG and the
    /// messages arrive main-thread-first, each strand contiguous.
    pub threads: usize,
    /// Messages on the head path — the length of `messages`.
    pub count: usize,
    /// How many of those a reader draws. The difference is successful tool results.
    pub drawn: usize,
    /// How to read `marks` — [`highlight::OFFSETS`], which is where the reasoning lives and
    /// which `cs search --json` reads the same answer out of.
    pub mark_offsets: &'static str,
    pub messages: Vec<WireBlock>,
}

/// One block on the wire: everything stored, plus the four answers core owes a client.
///
/// The four are this module's opening paragraph read out as fields — which messages are drawn,
/// which band each sits in, which fold by default, and which matches may claim to be the reason
/// a conversation ranked. Each is a rule a client would otherwise restate, and each restatement
/// is a chance for two surfaces to draw different conversations out of the same bytes.
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
    /// [`Block::band`] — the same four names `cs search --json` already puts on a row's
    /// `kind_runs`, said per message because a reader draws bands where a strip draws runs.
    ///
    /// A client cannot spell [`band`] for itself without also owning the decisions that make it
    /// right: that `system` prose is the agent's side rather than a fifth stripe, and that a
    /// call and its result are one stretch of traffic. Both were already re-derived once, in the
    /// prototype's JavaScript.
    pub band: Band,
    /// How this block folds when the reader has not said otherwise —
    /// [`Density::default_fold`] at [`Density::Full`], which is the density a reader opens at.
    ///
    /// A default and never a state: an explicit per-message fold beats it, and holding that
    /// override is the client's job because it is session state rather than a property of the
    /// conversation. [`Density::Outline`] is not answered here — an outline row needs the
    /// collapsed *form* as well as the fold, and that is chat-search-me9.20.
    pub fold: Fold,
}

impl Transcript {
    /// Everything `cs show --json` says about an id, read as one conversation.
    ///
    /// The entry point a client goes through, so that resolving the id, loading the messages and
    /// naming what came back are one decision rather than three a caller has to sequence
    /// correctly. `cs show` used to make the last of them itself, which is how the id it echoed
    /// stayed right while the transcript under it was one record of thirty-one.
    ///
    /// Empty when there is no such conversation. Telling that from a conversation with nothing
    /// on its head path is the caller's job — both are `count: 0` here, and only the caller
    /// knows whether that deserves a nonzero exit.
    pub fn read(conn: &Connection, conv_id: &str, terms: &[String]) -> rusqlite::Result<Self> {
        let blocks = load(conn, conv_id, terms)?;
        let sitting = crate::sittings::Sitting::of(conn, conv_id)?;
        Ok(Transcript::of(conv_id, terms, blocks, sitting))
    }

    /// The same thing from parts already in hand, for a caller that read the blocks itself.
    pub fn of(
        conv_id: &str,
        terms: &[String],
        blocks: Vec<Block>,
        sitting: Option<crate::sittings::Sitting>,
    ) -> Self {
        // Named by the opener rather than by whatever was asked for — see `conv_id`. `members`
        // is never empty for a `Some`, because `Sitting::of` only builds one from two or more.
        let conv_id = sitting.as_ref().map_or(conv_id, |s| s.members[0].as_str());
        Transcript {
            v: 1,
            conv_id: conv_id.to_string(),
            terms: terms.to_vec(),
            sitting,
            threads: thread_count(&blocks),
            count: blocks.len(),
            drawn: blocks.iter().filter(|b| b.drawn()).count(),
            mark_offsets: highlight::OFFSETS,
            messages: blocks
                .into_iter()
                .map(|block| WireBlock {
                    drawn: block.drawn(),
                    mark_kind: block.mark_kind(),
                    band: block.band(),
                    fold: Density::Full.default_fold(block.band()),
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
    fn full_density_expands_both_sides_of_the_prose_and_collapses_the_rest() {
        // Unchanged from when this was keyed on `kind`, and that is the point: re-keying the
        // default on band is a change to what *can* be said, not to what the presets say.
        assert_eq!(Density::Full.default_fold(Band::User), Fold::Expanded);
        assert_eq!(Density::Full.default_fold(Band::Agent), Fold::Expanded);
        assert_eq!(Density::Full.default_fold(Band::Reasoning), Fold::Collapsed);
        assert_eq!(Density::Full.default_fold(Band::Tool), Fold::Collapsed);
    }

    #[test]
    fn outline_collapses_every_band_including_both_prose_ones() {
        for band in Band::ALL {
            assert_eq!(Density::Outline.default_fold(band), Fold::Collapsed, "{band:?}");
        }
    }

    #[test]
    fn a_default_fold_can_differ_between_the_two_prose_bands() {
        // The thing the old signature made unsayable. `default_fold(kind)` saw one `"prose"`
        // where a reader sees two, so `{ user: expanded, agent: collapsed }` — the fidelity the
        // interface prototype opens at — could not be expressed at all, whatever value of
        // `Density` it was asked for.
        let asked = Folds {
            user: Fold::Expanded,
            agent: Fold::Collapsed,
            reasoning: Fold::Collapsed,
            tool: Fold::Collapsed,
        };
        let question = block("prose", "user", "a", "why is this slow");
        let answer = block("prose", "assistant", "b", "let me look");
        assert_eq!(question.kind, answer.kind, "the two are one kind, which was the problem");
        assert_eq!(asked.of(question.band()), Fold::Expanded);
        assert_eq!(asked.of(answer.band()), Fold::Collapsed);
    }

    #[test]
    fn each_band_reads_its_own_knob_and_no_one_elses() {
        // Four fields behind one `match` is where a copy-paste goes unnoticed: `Band::Agent =>
        // self.user` type-checks, and every preset in this file happens to give those two the
        // same answer, so nothing else here would catch it. Raising one band at a time is what
        // makes the wiring visible when there are only two folds to tell the fields apart with.
        let collapsed = Density::Outline.folds();
        for raised in Band::ALL {
            let mut folds = collapsed;
            match raised {
                Band::User => folds.user = Fold::Expanded,
                Band::Agent => folds.agent = Fold::Expanded,
                Band::Reasoning => folds.reasoning = Fold::Expanded,
                Band::Tool => folds.tool = Fold::Expanded,
            }
            for band in Band::ALL {
                let expected = if band == raised { Fold::Expanded } else { Fold::Collapsed };
                assert_eq!(folds.of(band), expected, "raised {raised:?}, asked {band:?}");
            }
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
    fn a_band_says_who_was_talking_where_the_kind_alone_cannot() {
        // The one place the shape is finer than `Kind`: a prose message is the person or it is
        // the machine, and on an agent session that is the only boundary a reader has.
        assert_eq!(band("user", "prose"), Band::User);
        assert_eq!(band("assistant", "prose"), Band::Agent);
        // And the one place it is coarser: a call and its result are one stretch of traffic.
        assert_eq!(band("assistant", "tool_call"), Band::Tool);
        assert_eq!(band("tool", "tool_result"), Band::Tool);
        assert_eq!(band("assistant", "reasoning"), Band::Reasoning);
        // Four `system` prose messages exist in this corpus. They are the machine's side of
        // the conversation, not a fifth stripe.
        assert_eq!(band("system", "prose"), Band::Agent);
    }

    #[test]
    fn every_kind_lands_in_a_band_so_none_can_vanish_from_the_shape() {
        // A kind with nowhere to go would be a message the strip silently drops, which is the
        // failure the run lengths cannot report: the shape would still look plausible.
        for kind in crate::Kind::ALL {
            for role in ["user", "assistant", "tool", "system"] {
                let b = band(role, kind.as_str());
                assert_eq!(b, block(kind.as_str(), role, "a", "x").band(), "{kind:?}/{role}");
            }
        }
    }

    #[test]
    fn runs_collapse_neighbours_and_keep_the_order_they_arrived_in() {
        use Band::{Agent, Tool, User};
        let bands = [User, Agent, Tool, Tool, Tool, Agent, Tool];
        assert_eq!(
            runs(bands),
            [Run(User, 1), Run(Agent, 1), Run(Tool, 3), Run(Agent, 1), Run(Tool, 1)]
        );
        // The same band twice with something between it is two runs, not one — a strip that
        // merged them would move every position after it.
        assert_eq!(runs(bands).iter().map(|Run(_, n)| n).sum::<usize>(), bands.len());
        assert!(runs([]).is_empty(), "nothing to draw is no runs, not one empty run");
    }

    #[test]
    fn a_run_is_a_pair_on_the_wire_rather_than_an_object() {
        // A published shape (ADR 12): the interface prototype draws one strip per result row,
        // and at 354 rows the difference between `["tool",12]` and a named object is the
        // difference between a payload a client streams and one it waits for.
        let v = serde_json::to_value(runs([Band::User, Band::Tool, Band::Tool])).unwrap();
        assert_eq!(v, serde_json::json!([["user", 1], ["tool", 2]]));
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
            None,
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
    fn the_wire_form_also_answers_band_and_fold_so_a_reader_derives_neither() {
        // The other two rules a client would otherwise restate. `band` is what colours the
        // gutter spine beside a message and `fold` is what makes a 900-message agent session
        // legible; a client that worked either out for itself would be a second copy of §8,
        // free to disagree with the TUI about what the same conversation looks like.
        let mut failed = block("tool_result", "tool", "r", "error: no such file");
        failed.is_error = true;
        let t = Transcript::of(
            "claude-code:abc",
            &[],
            vec![
                block("prose", "user", "a", "why is this slow"),
                block("prose", "assistant", "b", "let me look"),
                block("tool_call", "assistant", "c", "Bash\n{\"command\":\"ls\"}"),
                failed,
                block("reasoning", "assistant", "e", "**Planning**"),
            ],
            None,
        );
        let v = serde_json::to_value(&t).expect("the transcript is plain data");
        let msgs = v["messages"].as_array().expect("an array");
        // The same four names `kind_runs` already uses, so a client colours a strip and a spine
        // off one vocabulary rather than two that happen to agree today.
        let bands: Vec<&str> = msgs.iter().map(|m| m["band"].as_str().unwrap()).collect();
        assert_eq!(bands, ["user", "agent", "tool", "tool", "reasoning"]);
        // Both sides of the prose expand, and now because `Density::Full` says so per band
        // rather than because `"prose"` was one word. What travels is still one fold per
        // message: the map is core's way of stating the default, not a second thing the wire
        // has to carry.
        let folds: Vec<&str> = msgs.iter().map(|m| m["fold"].as_str().unwrap()).collect();
        assert_eq!(folds, ["expanded", "expanded", "collapsed", "collapsed", "collapsed"]);
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
