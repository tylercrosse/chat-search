import { DatabaseSync } from "node:sqlite";

export const SCHEMA = `
CREATE TABLE IF NOT EXISTS conversation(
  id           TEXT PRIMARY KEY,
  source       TEXT NOT NULL,
  native_id    TEXT NOT NULL,
  title        TEXT,
  cwd          TEXT,
  git_branch   TEXT,
  model        TEXT,
  started_at   INTEGER,
  ended_at     INTEGER,
  msg_count    INTEGER NOT NULL DEFAULT 0,
  prose_count  INTEGER NOT NULL DEFAULT 0,
  forked_from  TEXT,
  resume_cmd   TEXT
);

CREATE TABLE IF NOT EXISTS message(
  id           TEXT PRIMARY KEY,
  conv_id      TEXT NOT NULL,
  parent_id    TEXT,
  seq          INTEGER NOT NULL,
  role         TEXT NOT NULL,
  kind         TEXT NOT NULL,
  ts           INTEGER,
  is_sidechain INTEGER NOT NULL DEFAULT 0,
  text         TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_message_conv ON message(conv_id, seq);

CREATE VIRTUAL TABLE IF NOT EXISTS fts_prose USING fts5(
  text, content='', tokenize="porter unicode61 remove_diacritics 2");
CREATE VIRTUAL TABLE IF NOT EXISTS fts_tools USING fts5(
  text, content='', tokenize="porter unicode61 remove_diacritics 2");
`;

/** Pragmas are identical in both implementations so the comparison stays fair. */
export function open(path: string, forWrite: boolean): DatabaseSync {
  const db = new DatabaseSync(path);
  if (forWrite) {
    db.exec("PRAGMA journal_mode=WAL");
    db.exec("PRAGMA synchronous=OFF");
    db.exec(SCHEMA);
  }
  return db;
}
