import type { DatabaseSync } from "node:sqlite";

const YEAR_MS = 365 * 24 * 60 * 60 * 1000;

/**
 * Raw user input is not a valid FTS5 MATCH expression (bare `-`, `*`, quotes all
 * blow up). Tokenise and re-quote each term; implicit AND between them.
 */
export function toMatchExpr(query: string): string {
  const terms = query
    .split(/[^\p{L}\p{N}_]+/u)
    .filter((t) => t.length > 0)
    .map((t) => `"${t.replace(/"/g, '""')}"`);
  return terms.join(" ");
}

export function snippet(text: string, query: string, width = 160): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const terms = query.toLowerCase().split(/[^\p{L}\p{N}_]+/u).filter(Boolean);
  const lower = flat.toLowerCase();
  let at = -1;
  for (const t of terms) {
    const i = lower.indexOf(t);
    if (i !== -1 && (at === -1 || i < at)) at = i;
  }
  if (at === -1) return flat.slice(0, width) + (flat.length > width ? "…" : "");
  const start = Math.max(0, at - width / 3);
  const end = Math.min(flat.length, start + width);
  return (start > 0 ? "…" : "") + flat.slice(start, end) + (end < flat.length ? "…" : "");
}

export interface Result {
  conv_id: string;
  msg_id: string;
  source: string;
  title: string | null;
  role: string;
  kind: string;
  ts: number | null;
  score: number;
  snippet: string;
  resume_cmd: string | null;
}

export function search(
  db: DatabaseSync,
  query: string,
  limit: number,
  tools: boolean,
): Result[] {
  const table = tools ? "fts_tools" : "fts_prose";
  const now = Date.now();
  // bm25()/MATCH need the literal FTS table name — an alias raises "no such column".
  const sql = `
    SELECT m.id AS msg_id, m.conv_id, m.role, m.kind, m.ts, m.text,
           c.source, c.title, c.resume_cmd,
           bm25(${table}) * (1.0 + 0.3 * (max(0, ? - ifnull(m.ts, ?)) / ${YEAR_MS}.0)) AS score
    FROM ${table}
    JOIN message m ON m.rowid = ${table}.rowid
    JOIN conversation c ON c.id = m.conv_id
    WHERE ${table} MATCH ?
    ORDER BY score
    LIMIT ?`;
  const rows = db.prepare(sql).all(now, now, toMatchExpr(query), limit) as any[];
  return rows.map((r) => ({
    conv_id: r.conv_id,
    msg_id: r.msg_id,
    source: r.source,
    title: r.title,
    role: r.role,
    kind: r.kind,
    ts: r.ts,
    score: r.score,
    snippet: snippet(r.text, query),
    resume_cmd: r.resume_cmd,
  }));
}
