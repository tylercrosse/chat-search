//! The index schema.
//!
//! Every column here must be a pure function of (archive, importer version). Anything that
//! is not — read state, stars, notes — belongs in `library.db`, not here. Holding that
//! invariant is what makes `rm index.db && cs index` a valid response to any schema change,
//! and why this file has no migrations (ADR 1, ADR 3).

pub const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS conversation(
  id                  TEXT PRIMARY KEY,
  source              TEXT NOT NULL,
  native_id           TEXT NOT NULL,
  title               TEXT,
  title_origin        TEXT,          -- custom | generated | first_user
  cwd                 TEXT,
  git_branch          TEXT,
  model               TEXT,
  surface             TEXT,          -- derived, never part of an id (ADR 16)
  started_at          INTEGER,
  ended_at            INTEGER,
  msg_count           INTEGER NOT NULL DEFAULT 0,
  prose_count         INTEGER NOT NULL DEFAULT 0,
  user_turns          INTEGER NOT NULL DEFAULT 0,   -- what a human would call a 'turn'
  thread_count        INTEGER NOT NULL DEFAULT 0,
  forked_from         TEXT,          -- conversation.id of the parent, if declared
  head_id             TEXT,          -- currently-selected leaf; the only mutable notion
  -- No resume command. It is a function of (source, native_id) evaluated at display time, so
  -- storing it froze one answer and staleness-bombed every row when a CLI changed its syntax
  -- (chat-search-me9.3). See `crate::destination`.
  deleted_upstream_at INTEGER        -- tombstone: gone from source, kept here (ADR 9)
);

CREATE INDEX IF NOT EXISTS idx_conversation_source ON conversation(source, ended_at DESC);

CREATE TABLE IF NOT EXISTS message(
  id           TEXT PRIMARY KEY,
  conv_id      TEXT NOT NULL,
  parent_id    TEXT,                 -- DAG edge; NULL at a thread root (ADR 4)
  thread_key   TEXT NOT NULL,        -- carried explicitly, not parsed out of id (ADR 4)
  is_sidechain INTEGER NOT NULL DEFAULT 0,
  seq          INTEGER NOT NULL,
  role         TEXT NOT NULL,
  kind         TEXT NOT NULL,        -- prose | reasoning | tool_call | tool_result
  ts           INTEGER,
  on_head_path INTEGER NOT NULL DEFAULT 1,  -- reachable from head_id by parent walk
  is_error     INTEGER NOT NULL DEFAULT 0,  -- a tool_result reporting a failure
  text         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_conv    ON message(conv_id, seq);
CREATE INDEX IF NOT EXISTS idx_message_parent  ON message(parent_id);

-- Reading order, covering. `blocks::READING_ORDER` sorts by (is_sidechain, thread_key, seq) and
-- nothing here could produce it — the old `idx_message_thread` was (conv_id, thread_key, seq),
-- which omits the column the sort leads with, so every query that reads a conversation in order
-- built a temp b-tree over its messages first.
--
-- Carrying `role`, `kind` and `is_error` past the ordering columns is what makes the
-- conversation shape (`Group::kind_runs`) an index-only read. That matters more than three
-- narrow columns look: `message` stores its text inline, so selecting them out of the table
-- walks the bodies too, and the shape reads every message of every conversation a search
-- returns.
--
-- `on_head_path` sits *after* the ordering columns rather than in the middle of them, so a
-- query that does not constrain it still gets the order for free. The off-path toggle in
-- TUI-DESIGN §8 is not built yet, and an index that stopped working the day it was would be a
-- trap. The cost is walking the 4% of rows that are off-path, against a sort this removes.
--
-- What it costs on the way in: nothing measurable. Ten alternating full rebuilds of 4,490
-- conversations / 207,857 messages, this index against the three-column one it replaced, put
-- the medians at 9.40 s and 9.71 s — the wider index ahead, which only means the difference is
-- under the noise. Individual paired runs range −1.0 s to +1.1 s. The whole machine drifts far
-- harder than the schema does: the first five rounds ran 9.7–12.2 s and the last five 7.6–9.1 s,
-- which is why this had to be interleaved. An A-then-B reading of the same two builds would
-- have reported a 30% regression that is entirely warm-up, and nearly did (chat-search-me9.26).
-- On disk it is +4.4 MB on 388 MB.
CREATE INDEX IF NOT EXISTS idx_message_reading
  ON message(conv_id, is_sidechain, thread_key, seq, on_head_path, role, kind, is_error);

-- External content. The index stores postings only, exactly as `content=''` did, and reads
-- the text back out of `message` by rowid on the rare occasion it needs it — so this is the
-- same bytes on disk, not a second copy. What it buys is that fts5's auxiliary functions
-- become usable against the index: `highlight()` can mark a match in a row that is already
-- there, where a contentless table forced `crate::highlight` to re-insert every message it
-- wanted to mark into a scratch table first (chat-search-6eb.30).
--
-- The fts column name has to match the content table's column, and both are `text`. Postings
-- stay explicitly written by the indexer rather than by trigger, which is what external
-- content expects of a writer that owns its content table — and the two must not drift, since
-- `highlight()` now believes `message.text` is what was indexed.
--
-- A consequence worth knowing when writing queries: an unconstrained scan of `fts_prose`
-- returns every row of `message`, not just the ones with postings. Only a `MATCH` consults the
-- index. See `search::explain`, which counts postings out of the `_docsize` shadow table.
--
-- No `prefix='...'`, deliberately. It would turn a prefix term's vocabulary walk into a doclist
-- lookup, which is real — but the walk is 11 ms of a 60 ms `the`* keystroke and the other 49 is
-- BM25 scoring the whole match set, so the queries it was wanted for stay over budget while the
-- index grows 26% and the rebuild 48%. Priced and rejected in ADR 6 (chat-search-6eb.38).
--
-- Prose and tool traffic are separate tables rather than one with field weights, because
-- tool text is 91% of the corpus and would otherwise dominate BM25 (ADR 5).
CREATE VIRTUAL TABLE IF NOT EXISTS fts_prose USING fts5(
  text, content='message', content_rowid='rowid',
  tokenize="porter unicode61 remove_diacritics 2");
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tools USING fts5(
  text, content='message', content_rowid='rowid',
  tokenize="porter unicode61 remove_diacritics 2");

-- Provenance for the rebuild: which importer version produced the current contents.
CREATE TABLE IF NOT EXISTS build_info(
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

/// Bumped when importer output changes in a way that requires a rebuild. Recorded in
/// `build_info` so a stale index is detectable rather than silently wrong.
pub const IMPORTER_VERSION: u32 = 4;

#[cfg(test)]
mod tests {
    /// How SQLite says it will answer a statement.
    ///
    /// Asked of an empty database deliberately. With no rows there are no statistics to lean
    /// on, so what comes back is the plan the *schema* forces rather than one the planner
    /// happened to like against a particular corpus — and an empty database is the only state
    /// `rm index.db && cs index` guarantees on the way through.
    fn plan(sql: &str) -> String {
        let conn = crate::open(":memory:").expect("an in-memory index");
        let mut stmt =
            conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).expect("a plannable statement");
        // Column 3 of `EXPLAIN QUERY PLAN` is `detail`, the sentence the planner writes. Both
        // statements take a `conv_id`, and it has to be bound even though the plan is fixed
        // at prepare time and cannot depend on what it is.
        let steps: Vec<String> = stmt
            .query_map(rusqlite::params!["codex:whichever"], |r| r.get(3))
            .expect("plan rows")
            .collect::<rusqlite::Result<_>>()
            .expect("plan rows");
        steps.join("\n")
    }

    /// Asserted rather than trusted, because `idx_message_thread` carried a comment claiming
    /// exactly this and could not deliver it for the whole time it existed. A comment about a
    /// query plan is a guess about another program's behaviour; nothing but the planner can
    /// settle it (chat-search-me9.26).
    #[test]
    fn the_conversation_shape_is_answered_from_the_index_alone() {
        let plan = plan(&crate::search::kind_runs_sql());
        assert!(
            plan.contains("COVERING INDEX idx_message_reading"),
            "the shape must never reach the table: `message` stores its text inline, so a plan \
             that fetches rows walks the bodies to collect three narrow columns, and it does \
             that for every message of every conversation a search returns. A fourth column in \
             the SELECT is all it takes to lose this, and losing it is silent — the answer \
             stays correct and only the keystroke gets slower. Plan was:\n{plan}"
        );
        assert!(
            !plan.contains("TEMP B-TREE"),
            "and no sort: the index is already in reading order. Plan was:\n{plan}"
        );
    }

    /// The other half of the same index, and the half that is easy to lose sight of — this
    /// query selects `text`, so it can never be covering, and the only thing it takes from
    /// `idx_message_reading` is the ordering.
    #[test]
    fn reading_a_conversation_in_order_costs_no_sort() {
        let plan = plan(&crate::blocks::load_sql());
        assert!(
            plan.contains("idx_message_reading"),
            "reading order comes from the index. Plan was:\n{plan}"
        );
        assert!(
            !plan.contains("TEMP B-TREE"),
            "a temp b-tree here means the whole conversation is sorted in memory before the \
             first message comes back, which is what `blocks::READING_ORDER` against the old \
             (conv_id, thread_key, seq) index did. Plan was:\n{plan}"
        );
    }
}
