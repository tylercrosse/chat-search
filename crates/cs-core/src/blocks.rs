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

/// The order a conversation is read in, and so the order every client draws it in.
///
/// `seq` is per *thread*, not per conversation — ADR 4 makes a conversation a DAG and
/// `thread_key` carries which strand a message belongs to. Ordering by `seq` alone therefore
/// interleaves every thread at once: on a real 9-thread conversation it put all nine opening
/// user turns first, then all nine first replies, which reads as nonsense. Main thread before
/// sidechains, each strand contiguous. `idx_message_thread` is (conv_id, thread_key, seq),
/// which exists for exactly this.
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

    /// The one line that stands in for this whole message when it is folded shut.
    ///
    /// Outline mode is every message reduced to this, and §8's table of collapsed forms is the
    /// spec: prose is the matching line else the first non-empty one, a `tool_call` is its name
    /// plus the argument that identifies it, and a failed result keeps its error text. Because
    /// the prose case prefers the *matching* line, a wholly folded conversation is a map of
    /// where the query landed, in conversation order — which is what makes outline mode worth
    /// more than a density knob.
    ///
    /// Content only. Which glyph fronts the row — the gear, the ellipsis, the cross, the
    /// speaker — is the client's business, the same line [`Block::mark_kind`] draws: core says
    /// what the line reads, the client says how it looks.
    pub fn collapsed(&self) -> Collapsed {
        let quoted = |(at, text): (usize, String)| Collapsed { text, at: Some(at) };
        // Asked before `kind`, because a failed result is the one result that survives
        // ([`Block::drawn`]), and it is the failure rather than the tool that earned the row.
        if self.is_error {
            return quoted(recognition_line(self));
        }
        // Resolved through [`crate::Kind`] rather than matched as strings, so a new kind has to
        // say which collapsed form it takes instead of inheriting the quoted one in silence.
        match crate::Kind::ALL.into_iter().find(|k| k.as_str() == self.kind) {
            // Reasoning opens with its own bolded summary — `**Planning README creation**` — so
            // where nothing matched, the model has already written the one-line version. Use it
            // rather than counting lines at the reader, which is what §8 first specced.
            Some(crate::Kind::Reasoning) => {
                let (at, line) = recognition_line(self);
                // The asterisks are the model's emphasis syntax rather than content, and
                // dropping them moves the offset every mark is rebased against.
                let text = line.trim_matches('*').to_string();
                let dropped = line.find(&text).unwrap_or(0);
                Collapsed { text, at: Some(at + dropped) }
            }
            // The one collapsed form that is *synthesised* rather than quoted, and therefore the
            // one with no offset to give.
            Some(crate::Kind::ToolCall) => Collapsed { text: tool_summary(&self.text), at: None },
            Some(crate::Kind::Prose | crate::Kind::ToolResult) | None => {
                quoted(recognition_line(self))
            }
        }
    }
}

/// One line standing in for a whole message, and where in the message it came from.
///
/// Built by [`Block::collapsed`]. Nothing here is styled: no sigil, no width, no colour.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Collapsed {
    /// What the row says.
    pub text: String,
    /// Byte offset of `text` within [`Block::text`], so a caller can rebase [`Block::marks`]
    /// onto the line: a mark lying wholly inside `at .. at + text.len()` belongs at
    /// `start - at`, and one straddling either edge is dropped rather than clamped — a clamped
    /// mark highlights a word fragment that no term actually matched.
    ///
    /// `None` when the line was *synthesised* from the message's fields instead of quoted out
    /// of its text, which today means a `tool_call`: `Bash ls -la` is a name taken from one line
    /// and an argument from inside the JSON on another, so it is a slice of nothing and no
    /// offset would be true. A caller reading a missing offset as zero would mark whatever byte
    /// happened to sit there, which is why this is an absence rather than a default — and why
    /// it is the one thing `cs show --json` emits as null.
    pub at: Option<usize>,
}

/// The one line of a message worth standing in for all of it, with its byte offset.
///
/// The line the query hit, when there is one. §8 specs this, and it is the whole reason outline
/// mode is worth more than a density knob: with every message reduced to the line that matched,
/// the pane becomes a map of where the query landed across the conversation, in conversation
/// order — complementary to the top-3 hit rows in the results list rather than a duplicate of
/// them. Falling back to the first non-empty line makes a non-matching message a table of
/// contents entry, which is all it can honestly be.
///
/// The offset is of the line *as returned*, so a caller rebasing marks onto that string does not
/// have to account for the leading whitespace this trimmed off.
pub fn recognition_line(block: &Block) -> (usize, String) {
    let marked = offset_lines(&block.text)
        .find(|(at, line)| block.marks.iter().any(|m| m.start >= *at && m.start < at + line.len()));
    let (at, line) = marked
        .or_else(|| offset_lines(&block.text).find(|(_, l)| !l.trim().is_empty()))
        .unwrap_or((0, ""));
    (at + (line.len() - line.trim_start().len()), line.trim().to_string())
}

/// `name` plus whatever argument identifies the call.
///
/// A `tool_call` body is the tool name on its own line followed by a JSON argument object, so
/// the name is free and the useful part is picking the one argument a human would use to
/// recognise the call — the command for a shell, the path for a read.
///
/// Measured over all 66,417 `tool_call` messages in this corpus: 80.1% carry a JSON object,
/// 6.6% an `apply_patch` body, and 13.3% something neither. The keys below are the string-valued
/// arguments that actually appear, in the order a reader would recognise the call by.
pub fn tool_summary(text: &str) -> String {
    let summary = summarise_call(text);
    // One row, whatever the argument turned out to be. Enforced here rather than by each branch
    // below, because a branch that forgot is exactly how this went wrong: the argv shape — the
    // common one, and the one whose scripts carry heredocs and `&&` chains — kept its newlines
    // while the two shapes either side of it stripped theirs, and 469 of this corpus's calls
    // collapsed to something that was not a line.
    //
    // Split with `lines` rather than replacing `'\n'`, so a CRLF script collapses the same way
    // and this agrees with [`offset_lines`] about where a line ends.
    if summary.contains('\n') {
        return summary.lines().collect::<Vec<_>>().join(" ");
    }
    summary
}

/// [`tool_summary`] before the one-row guarantee, which is the only reason this is split out:
/// the guarantee needs one place to live, and this has five `return`s.
fn summarise_call(text: &str) -> String {
    let mut lines = text.lines();
    let name = lines.next().unwrap_or("tool").trim().to_string();
    let rest: String = lines.collect::<Vec<_>>().join("\n");

    // `apply_patch` carries a unified patch instead of JSON, and its `*** Update File: <path>`
    // header names what the call touched. All 4,366 patch bodies in this corpus have one, and
    // without it every one of them collapses to a bare `apply_patch`.
    if rest.trim_start().starts_with("***") {
        let target = rest
            .lines()
            .filter(|l| l.starts_with("*** "))
            .find_map(|l| l.split_once(" File: "));
        return match target {
            Some((_, path)) => format!("{name} {}", path.trim()),
            None => name,
        };
    }

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&rest) else {
        return name;
    };

    // The command is spelled differently by each tool and all the spellings are common here:
    // Codex writes `cmd` (25,790 calls — the most frequent argument of any name in the corpus),
    // Claude Code writes `command` (10,623), as an argv array in one and a single string in the
    // other. Handling one of them meant the rest collapsed to a bare name — the tool with the
    // one piece of information a reader wanted stripped out.
    if let Some(cmd) = json.get("command").or_else(|| json.get("cmd")) {
        if let Some(line) = cmd.as_str() {
            return format!("{name} {line}");
        }
        if let Some(argv) = cmd.as_array() {
            let parts: Vec<&str> = argv.iter().filter_map(|v| v.as_str()).collect();
            // `["bash", "-lc", "<script>"]` is the overwhelmingly common argv shape, and the
            // script is the only interesting element — the shell and its flag never vary.
            let shown = match parts.as_slice() {
                [_shell, flag, script, ..] if flag.starts_with('-') => script,
                _ => return format!("{name} {}", parts.join(" ")),
            };
            return format!("{name} {shown}");
        }
    }
    // Paths first, because a call that names a file is recognised by the file. `description`
    // and `prompt` come last: they identify the calls that have nothing better — `Task`,
    // `AskUserQuestion` — but they are prose a tool wrote about itself, so anything concrete
    // beats them.
    for key in ["file_path", "path", "notebook_path", "pattern", "query", "url", "uri", "prompt",
                "description", "plan"]
    {
        if let Some(v) = json.get(key).and_then(|v| v.as_str()) {
            return format!("{name} {v}");
        }
    }
    name
}

/// The lines of a message with their byte offsets, so a mark can be attributed to the line
/// holding it.
///
/// Splits exactly where `str::lines` splits — `split_inclusive` yields one chunk per line and
/// agrees with it on a trailing newline, where a plain `split('\n')` would report one line too
/// many. That agreement is load-bearing past [`recognition_line`]: the TUI counts rendered rows
/// with it and turns a click into a message with those counts, so an off-by-one on the last
/// line of any expanded message would fold the wrong one.
pub fn offset_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |chunk| {
        let start = at;
        at += chunk.len();
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        (start, line.strip_suffix('\r').unwrap_or(line))
    })
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
    // [`READING_ORDER`] carries why this is not `ORDER BY seq`, and is shared rather than
    // repeated because the shape on a search row has to come out in the same order as this.
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id, role, kind, seq, on_head_path, text, is_sidechain, thread_key, is_error
         FROM message WHERE conv_id = ?1 AND on_head_path = 1
         {READING_ORDER}"
    ))?;
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
    /// [`Block::collapsed`] — the row this message shows when it is folded shut.
    ///
    /// Carried for every message rather than only the folded ones, because which messages are
    /// folded is session state the client holds, not a property of the conversation. Without it
    /// a client can list a conversation but cannot draw outline mode, which is the mode that
    /// makes a 900-message agent session legible — and the two attempts to build one of these
    /// outside Rust both began by reimplementing which line stands in for which message.
    pub collapsed: Collapsed,
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
                    collapsed: block.collapsed(),
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

    /// A block with its matches already located, as [`load`] would leave it.
    fn marked(kind: &str, role: &str, id: &str, text: &str, query: &str) -> Block {
        let mut b = block(kind, role, id, text);
        b.marks = highlight::spans(text, &crate::Query::exact(query).marking_terms());
        b
    }

    #[test]
    fn a_shell_call_collapses_to_the_script_not_the_shell() {
        let t = "shell\n{\"command\":[\"bash\",\"-lc\",\"sed -n '1,160p' config.py\"],\"workdir\":\".\"}";
        assert_eq!(tool_summary(t), "shell sed -n '1,160p' config.py");
    }

    #[test]
    fn a_command_string_reads_the_same_as_a_command_array() {
        // Codex writes argv, Claude Code writes a line. Both are `command`, and handling one
        // shape silently rendered the other as a bare tool name.
        assert_eq!(tool_summary("Bash\n{\"command\":\"ls -la\"}"), "Bash ls -la");
        assert_eq!(
            tool_summary("Bash\n{\"command\":\"echo one\\necho two\"}"),
            "Bash echo one echo two",
            "a multi-line script still occupies one row"
        );
    }

    #[test]
    fn a_collapsed_call_is_one_row_whatever_shape_the_arguments_arrived_in() {
        // The sentence the old tests nearly proved. "A multi-line script still occupies one row"
        // was asserted for a `command` string and for nothing else, so the argv shape — the
        // common one, and the one that actually carries heredocs — kept its newlines. 469 calls
        // in this corpus, each of them turning one outline row into three and putting a client's
        // row count out of step with the TUI's.
        let bodies = [
            // argv, the shape that was wrong.
            "shell\n{\"command\":[\"bash\",\"-lc\",\"cd /x && cat <<'EOF'\\nhello\\nEOF\"]}",
            // argv that does not match the shell-flag-script shape, so it joins the parts.
            "shell\n{\"cmd\":[\"printf\",\"one\\ntwo\"]}",
            // the string spelling, which was already right.
            "Bash\n{\"command\":\"echo one\\necho two\"}",
            // a prose argument, which can be as multi-line as a script.
            "Task\n{\"description\":\"do this\\nthen that\"}",
            // and a patch, whose body is nothing but newlines.
            "apply_patch\n*** Begin Patch\n*** Update File: a/b.rs\n@@\n-x\n+y",
        ];
        for body in bodies {
            let line = tool_summary(body);
            assert!(!line.contains('\n'), "{body:?} collapsed to {line:?}");
            // And the flattening joins rather than deletes, so two commands cannot read as one
            // word: `one\ntwo` is `one two`, never `onetwo`.
            assert!(!line.contains("  "), "{body:?} collapsed to {line:?}");
        }
        assert_eq!(
            tool_summary(bodies[0]),
            "shell cd /x && cat <<'EOF' hello EOF",
            "the script is still all there, on one row"
        );
    }

    #[test]
    fn a_call_with_an_identifying_path_uses_it() {
        assert_eq!(tool_summary("Read\n{\"file_path\":\"/x/schema.rs\"}"), "Read /x/schema.rs");
    }

    #[test]
    fn an_unparseable_body_still_yields_the_tool_name() {
        // Argument shapes vary per tool and per version; the name alone is always better
        // than a raw JSON fragment wrapped into the pane. 13.3% of this corpus's calls land
        // here, so it is the fallback, not the edge case.
        assert_eq!(tool_summary("update_plan\nnot json at all"), "update_plan");
        assert_eq!(tool_summary("bare_name"), "bare_name");
    }

    #[test]
    fn codexs_spelling_of_a_command_reads_the_same_as_claude_codes() {
        // `cmd` is the most frequent argument name in this corpus at 25,790 calls, more than
        // `command` itself, and it went unread — so every Codex `exec_command` drew as a bare
        // tool name. Found by folding the prototype's second implementation back into this one:
        // it knew about `cmd` and this did not.
        assert_eq!(
            tool_summary("exec_command\n{\"cmd\":[\"bash\",\"-lc\",\"cargo test\"],\"workdir\":\".\"}"),
            "exec_command cargo test"
        );
        assert_eq!(tool_summary("exec\n{\"cmd\":\"ls -la\"}"), "exec ls -la");
        // And `command` still wins where a call carries both spellings.
        assert_eq!(
            tool_summary("Bash\n{\"command\":\"git status\",\"cmd\":\"nonsense\"}"),
            "Bash git status"
        );
    }

    #[test]
    fn a_patch_collapses_to_the_file_it_edits() {
        // `apply_patch` is 6.6% of this corpus's calls and carries a unified patch rather than
        // JSON, so it fell through to the bare name — 4,366 rows reading `apply_patch` and
        // nothing else. Every one of them has this header.
        let patch = "apply_patch\n*** Begin Patch\n*** Update File: crates/cs-core/src/blocks.rs\n@@\n-a\n+b\n*** End Patch";
        assert_eq!(tool_summary(patch), "apply_patch crates/cs-core/src/blocks.rs");
        // Add and Delete name their file the same way; the verb is not the identifying part.
        assert_eq!(
            tool_summary("apply_patch\n*** Begin Patch\n*** Add File: docs/NEW.md\n+hello"),
            "apply_patch docs/NEW.md"
        );
        // A patch with no header at all is still better as a name than as a diff fragment.
        assert_eq!(tool_summary("apply_patch\n*** Begin Patch\n+stray"), "apply_patch");
    }

    #[test]
    fn a_call_whose_only_argument_is_prose_uses_it_rather_than_drawing_a_bare_name() {
        // `description` and `prompt` are what identify the calls with nothing concrete — a
        // subagent task, a question put to the user. Ranked below every path and pattern,
        // because a tool's prose about itself is the weakest thing a reader can recognise.
        assert_eq!(
            tool_summary("Task\n{\"description\":\"Find the outline spec\",\"subagent_type\":\"x\"}"),
            "Task Find the outline spec"
        );
        assert_eq!(
            tool_summary("Read\n{\"file_path\":\"/x/schema.rs\",\"description\":\"look at it\"}"),
            "Read /x/schema.rs",
            "something concrete beats the tool's own prose"
        );
        // Multi-line prose still occupies exactly one row, the same as a multi-line script.
        assert_eq!(
            tool_summary("Task\n{\"prompt\":\"do this\\nthen that\"}"),
            "Task do this then that"
        );
    }

    #[test]
    fn a_collapsed_message_shows_the_line_the_query_hit() {
        // §8: collapsed prose is "the matching line, else the first non-empty line". Showing
        // the first line instead makes outline mode a table of contents when its whole purpose
        // is being a map of where the query landed.
        let text = "a preamble nobody asked about\nsome filler\nthe borrow checker complained";
        let b = marked("prose", "assistant", "a", text, "borrow");
        assert!(!b.marks.is_empty(), "the fixture has to actually match");
        assert_eq!(recognition_line(&b).1, "the borrow checker complained");

        // Stemmed, because the whole point is agreeing with the ranker rather than with a
        // substring scan: `commits` ranks this row on `Commit`.
        let b = marked("prose", "user", "b", "hello there\nCommit current changes", "commits");
        assert_eq!(recognition_line(&b).1, "Commit current changes");
    }

    #[test]
    fn a_message_that_matched_nothing_falls_back_to_its_first_real_line() {
        let b = marked("prose", "user", "a", "\n\n  first words here\nlater", "elephant");
        assert!(b.marks.is_empty());
        assert_eq!(recognition_line(&b).1, "first words here");
    }

    #[test]
    fn offset_lines_splits_exactly_where_str_lines_splits() {
        // A renderer counts rows with this and turns clicks into messages with those counts, so
        // one extra line on a trailing newline — which `split('\n')` reports and `lines()` does
        // not — puts every click after it on the wrong message.
        for text in ["", "a", "a\n", "a\nb", "a\nb\n", "\n\n", "a\r\nb\r\n", "\n"] {
            let got: Vec<&str> = offset_lines(text).map(|(_, l)| l).collect();
            let want: Vec<&str> = text.lines().collect();
            assert_eq!(got, want, "{text:?}");
        }
        // And the offsets point at the line they came from.
        let text = "alpha\nbeta\ngamma";
        for (at, line) in offset_lines(text) {
            assert_eq!(&text[at..at + line.len()], line);
        }
    }

    #[test]
    fn a_quoted_collapsed_line_reports_where_it_was_lifted_from() {
        // The offset is the whole seam: a client rebases this message's marks onto the one line
        // it shows, and an offset that is off by the leading whitespace or by the trimmed
        // asterisks highlights the wrong word rather than failing.
        let text = "a preamble nobody asked about\n   the borrow checker complained";
        let b = marked("prose", "assistant", "a", text, "borrow");
        let c = b.collapsed();
        let at = c.at.expect("a quoted line knows where it came from");
        assert_eq!(&b.text[at..at + c.text.len()], c.text, "the offset does not point at the line");
        // And the mark rebases onto the word it actually matched.
        let m = b.marks.iter().find(|m| m.start >= at).expect("the fixture has to match");
        assert_eq!(&c.text[m.start - at..m.end - at], "borrow");
    }

    #[test]
    fn reasoning_collapses_to_the_models_own_summary_without_its_asterisks() {
        // `**Planning README creation**` is the one-line version the model already wrote. The
        // asterisks are its emphasis syntax rather than content, and dropping them has to move
        // the offset with them or every mark on the line lands two bytes early.
        let b = marked("reasoning", "assistant", "a", "**Planning the borrow fix**\ndetail", "borrow");
        let c = b.collapsed();
        assert_eq!(c.text, "Planning the borrow fix");
        let at = c.at.expect("reasoning is quoted, not synthesised");
        assert_eq!(&b.text[at..at + c.text.len()], c.text, "the asterisks moved the offset");
    }

    #[test]
    fn a_synthesised_tool_summary_has_no_offset_to_give() {
        // `Bash grep borrow src` is built from the tool name and one argument, not quoted from
        // the message, so `marks` index text that is not on the line. An offset here would be
        // fiction, and a client rebasing against it would highlight whatever sat at that byte.
        let b = marked("tool_call", "assistant", "a", "Bash\n{\"command\":\"grep borrow src\"}", "borrow");
        let c = b.collapsed();
        assert_eq!(c.text, "Bash grep borrow src");
        assert_eq!(c.at, None, "a synthesised line cannot locate a mark");
        assert!(!b.marks.is_empty(), "the fixture matched, which is what makes the None matter");
    }

    #[test]
    fn a_failed_result_collapses_to_its_error_text_whatever_kind_it_is() {
        // §8 keeps the failure and drops the success. `is_error` is asked before `kind` so the
        // error text is quoted rather than run through the form its kind would otherwise take.
        let mut bad = block("tool_result", "tool", "b", "error: no such file or directory");
        bad.is_error = true;
        assert_eq!(bad.collapsed().text, "error: no such file or directory");
        assert_eq!(bad.collapsed().at, Some(0));
    }

    #[test]
    fn every_kind_collapses_to_something_a_client_can_draw() {
        // A kind with no collapsed form would be a blank row in outline mode — the one mode
        // where a message *is* its collapsed line, so an empty one loses the message entirely.
        // And one row means one row: outline mode is a message per line, so a collapsed form
        // carrying a newline puts a client's row count out of step with the TUI's.
        let bodies = [
            "shell\n{\"command\":[\"bash\",\"-lc\",\"cd /x\\nls\"]}",
            "**Planning**\nthe long version\nwith more lines",
            "a first line\nand a second",
        ];
        for kind in crate::Kind::ALL {
            for body in bodies {
                for is_error in [false, true] {
                    let mut b = block(kind.as_str(), "assistant", "a", body);
                    b.is_error = is_error;
                    let c = b.collapsed();
                    assert!(!c.text.is_empty(), "{kind:?}/{is_error} collapsed to nothing");
                    assert!(!c.text.contains('\n'), "{kind:?}/{is_error} collapsed to {:?}", c.text);
                }
            }
        }
    }

    #[test]
    fn the_wire_form_carries_the_collapsed_row_so_outline_mode_needs_no_second_copy() {
        // The acceptance test for the move: everything a client needs to draw one outline row
        // is on the wire. Before this, a JSON client could list a conversation's messages but
        // had to reimplement which line stands in for each of them — which two prototypes did.
        let t = Transcript::of(
            "claude-code:abc",
            &[],
            vec![
                block("prose", "user", "a", "  the opening question\nmore"),
                block("tool_call", "assistant", "b", "Read\n{\"file_path\":\"/x/schema.rs\"}"),
            ],
        );
        let v = serde_json::to_value(&t).expect("the transcript is plain data");
        let msgs = v["messages"].as_array().expect("an array");
        assert_eq!(msgs[0]["collapsed"], serde_json::json!({
            "text": "the opening question",
            "at": 2,
        }));
        // The synthesised form says so in the only way JSON can, rather than claiming an offset.
        assert_eq!(msgs[1]["collapsed"], serde_json::json!({
            "text": "Read /x/schema.rs",
            "at": null,
        }));
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
