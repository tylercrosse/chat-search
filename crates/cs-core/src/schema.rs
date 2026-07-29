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
  resume_cmd          TEXT,
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
  text         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_conv    ON message(conv_id, seq);
CREATE INDEX IF NOT EXISTS idx_message_parent  ON message(parent_id);
CREATE INDEX IF NOT EXISTS idx_message_thread  ON message(conv_id, thread_key, seq);

-- Contentless: the index stores postings only and joins back to message by rowid.
-- Prose and tool traffic are separate tables rather than one with field weights, because
-- tool text is 91% of the corpus and would otherwise dominate BM25 (ADR 5).
CREATE VIRTUAL TABLE IF NOT EXISTS fts_prose USING fts5(
  text, content='', tokenize="porter unicode61 remove_diacritics 2");
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tools USING fts5(
  text, content='', tokenize="porter unicode61 remove_diacritics 2");

-- Provenance for the rebuild: which importer version produced the current contents.
CREATE TABLE IF NOT EXISTS build_info(
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
"#;

/// Bumped when importer output changes in a way that requires a rebuild. Recorded in
/// `build_info` so a stale index is detectable rather than silently wrong.
pub const IMPORTER_VERSION: u32 = 1;
