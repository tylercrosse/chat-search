//! The reply to a search — one type, one door, and the rules of the envelope (ADR 23).
//!
//! Sibling to [`crate::blocks`] and built the same way: this module holds the content and the
//! rules of the reply, never its rendering. Serializing an [`Answer`] *is* the wire, exactly as
//! serializing a [`crate::blocks::Transcript`] already is for `cs show --json`. The CLI prints
//! it, the TUI draws it, a Swift app decodes it — and none of the three assembles it.
//!
//! Until this existed the reply had no author, so every client became one: the envelope was
//! built inline at four sites in `cs/src/commands.rs`, the flat and grouped key lists agreed
//! only because a test asserted it, and `(ms * 100.0).round() / 100.0` appeared three times —
//! one copy feeding the query log, so a logged need and a printed answer could round
//! differently.
//!
//! # One door, and the routing behind it
//!
//! [`answer`] is the whole of asking. An empty or too-short query is answered out of the recent
//! list *inside* this call, so no caller has to know the recent route exists at all: the TUI's
//! old habit of blanking its own query text to force that branch has nothing left to force, and
//! a client that never learns the two routes apart cannot send a question down the wrong one.
//!
//! # The receipt
//!
//! A first pass declines to pay for the true total whenever the ranking scan stops at its
//! ceiling, and [`Answer::settle`] establishes it later. It replays a *private* receipt of the
//! original ask — the parsed query and the resolved options, `now_ms` included — so counting a
//! set other than the one on screen is unwritable rather than merely discouraged: there is no
//! second [`SearchOptions`] to pass and no clock to re-read. That invariant used to live in a
//! doc comment on the TUI's `options()`, beside a second options struct built on the pick path.
//!
//! # What is not here
//!
//! Nothing that renders. No colour, no width, no sigil — the same line [`crate::blocks`] draws.
//! And no second shape for the same question: `results` is always conversations, `--flat` has
//! its own small envelope ([`FlatAnswer`]), and a decoder never branches to learn what type a
//! key holds.

use std::time::Instant;

use serde::Serialize;

use crate::build::{Reader, Unreadable};
use crate::destination::Destination;
use crate::highlight::{self, Span};
use crate::query::Query;
use crate::search::{self, Hit, SearchOptions, Total};

/// Contract version, matching what [`crate::blocks::Transcript`] already carries.
///
/// Bumped when a field changes *meaning*, which is the one change a client cannot detect any
/// other way — a rename breaks loudly, an addition is ignored, and a key that quietly starts
/// meaning something else breaks silently. Additions are therefore not a bump; the policy is
/// stated for clients in `docs/JSON-CONTRACT.md`.
const V: u32 = 1;

/// What `cs search --json` emits, and what every other client reads.
///
/// Declared envelope first and results last so the struct reads in the order the reply is
/// understood. That is *not* the order a client sees: the CLI renders through
/// `serde_json::to_value`, whose `Map` is a `BTreeMap` in this workspace, so keys arrive
/// alphabetically — `count` first and `v` last, with `results` in the middle. Only a serializer
/// that walks the struct directly would honour the declaration order, and neither surface does.
///
/// `docs/JSON-CONTRACT.md` is where that is stated for clients, along with the instruction that
/// follows from it: decode by name, because the order is an artefact and not a promise.
#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    pub v: u32,
    /// The query as parsed, `--source` desugaring included ([`Query::with_source`] rewrites the
    /// text). What is reported is therefore what was actually run, not what a flag and a string
    /// would have to be recombined to reconstruct.
    pub query: String,
    /// How long the search took, in milliseconds, rounded to two decimal places.
    ///
    /// Measured and rounded here, once. Three clients used to round their own copy and one of
    /// those copies fed the query log, so the number a search was recorded under and the number
    /// it printed could differ in the last place.
    pub ms: f64,
    /// How many conversations came back — the length of `results`.
    pub count: usize,
    /// How many conversations the query selects with `limit` ignored.
    ///
    /// Read it beside `settled`. A hundred results out of a hundred and a hundred out of two
    /// thousand are the same hundred rows on screen, and this is the number that says which one
    /// is being read — but while `settled` is false it is a floor and a *poor* one, to be ranged
    /// rather than displayed as the answer: measured over the real index on 2026-08-04, the
    /// typeahead `the` came back with a floor of 1,025 against a true 4,243.
    /// [`Answer::settle`] replaces it with the whole number.
    pub total: usize,
    /// Whether `total` is the whole number rather than a floor.
    ///
    /// Two flat scalars rather than one polymorphic key: the `Total` enum stays honest inside
    /// this crate, where it is now the only thing that reads it, and a Swift `Codable` never
    /// meets a value that is sometimes a number and sometimes an object.
    pub settled: bool,
    /// Filter tokens that named a value nothing can be selected on — `date:nope`, a half-typed
    /// `agent:`. Returning unfiltered results for a query that names a filter is a worse answer
    /// than returning none, because it looks like it worked.
    ///
    /// A published key (ADR 12), spelled as it always was. What narrowed is which filters land
    /// in it: since chat-search-6eb.11 every filter the parser accepts is applied.
    pub unapplied_filters: Vec<String>,
    /// [`crate::IndexState`] as its wire name — `ready` or `rebuilding`, since the other two
    /// states never produce an answer at all.
    ///
    /// It lands here by construction rather than by a caller remembering to attach it: [`answer`]
    /// takes a [`Reader`], and a `Reader` is the thing that knows what was at the index path when
    /// it opened. `rebuilding` still means the results are complete — the index is swapped in
    /// whole (chat-search-me9.28) — and says only that a newer one is on the way.
    pub index_state: &'static str,
    /// How to read every `snippet_spans` below — [`highlight::OFFSETS`].
    pub mark_offsets: &'static str,
    /// The conversations that matched, best first. Always conversations: the polymorphic
    /// `results` and its sometimes-present `grouped` discriminator are gone.
    pub results: Vec<Group>,
    /// The ask this answer was born from. Never serialized, never readable — see [`Answer::settle`].
    #[serde(skip)]
    receipt: Receipt,
}

/// The ask an [`Answer`] ran under, kept so [`Answer::settle`] can replay it exactly.
///
/// `now_ms` is not a third field because it is already one of the resolved options, and that is
/// the point: a `date:` filter resolved against a second instant selects a second set of
/// conversations, so the clock has to travel with the rest of the ask rather than beside it.
#[derive(Debug, Clone)]
struct Receipt {
    query: Query,
    opts: SearchOptions,
}

/// Run a search and answer it, whatever the query turns out to be.
///
/// The routing is not the caller's decision and is not visible in the result: an empty query and
/// a query too short to expand are both answered out of the recent list, with an exact total,
/// through this same call. A client that wants that list asks for it by having nothing typed,
/// which is what it already had.
///
/// Takes a [`Reader`] rather than a bare `Connection` so `index_state` cannot be forgotten, and a
/// parsed [`Query`] rather than text so the TUI — which holds a `Query` for its facet chips — is
/// never asked to render one back to a string for core to re-parse.
pub fn answer(reader: &Reader, query: &Query, opts: &SearchOptions) -> rusqlite::Result<Answer> {
    let started = Instant::now();
    // Counted, always. The count costs nothing here: it is what the ranking pass saw on its way
    // past, and the queries it cannot answer are the broad prefixes whose total nobody is
    // reading yet. `settle` is where the rest is paid for, once, at the moment it is read.
    let found = search::search_grouped_counted(&reader.conn, query, opts)?;
    let (total, settled) = match found.matched {
        Total::Exact(n) => (n, true),
        Total::AtLeast(n) => (n, false),
    };
    let results: Vec<Group> = found.groups.into_iter().map(Group::from).collect();
    Ok(Answer {
        v: V,
        query: query.raw().to_string(),
        // Read after the results are built, so it covers making the reply rather than only the
        // SQL underneath it.
        ms: elapsed_ms(started),
        count: results.len(),
        total,
        settled,
        unapplied_filters: query.rejected(),
        index_state: reader.state.as_str(),
        mark_offsets: highlight::OFFSETS,
        results,
        receipt: Receipt { query: query.clone(), opts: opts.clone() },
    })
}

impl Answer {
    /// Establish the total the search declined to pay for. Reports whether it had anything to do.
    ///
    /// Call it when the answer is about to be read rather than when it is produced. The second
    /// pass costs 50–69 ms against the real index — `the` 49.6 ms, `the agent:codex` 68.6 ms over
    /// 4,377 conversations on 2026-08-04 — and it is needed only for the broad prefixes that
    /// leave a total unsettled, which are exactly the queries somebody is still typing, where the
    /// number buys nothing. In a one-shot CLI that moment is immediately; in a typeahead it is
    /// when the keystrokes stop.
    ///
    /// Idempotent: a settled answer is already the whole truth, so this returns `false` and
    /// touches nothing. A failure leaves the answer exactly as it was, and it is still true —
    /// the floor stands, the rows it describes are still on screen, and a caller may try again
    /// or not.
    ///
    /// It counts the set that was ranked because it cannot express anything else. The receipt
    /// holds the query and the options the ranking ran under, so there is no signature here
    /// through which a caller could hand it a different question.
    pub fn settle(&mut self, reader: &Reader) -> rusqlite::Result<bool> {
        if self.settled {
            return Ok(false);
        }
        // Only a searchable query can arrive here: the recent branch counts its own set exactly,
        // in a scan of `conversation` rather than of a term's postings, and comes back settled.
        let total = search::count_matching(&reader.conn, &self.receipt.query, &self.receipt.opts)?;
        self.total = total;
        self.settled = true;
        Ok(true)
    }
}

/// A conversation and the messages in it that matched — the Algolia DocSearch shape, where the
/// conversation is the result and matching messages nest beneath it.
///
/// Identity is stated once, here, and never repeated inside `matches`. Repeating `conv_id`,
/// `title`, `source` and the whole `destinations` array under every nested hit was bytes on a
/// payload that carries hundreds of rows, and a second place for two copies of one fact to
/// disagree.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub conv_id: String,
    pub source: String,
    /// The source's own id for this conversation (ADR 2 makes it stable).
    pub native_id: String,
    /// Every way to reopen this conversation, best first.
    ///
    /// Resolved in core because a client reading JSON cannot call [`crate::destinations`]. It
    /// replaces the single frozen `resume_cmd` string, which held one answer and went stale
    /// whenever a CLI changed its syntax (chat-search-me9.3).
    pub destinations: Vec<Destination>,
    /// `null` for the 11 untitled conversations in 3,059, none of them near the top of anything
    /// — which is how a client that typed this as non-optional got to `results[54]` before
    /// finding out (chat-search-me9.27).
    pub title: Option<String>,
    pub ended_at: Option<i64>,
    /// `ended_at` as a local `YYYY-MM-DD`, rendered in core so no client re-derives it.
    ///
    /// Redundant with `ended_at` on purpose: every client wants the day, and each one computing
    /// it from the epoch value gets its own chance to compute it in UTC and name tomorrow for an
    /// evening conversation — which is exactly how a shell client's jq and the binary's
    /// formatter came to disagree.
    pub ended_date: Option<String>,
    /// What a human would call a turn: user prose, not raw message count.
    pub user_turns: i64,
    /// Every message, tool traffic included. The denominator `match_seqs` positions are relative
    /// to.
    pub msg_count: i64,
    /// Prose messages only, which is what a prose search could possibly have matched.
    pub prose_count: i64,
    /// Working directory, for sources that have one. `null` for every ChatGPT conversation —
    /// 2,011 of this corpus — which a client must draw as "this source has no such thing"
    /// rather than as missing data (chat-search-6eb.26).
    pub cwd: Option<String>,
    /// **Opaque ordering.** Results arrive best-first; a client never re-sorts on this and never
    /// parses it. Today it is damped BM25 divided by a recency factor, and the ranking is
    /// expected to grow into hybrid lexical-plus-vector scoring (GLOSSARY.md's Indexer entry
    /// already says "and later vectors"). Promised as opaque now because that costs a sentence,
    /// where promising it after a client ships costs a `v` bump.
    pub score: f64,
    /// Total matching messages, including any this group does not carry.
    pub match_count: usize,
    /// Where the matches sit, as 0-based message positions, ascending.
    ///
    /// Three matches at the start of a forty-message conversation and three at the very end mean
    /// different things — the topic, or an aside on the way out — and 58% of conversations over
    /// 15 turns span more than four hours, so most of them are several sittings.
    pub match_seqs: Vec<i64>,
    /// What the conversation is made of, in reading order, run-length encoded:
    /// `[["user",1],["agent",1],["tool",34]]`. The shape a client draws as a strip.
    ///
    /// **Empty unless [`SearchOptions::shape`] asked for it**, which is not the same thing as a
    /// conversation with nothing in it — see that flag for what filling it costs, and
    /// `search::Group::kind_runs` for the two coordinate spaces this and `match_seqs` live in.
    pub kind_runs: Vec<crate::blocks::Run>,
    /// Gone from the source, kept here (ADR 9).
    pub deleted_upstream: bool,
    /// The conversations this row stands for, when it stands for more than itself, or `null`
    /// when it is one conversation — which is the whole corpus bar the Google Takeout records.
    /// A reconstruction over timestamps rather than something an export recorded, reported so a
    /// client can say so (`chat-search-o1i.5`).
    pub sitting: Option<crate::sittings::Sitting>,
    /// The best matching messages, best first, capped by [`SearchOptions::nested`]. Empty when
    /// nothing was asked for and on the recent route, where nothing matched.
    ///
    /// Clients truncate this rather than sample it, and the first entry is the message that won
    /// the ranking, so the order is load-bearing.
    pub matches: Vec<Match>,
}

/// One matching message, in a reply where its conversation is already on screen.
///
/// Message fields only. Everything about the parent — `conv_id`, `source`, `native_id`, `title`,
/// `destinations`, `deleted_upstream` — is on the [`Group`] that holds this. `thread_key` is not
/// among them: a conversation is a DAG (ADR 4) and the strand is a property of the message, so
/// stating it on the group would be a fact about several threads asserted about one.
#[derive(Debug, Clone, Serialize)]
pub struct Match {
    pub msg_id: String,
    pub role: String,
    pub kind: String,
    pub ts: Option<i64>,
    /// **Opaque ordering** — see [`Group::score`], which this is the message-level half of.
    pub score: f64,
    /// A window of the message around what matched, whitespace flattened, ellipses included.
    /// Prefixed with `⟨no match⟩ ` when the match could not be located — the sentence a
    /// reader gets, and not the same statement as an empty `snippet_spans`, which also covers a
    /// match that never had a lexical term to locate.
    pub snippet: String,
    /// Where the matched words are in `snippet`, in the units [`Answer::mark_offsets`] names.
    ///
    /// Carried because no client can recover them: locating a stemmed match means asking the
    /// index's own tokenizer, and by the time this exists the text has been windowed and its
    /// whitespace flattened, so even offsets into the message would no longer fit.
    ///
    /// **May be empty**, and not only for an unlocatable match: a semantic match need contain no
    /// lexical term to highlight at all. The model already carries the cousin of that idea in
    /// [`crate::MarkKind::Unranked`], and promising it now is free where promising it after a
    /// client ships is a `v` bump.
    pub snippet_spans: Vec<Span>,
    /// False when this message sits on a branch that was edited away — still searchable, but not
    /// part of the conversation as currently displayed.
    pub on_head_path: bool,
    /// A subagent strand: part of what happened, but not the conversation you were having.
    pub is_sidechain: bool,
    /// Which strand of the DAG this message belongs to (ADR 4).
    pub thread_key: String,
}

/// What `cs search --flat --json` emits: matching *messages*, ungrouped.
///
/// Deliberately a second, smaller envelope rather than a mode of [`Answer`]. Folding both into
/// one shape would readmit a `unit: conversation|message` discriminator that every decoder has
/// to model for a case the app never asks for, and its `count`-versus-`total` semantics would
/// differ per unit — the kind of rule that is misread in year two.
///
/// [`Hit`] survives here and only here. Carrying its parent's identity on every row is right in
/// a list with no parent rows in it, and wrong the moment there are.
#[derive(Debug, Clone, Serialize)]
pub struct FlatAnswer {
    pub v: u32,
    /// See [`Answer::query`].
    pub query: String,
    /// See [`Answer::ms`].
    pub ms: f64,
    /// How many messages came back — the length of `hits`.
    ///
    /// No `total` or `settled` beside it. The number worth settling is how many *conversations*
    /// a query selects, and this envelope is not about conversations; a message-level total
    /// would be a second counting rule that no surface reads.
    pub count: usize,
    /// See [`Answer::unapplied_filters`].
    pub unapplied_filters: Vec<String>,
    /// See [`Answer::index_state`].
    pub index_state: &'static str,
    /// See [`Answer::mark_offsets`].
    pub mark_offsets: &'static str,
    pub hits: Vec<Hit>,
}

impl FlatAnswer {
    /// Search for messages rather than conversations.
    ///
    /// No recent route, unlike [`answer`]: the fallback for a query too short to rank is a list
    /// of recent *conversations*, and there is no honest way to answer a question about messages
    /// with one. An empty query therefore comes back with nothing in it, which is what it means.
    pub fn of(
        reader: &Reader,
        query: &Query,
        opts: &SearchOptions,
    ) -> rusqlite::Result<FlatAnswer> {
        let started = Instant::now();
        let hits = search::search(&reader.conn, query, opts)?;
        Ok(FlatAnswer {
            v: V,
            query: query.raw().to_string(),
            ms: elapsed_ms(started),
            count: hits.len(),
            unapplied_filters: query.rejected(),
            index_state: reader.state.as_str(),
            mark_offsets: highlight::OFFSETS,
            hits,
        })
    }
}

/// Why there is nothing to search, as a body a client can branch on.
///
/// Core-owned for the same reason the reply is: the machine-readable error used to be assembled
/// at the print site, and before that it was one English sentence on stderr — which is what left
/// the first non-Rust client classifying index health by substring-matching another program's
/// prose (chat-search-me9.22).
#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    pub error: Reason,
}

/// The two halves of a refusal: the name that is the contract, and the sentence that is not.
#[derive(Debug, Clone, Serialize)]
pub struct Reason {
    /// [`Unreadable::code`] — stable, and what a client switches on.
    pub code: &'static str,
    /// Free to be reworded, and never parsed.
    pub message: String,
}

impl From<&Unreadable> for Refusal {
    fn from(why: &Unreadable) -> Self {
        Refusal { error: Reason { code: why.code(), message: why.to_string() } }
    }
}

/// Milliseconds since `started`, to two decimal places.
///
/// The only rounding of a search duration in this workspace. It was three, and one of the three
/// fed the query log.
fn elapsed_ms(started: Instant) -> f64 {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    (ms * 100.0).round() / 100.0
}

impl From<search::Group> for Group {
    /// The ranked row as a client reads it: identity kept, nested hits projected down to the
    /// message fields the group does not already state.
    ///
    /// Destructured exhaustively rather than field-by-field, so a column added to the ranked row
    /// fails to compile here instead of quietly never reaching the wire.
    fn from(g: search::Group) -> Self {
        let search::Group {
            conv_id,
            source,
            native_id,
            destinations,
            title,
            ended_at,
            ended_date,
            user_turns,
            msg_count,
            prose_count,
            cwd,
            score,
            match_count,
            match_seqs,
            kind_runs,
            deleted_upstream,
            sitting,
            hits,
        } = g;
        Group {
            conv_id,
            source,
            native_id,
            destinations,
            title,
            ended_at,
            ended_date,
            user_turns,
            msg_count,
            prose_count,
            cwd,
            score,
            match_count,
            match_seqs,
            kind_runs,
            deleted_upstream,
            sitting,
            matches: hits.into_iter().map(Match::from).collect(),
        }
    }
}

impl From<Hit> for Match {
    /// Drop what the group already says. Named rather than ignored with `..`, so adding a field
    /// to [`Hit`] is a decision about whether it is a fact about the message or about its
    /// conversation, taken here, at compile time.
    fn from(h: Hit) -> Self {
        let Hit {
            msg_id,
            role,
            kind,
            ts,
            score,
            snippet,
            snippet_spans,
            on_head_path,
            is_sidechain,
            thread_key,
            // Stated once on the group that holds this match.
            conv_id: _,
            source: _,
            native_id: _,
            destinations: _,
            title: _,
            deleted_upstream: _,
        } = h;
        Match {
            msg_id,
            role,
            kind,
            ts,
            score,
            snippet,
            snippet_spans,
            on_head_path,
            is_sidechain,
            thread_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::IndexState;
    use crate::model::{Conversation, Kind, Message, Role, Titles};
    use serde_json::Value;

    /// Later than every fixture timestamp, so decay is doing what it does in production rather
    /// than being switched off by a clock at the epoch (see [`SearchOptions::new`]).
    const NOW: i64 = 1_785_880_000_000;
    /// 2023-11-14T22:13:20Z, the instant the other fixtures hang off.
    const TS: i64 = 1_700_000_000_000;

    fn opts() -> SearchOptions {
        SearchOptions { limit: 10, nested: 3, ..SearchOptions::new(NOW) }
    }

    /// One message of a fixture conversation, chained to the one before it.
    ///
    /// The chaining is not decoration: `index::on_head_path` walks parent edges up from each
    /// thread's leaf, so a conversation whose messages all have no parent has exactly one
    /// message on the head path and every search filters the rest away.
    fn msg(seq: i64, role: Role, kind: Kind, text: &str) -> Message {
        Message {
            native_id: format!("m{seq}"),
            parent_native_id: (seq > 0).then(|| format!("m{}", seq - 1)),
            thread_key: "main".into(),
            is_sidechain: false,
            is_error: false,
            seq,
            role,
            kind,
            // chat-search-n58.25: declared per message, never inferred. A fixture states none.
            model: None,
            ts: Some(TS + seq),
            text: text.into(),
        }
    }

    fn conv(source: &str, native_id: &str, title: Option<&str>, msgs: Vec<Message>) -> Conversation {
        Conversation {
            source: source.into(),
            native_id: native_id.into(),
            titles: Titles { custom: title.map(String::from), ..Default::default() },
            cwd: Some("/Users/x/dev/chat-search".into()),
            git_branch: None,
            declared_model: None,
            surface: None,
            forked_from_native_id: None,
            head_native_id: None,
            messages: msgs,
        }
    }

    /// An index holding `convs`, opened as a reader in `state`.
    ///
    /// Built rather than mocked, so every derived column an answer reports — `ended_at`,
    /// `user_turns`, `msg_count`, the head path — is the indexer's own answer.
    fn reader(convs: &[Conversation], state: IndexState) -> Reader {
        let mut conn = crate::open(":memory:").unwrap();
        crate::write_conversations(&mut conn, convs.iter()).unwrap();
        Reader { conn, state }
    }

    /// Three conversations of different ages, all mentioning the borrow checker.
    fn corpus() -> Vec<Conversation> {
        vec![
            conv(
                "codex",
                "c0",
                Some("Borrow checker notes"),
                vec![
                    msg(0, Role::User, Kind::Prose, "how does the borrow checker decide?"),
                    msg(1, Role::Assistant, Kind::Prose, "the borrow checker tracks lifetimes"),
                    msg(2, Role::Assistant, Kind::ToolCall, "Read(lifetimes.rs)"),
                    msg(3, Role::Tool, Kind::ToolResult, "84 lines"),
                ],
            ),
            conv(
                "claude-code",
                "c1",
                Some("Lifetimes again"),
                vec![msg(0, Role::User, Kind::Prose, "borrow checker, one more time")],
            ),
            // Untitled, and the only conversation of the three a query for `elephant` finds.
            conv(
                "codex",
                "c2",
                None,
                vec![msg(0, Role::User, Kind::Prose, "an elephant never forgets")],
            ),
        ]
    }

    fn json(a: &Answer) -> Value {
        serde_json::to_value(a).expect("an answer is plain data")
    }

    fn keys(v: &Value) -> Vec<String> {
        let mut k: Vec<String> = v.as_object().expect("an object").keys().cloned().collect();
        k.sort();
        k
    }

    #[test]
    fn the_envelope_carries_every_field_a_client_was_promised() {
        // Spelled out rather than derived from the struct, because a rename would otherwise
        // rename this list along with the contract it is pinning.
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::exact("borrow"), &opts()).unwrap();
        let v = json(&a);
        assert_eq!(
            keys(&v),
            [
                "count",
                "index_state",
                "mark_offsets",
                "ms",
                "query",
                "results",
                "settled",
                "total",
                "unapplied_filters",
                "v",
            ]
        );
        assert_eq!(v["v"], 1);
        assert_eq!(v["query"], "borrow");
        assert_eq!(v["count"], 2, "two conversations mention it");
        assert_eq!(v["total"], 2);
        assert_eq!(v["settled"], true);
        assert_eq!(v["unapplied_filters"], serde_json::json!([]));
        assert_eq!(v["index_state"], "ready");
        assert_eq!(v["mark_offsets"], "utf8-bytes");
        assert!(v["ms"].is_number());
        // The receipt is not a field. A client that could read it could also hand it back.
        assert!(v.get("receipt").is_none(), "the receipt reached the wire");
        // And the discriminator that used to say which of two shapes `results` held.
        assert!(v.get("grouped").is_none(), "`results` is always conversations now");
    }

    #[test]
    fn a_group_states_its_conversation_once_and_its_matches_never_repeat_it() {
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(
            &r,
            &Query::exact("borrow"),
            &SearchOptions { shape: true, ..opts() },
        )
        .unwrap();
        let v = json(&a);
        // By id rather than by position: which of the two matching conversations ranks first is
        // the ranker's business, and this test is about the shape of one.
        let g = v["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["conv_id"] == "codex:c0")
            .expect("the conversation with two matches in it");
        assert_eq!(
            keys(g),
            [
                "conv_id",
                "cwd",
                "deleted_upstream",
                "destinations",
                "ended_at",
                "ended_date",
                "kind_runs",
                "match_count",
                "match_seqs",
                "matches",
                "msg_count",
                "native_id",
                "prose_count",
                "score",
                // Null on every row here, and on every row of every source but Google Takeout
                // — but a key a client decodes, so it is stated (`chat-search-o1i.5`).
                "sitting",
                "source",
                "title",
                "user_turns",
            ]
        );
        assert_eq!(g["conv_id"], "codex:c0");
        assert_eq!(g["source"], "codex");
        assert_eq!(g["native_id"], "c0");
        assert_eq!(g["title"], "Borrow checker notes");
        assert_eq!(g["cwd"], "/Users/x/dev/chat-search");
        assert_eq!(g["ended_at"], TS + 3);
        assert!(g["ended_date"].is_string(), "the local day, rendered by core");
        assert_eq!(g["user_turns"], 1);
        assert_eq!(g["msg_count"], 4);
        assert_eq!(g["prose_count"], 2);
        assert_eq!(g["match_count"], 2);
        assert_eq!(g["match_seqs"], serde_json::json!([0, 1]));
        assert!(g["score"].is_number());
        assert_eq!(g["deleted_upstream"], false);
        // Asked for, so it is filled: user turn, agent turn, the call — and not the successful
        // result, which no reader draws.
        assert_eq!(g["kind_runs"], serde_json::json!([["user", 1], ["agent", 1], ["tool", 1]]));
        assert_eq!(
            g["destinations"],
            serde_json::json!([{ "kind": "terminal", "argv": ["codex", "resume", "c0"] }])
        );

        let m = &g["matches"][0];
        assert_eq!(
            keys(m),
            [
                "is_sidechain",
                "kind",
                "msg_id",
                "on_head_path",
                "role",
                "score",
                "snippet",
                "snippet_spans",
                "thread_key",
                "ts",
            ]
        );
        // The whole point of the shape: everything about the parent is stated one level up.
        for repeated in ["conv_id", "source", "native_id", "title", "destinations", "deleted_upstream"] {
            assert!(m.get(repeated).is_none(), "{repeated} is repeated under every match again");
        }
        // Best-scoring first, which is the order a client relies on when it truncates rather
        // than samples — and bm25 is negative, so better is *more* negative.
        let scores: Vec<f64> =
            g["matches"].as_array().unwrap().iter().map(|m| m["score"].as_f64().unwrap()).collect();
        assert_eq!(scores.len(), 2, "both matching messages, capped at `nested`");
        assert!(scores.windows(2).all(|w| w[0] <= w[1]), "{scores:?}");

        let opening = g["matches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["msg_id"] == "codex:c0:m0")
            .expect("the user's own question is one of the matches");
        assert_eq!(opening["role"], "user");
        assert_eq!(opening["kind"], "prose");
        assert_eq!(opening["ts"], TS);
        assert_eq!(opening["thread_key"], "main", "a strand is a fact about the message (ADR 4)");
        assert_eq!(opening["on_head_path"], true);
        assert_eq!(opening["is_sidechain"], false);
        assert!(opening["snippet"].as_str().unwrap().contains("borrow"));
        assert!(
            !opening["snippet_spans"].as_array().unwrap().is_empty(),
            "the row ranked on a word that is in the snippet"
        );
    }

    #[test]
    fn a_tombstoned_conversation_says_so_on_the_group() {
        // No importer writes this column yet — ADR 9 makes it the tombstone a forget leaves —
        // so the fixture sets it the way a future one will, and the wire field is exercised
        // rather than asserted to be false forever.
        let r = reader(&corpus(), IndexState::Ready);
        r.conn
            .execute("UPDATE conversation SET deleted_upstream_at = ?1 WHERE id = 'codex:c0'", [TS])
            .unwrap();
        let a = answer(&r, &Query::exact("borrow"), &opts()).unwrap();
        let g = a.results.iter().find(|g| g.conv_id == "codex:c0").unwrap();
        assert!(g.deleted_upstream, "the tombstone is on the wire, not only in the index");
    }

    #[test]
    fn an_empty_query_is_answered_out_of_the_recent_list_by_the_same_call() {
        // The routing nobody has to know about. There is no second entry point to reach, and no
        // reason left to blank a query box to force this branch.
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::typeahead(""), &opts()).unwrap();
        assert_eq!(a.count, 3, "every conversation, most recently active first");
        assert_eq!(a.results[0].conv_id, "codex:c0");
        assert_eq!((a.total, a.settled), (3, true), "a list nothing was ranked for has an exact total");
        assert_eq!(a.query, "");
        assert!(a.results.iter().all(|g| g.matches.is_empty() && g.match_count == 0));
    }

    #[test]
    fn a_query_too_short_to_expand_takes_the_recent_route_too() {
        // `bo` is below the prefix floor, so it would be matched as a whole word and return
        // whatever happens to contain the literal token. Recency is the honest answer instead,
        // and the client is not told which of the two branches ran.
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::typeahead("bo"), &opts()).unwrap();
        assert_eq!(a.count, 3);
        assert_eq!((a.total, a.settled), (3, true));
        assert_eq!(a.query, "bo", "and the envelope still reports what was asked");
    }

    #[test]
    fn a_filtered_recent_list_is_counted_against_the_filter_and_not_the_corpus() {
        // `date:` with nothing typed is a real question — "what did I work on today" — and its
        // total is not the size of the index.
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::typeahead("agent:codex"), &opts()).unwrap();
        assert_eq!((a.count, a.total, a.settled), (2, 2, true));
    }

    /// 600 conversations mentioning the borrow checker, half of them Codex.
    ///
    /// Two matching messages each, so the ranking scan hits its 500-message ceiling and has to
    /// report a floor — which is the only way to reach the unsettled state at all.
    fn crowd() -> Vec<Conversation> {
        (0..600)
            .map(|i| {
                let source = if i % 2 == 0 { "codex" } else { "claude-code" };
                conv(
                    source,
                    &format!("x{i}"),
                    Some("borrow"),
                    vec![
                        msg(0, Role::User, Kind::Prose, "the borrow checker again"),
                        msg(1, Role::Assistant, Kind::Prose, "borrow, once more"),
                    ],
                )
            })
            .collect()
    }

    #[test]
    fn a_scan_that_stopped_at_its_ceiling_reports_a_floor_until_it_is_settled() {
        let r = reader(&crowd(), IndexState::Ready);
        let mut a = answer(&r, &Query::exact("borrow"), &SearchOptions { limit: 1, ..opts() }).unwrap();
        assert!(!a.settled, "1,200 matching messages cannot fit under a 500-message ceiling");
        let floor = a.total;
        assert!(floor <= 600, "a floor above the truth is worse than no number: {floor}");

        assert!(a.settle(&r).unwrap(), "settling had something to do");
        assert_eq!((a.total, a.settled), (600, true));
        // Idempotent, and cheap to call blindly: a settled answer is already the whole truth.
        assert!(!a.settle(&r).unwrap(), "a second settle went and counted again");
        assert_eq!(a.total, 600);
        assert_eq!(a.count, 1, "and settling never touches the rows that were ranked");
    }

    #[test]
    fn settling_counts_the_set_that_was_ranked_rather_than_one_resembling_it() {
        // The invariant the receipt exists for. `settle` takes a reader and nothing else, so
        // there is no signature through which the filters, the limit or the clock of the
        // original ask could be replaced by a second set that counts a different corpus.
        let r = reader(&crowd(), IndexState::Ready);
        let mut a =
            answer(&r, &Query::exact("borrow agent:codex"), &SearchOptions { limit: 1, ..opts() })
                .unwrap();
        assert!(!a.settled);
        assert!(a.settle(&r).unwrap());
        assert_eq!(a.total, 300, "the filtered set, not the 600 conversations that match the text");
    }

    #[test]
    fn a_filter_nothing_can_select_on_is_reported_rather_than_silently_dropped() {
        // Returning unfiltered results for a query that names a filter looks like it worked,
        // which is worse than returning none.
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::exact("borrow date:nope"), &opts()).unwrap();
        assert_eq!(a.unapplied_filters, ["date:nope"]);
        assert_eq!(json(&a)["unapplied_filters"], serde_json::json!(["date:nope"]));
    }

    #[test]
    fn an_answer_says_which_index_it_came_out_of_without_being_told() {
        // It arrives with the reader rather than from a caller, which is why `answer` takes one.
        // A client reading `rebuilding` knows the results are a whole previous build and can
        // offer to ask again, instead of guessing whether an empty answer is the final word.
        let r = reader(&corpus(), IndexState::Rebuilding);
        assert_eq!(answer(&r, &Query::exact("borrow"), &opts()).unwrap().index_state, "rebuilding");
        let r = reader(&corpus(), IndexState::Ready);
        assert_eq!(answer(&r, &Query::exact("borrow"), &opts()).unwrap().index_state, "ready");
    }

    #[test]
    fn the_spans_are_the_bytes_the_envelope_says_they_are() {
        // The half of the contract that draws blood. The Swift spike read these as character
        // offsets and mis-highlighted every snippet containing an em-dash, which in this corpus
        // is most of them — so the fixture puts multi-byte characters before the match and the
        // offsets are checked against the string as emitted.
        let text = "the café — and the borrow checker after it";
        let convs = vec![conv("codex", "c0", Some("t"), vec![msg(0, Role::User, Kind::Prose, text)])];
        let r = reader(&convs, IndexState::Ready);
        let a = answer(&r, &Query::exact("borrow"), &opts()).unwrap();

        let m = &a.results[0].matches[0];
        let span = *m.snippet_spans.first().expect("the row ranked on a word in it");
        assert_eq!(&m.snippet[span.start..span.end], "borrow");
        assert_ne!(
            span.start,
            m.snippet.chars().take_while(|c| *c != 'b').count(),
            "the fixture has to actually distinguish the two encodings"
        );
        // And it is named on the wire, in the spelling `cs show --json` already uses, because
        // the two contracts move together or not at all (chat-search-me9.33).
        assert_eq!(a.mark_offsets, "utf8-bytes");
        let transcript = serde_json::to_value(crate::Transcript::of("codex:c0", &[], Vec::new()))
            .unwrap();
        assert_eq!(json(&a)["mark_offsets"], transcript["mark_offsets"]);
    }

    #[test]
    fn a_snippet_and_its_spans_are_one_statement_made_twice() {
        // While matching is lexical the two channels agree: a match this crate's own tokenizer
        // cannot find is the only thing that empties the spans, so the prefix a reader sees and
        // the empty list a decoder sees are one fact stated twice, and neither has to be
        // re-derived from the other.
        //
        // That agreement is a property of this matcher, not a promise to clients.
        // docs/JSON-CONTRACT.md promises only that the spans may be empty, because a ranking
        // that matches on meaning will empty them for a result that holds no query term and is
        // not "no match" in any sense worth printing. So this test failing when that lands is
        // the signal to decide what the prefix means then — not licence to keep the equality by
        // labelling a semantic hit as unmatched.
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::exact("borrow"), &opts()).unwrap();
        for g in &a.results {
            for m in &g.matches {
                assert_eq!(
                    m.snippet.starts_with(crate::search::UNLOCATED),
                    m.snippet_spans.is_empty(),
                    "{}",
                    m.snippet
                );
            }
        }
    }

    #[test]
    fn ms_is_measured_and_rounded_here_so_no_client_rounds_it_again() {
        let r = reader(&corpus(), IndexState::Ready);
        let a = answer(&r, &Query::exact("borrow"), &opts()).unwrap();
        assert_eq!(a.ms, (a.ms * 100.0).round() / 100.0, "two decimal places, decided once");
        assert!(a.ms >= 0.0 && a.ms < 10_000.0, "{}", a.ms);
    }

    #[test]
    fn a_flat_answer_is_a_second_smaller_envelope_of_messages() {
        let r = reader(&corpus(), IndexState::Ready);
        let f = FlatAnswer::of(&r, &Query::exact("borrow"), &opts()).unwrap();
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(
            keys(&v),
            [
                "count",
                "hits",
                "index_state",
                "mark_offsets",
                "ms",
                "query",
                "unapplied_filters",
                "v",
            ]
        );
        assert_eq!(v["count"], 3, "three messages, where the grouped reply had two conversations");
        // No total to settle: the number worth settling counts conversations, and this envelope
        // is not about conversations.
        assert!(v.get("total").is_none() && v.get("settled").is_none());
        // The one surviving role of the dual-role hit: a message row that names its own parent,
        // in a list with no parent rows to name it.
        let h = v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["conv_id"] == "codex:c0")
            .expect("a message from the titled conversation");
        assert_eq!(h["title"], "Borrow checker notes");
        assert!(h["destinations"].is_array());
    }

    #[test]
    fn a_flat_answer_has_no_recent_route_because_a_message_list_has_no_such_thing() {
        let r = reader(&corpus(), IndexState::Ready);
        let f = FlatAnswer::of(&r, &Query::typeahead(""), &opts()).unwrap();
        assert_eq!((f.count, f.hits.len()), (0, 0));
    }

    #[test]
    fn a_refusal_is_the_error_body_a_client_branches_on() {
        // Same two keys the CLI has been printing, now stated where the rest of the contract
        // lives. `code` is the contract; `message` stays free to be reworded.
        let why = Unreadable::NoIndex { path: "/tmp/nope/index.db".into() };
        let r = Refusal::from(&why);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(keys(&v), ["error"]);
        assert_eq!(keys(&v["error"]), ["code", "message"]);
        assert_eq!(v["error"]["code"], "no_index");
        assert_eq!(v["error"]["message"], why.to_string());
        assert!(v["error"]["message"].as_str().unwrap().contains("/tmp/nope/index.db"));

        // Every state that can refuse, so none of them arrives as a code nobody documented.
        for why in [
            Unreadable::Building { path: "/tmp/index.db".into() },
            Unreadable::Stale("built by importer version 3".into()),
        ] {
            assert_eq!(Refusal::from(&why).error.code, why.code());
        }
    }

    #[test]
    fn an_answer_over_an_empty_index_is_an_answer_rather_than_a_failure() {
        // Nothing found is a result; an unreadable index is a `Refusal`. A client that cannot
        // tell those apart is the one that reported 40% of the corpus as missing.
        let conn = crate::open(":memory:").unwrap();
        let r = Reader { conn, state: IndexState::Ready };
        let a = answer(&r, &Query::exact("borrow"), &opts()).unwrap();
        assert_eq!((a.count, a.total, a.settled), (0, 0, true));
        assert!(a.results.is_empty());
    }

    /// The receipt is private, so this is the one thing a test can say about its type.
    #[test]
    fn an_answer_can_be_cloned_and_settled_independently() {
        let r = reader(&crowd(), IndexState::Ready);
        let a = answer(&r, &Query::exact("borrow"), &SearchOptions { limit: 1, ..opts() }).unwrap();
        let mut copy = a.clone();
        assert!(copy.settle(&r).unwrap());
        assert!(!a.settled, "the original is untouched, and its floor is still true");
        assert_eq!(copy.total, 600);
    }
}
