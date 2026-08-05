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
  -- The model of the last message that named one, resolved in the rollup at the bottom of
  -- `index::write_conversations_with` — never collapsed by an importer. See `message.model`.
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
  -- The model that produced this message, as declared; NULL on user turns and tool results.
  -- 166 of 3,097 conversations name more than one, so this is where that fact lives; the
  -- `conversation.model` label is a summary of this column (chat-search-n58.25, ADR 23).
  model        TEXT,
  text         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_conv    ON message(conv_id, seq);
CREATE INDEX IF NOT EXISTS idx_message_parent  ON message(parent_id);
CREATE INDEX IF NOT EXISTS idx_message_thread  ON message(conv_id, thread_key, seq);

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
pub const IMPORTER_VERSION: u32 = 5;
