import { homedir } from "node:os";
import { join } from "node:path";
import { open } from "./db.ts";
import { discover, importClaudeCode, importCodex } from "./importers.ts";
import { search } from "./search.ts";
import type { Conv } from "./types.ts";

const DEFAULT_DB = join(homedir(), ".chat-search", "poc-ts.db");

function arg(name: string, fallback?: string): string | undefined {
  const i = process.argv.indexOf(`--${name}`);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : fallback;
}
const has = (name: string) => process.argv.includes(`--${name}`);

function cmdIndex() {
  const dbPath = arg("db", DEFAULT_DB)!;
  const limit = Number(arg("limit", "0"));
  const t0 = performance.now();

  const files = discover();
  const picked = limit > 0 ? files.slice(0, limit) : files;

  const db = open(dbPath, true);
  db.exec("DELETE FROM conversation; DELETE FROM message;");
  db.exec("DELETE FROM fts_prose; DELETE FROM fts_tools;");

  // OR IGNORE, not OR REPLACE: several files can carry the same sessionId (a session
  // and its subagent sidechains). Replacing would orphan the FTS rows keyed on the old
  // rowid. Ignoring keeps message and index in lockstep; duplicates are genuinely
  // the same message.
  // Field-level merge, not first-write-wins: a conversation can be spread over several
  // files (a session plus its subagent sidechains, or a resumed rollout) and only some
  // of them carry the title, cwd or model.
  const insConv = db.prepare(`INSERT INTO conversation
    (id, source, native_id, title, cwd, git_branch, model, started_at, ended_at,
     msg_count, prose_count, forked_from, resume_cmd)
    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)
    ON CONFLICT(id) DO UPDATE SET
      title       = COALESCE(conversation.title, excluded.title),
      cwd         = COALESCE(conversation.cwd, excluded.cwd),
      git_branch  = COALESCE(conversation.git_branch, excluded.git_branch),
      model       = COALESCE(conversation.model, excluded.model),
      forked_from = COALESCE(conversation.forked_from, excluded.forked_from)`);
  const insMsg = db.prepare(`INSERT OR IGNORE INTO message
    (id, conv_id, parent_id, seq, role, kind, ts, is_sidechain, text)
    VALUES (?,?,?,?,?,?,?,?,?)`);
  const insProse = db.prepare("INSERT INTO fts_prose(rowid, text) VALUES (?,?)");
  const insTools = db.prepare("INSERT INTO fts_tools(rowid, text) VALUES (?,?)");

  let convs = 0, msgs = 0, prose = 0, failed = 0, bytes = 0, dupes = 0;
  const bySource: Record<string, number> = {};

  db.exec("BEGIN");
  for (const f of picked) {
    let c: Conv | null = null;
    try {
      c = f.source === "codex" ? importCodex(f.path) : importClaudeCode(f.path);
    } catch {
      failed++;
      continue;
    }
    if (!c) { failed++; continue; } // no session id or no usable messages

    const convId = `${c.source}:${c.nativeId}`;
    // counts and time bounds are recomputed by aggregate after the load, so that
    // multiple files feeding one conversation roll up correctly
    insConv.run(
      convId, c.source, c.nativeId, c.title, c.cwd, c.gitBranch, c.model, null, null,
      0, 0, c.forkedFrom, c.resumeCmd,
    );

    for (const m of c.messages) {
      const res = insMsg.run(
        `${convId}:${m.nativeId}`, convId,
        m.parentId ? `${convId}:${m.parentId}` : null,
        m.seq, m.role, m.kind, m.ts, m.isSidechain ? 1 : 0, m.text,
      );
      if (res.changes === 0) { dupes++; continue; } // already indexed
      const rowid = Number(res.lastInsertRowid);
      if (m.kind === "prose") {
        insProse.run(rowid, m.text);
        prose++;
      } else if (m.kind === "tool_call" || m.kind === "tool_result") {
        insTools.run(rowid, m.text);
      }
      bytes += m.text.length;
      msgs++;
    }

    convs++;
    bySource[c.source] = (bySource[c.source] ?? 0) + 1;
  }

  db.exec(`UPDATE conversation SET
      msg_count   = (SELECT COUNT(*) FROM message WHERE conv_id = conversation.id),
      prose_count = (SELECT COUNT(*) FROM message WHERE conv_id = conversation.id AND kind='prose'),
      started_at  = (SELECT MIN(ts)  FROM message WHERE conv_id = conversation.id),
      ended_at    = (SELECT MAX(ts)  FROM message WHERE conv_id = conversation.id)`);
  db.exec("COMMIT");
  db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
  db.close();

  console.log(JSON.stringify({
    impl: "ts", db: dbPath, files_seen: files.length, files_indexed: picked.length,
    failed, conversations: convs, messages: msgs, prose, dupes, text_bytes: bytes,
    by_source: bySource, ms: Math.round(performance.now() - t0),
  }, null, 2));
}

function cmdSearch() {
  const q = process.argv[3];
  if (!q || q.startsWith("--")) {
    console.error("usage: search <query> [--db PATH] [--limit N] [--tools]");
    process.exit(2);
  }
  const dbPath = arg("db", DEFAULT_DB)!;
  const limit = Number(arg("limit", "10"));
  const t0 = performance.now();
  const db = open(dbPath, false);
  const results = search(db, q, limit, has("tools"));
  const ms = performance.now() - t0;
  db.close();
  console.log(JSON.stringify({ impl: "ts", query: q, ms: +ms.toFixed(2), results }, null, 2));
}

function cmdStats() {
  const db = open(arg("db", DEFAULT_DB)!, false);
  const one = (sql: string) => (db.prepare(sql).get() as any);
  const bySource = db.prepare(
    "SELECT source, COUNT(*) n FROM conversation GROUP BY source").all() as any[];
  const byKind = db.prepare(
    "SELECT kind, COUNT(*) n FROM message GROUP BY kind").all() as any[];
  console.log(JSON.stringify({
    impl: "ts",
    conversations: one("SELECT COUNT(*) n FROM conversation").n,
    messages: one("SELECT COUNT(*) n FROM message").n,
    by_source: Object.fromEntries(bySource.map((r) => [r.source, r.n])),
    by_kind: Object.fromEntries(byKind.map((r) => [r.kind, r.n])),
  }, null, 2));
  db.close();
}

switch (process.argv[2]) {
  case "index": cmdIndex(); break;
  case "search": cmdSearch(); break;
  case "stats": cmdStats(); break;
  default:
    console.error("usage: <index|search|stats> [...]");
    process.exit(2);
}
