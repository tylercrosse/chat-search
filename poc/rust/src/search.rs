use rusqlite::{Connection, Result};
use serde_json::{json, Value};

const YEAR_MS: f64 = 365.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// Raw user input is not a valid FTS5 MATCH expression (bare `-`, `*`, quotes all
/// blow up). Tokenise and re-quote each term; implicit AND between them.
pub fn to_match_expr(query: &str) -> String {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn snippet(text: &str, query: &str, width: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = flat.to_lowercase();
    let at = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .filter_map(|t| lower.find(t))
        .min();

    // operate on char boundaries; byte slicing panics on multi-byte UTF-8
    let chars: Vec<char> = flat.chars().collect();
    let start_char = match at {
        None => 0,
        Some(byte_at) => {
            let ci = flat[..byte_at].chars().count();
            ci.saturating_sub(width / 3)
        }
    };
    let end_char = (start_char + width).min(chars.len());
    let body: String = chars[start_char..end_char].iter().collect();
    format!(
        "{}{}{}",
        if start_char > 0 { "…" } else { "" },
        body,
        if end_char < chars.len() { "…" } else { "" }
    )
}

pub fn search(
    conn: &Connection,
    query: &str,
    limit: i64,
    tools: bool,
    now: i64,
) -> Result<Vec<Value>> {
    let table = if tools { "fts_tools" } else { "fts_prose" };
    // bm25()/MATCH need the literal FTS table name — an alias raises "no such column".
    let sql = format!(
        "SELECT m.id AS msg_id, m.conv_id, m.role, m.kind, m.ts, m.text,
                c.source, c.title, c.resume_cmd,
                bm25({table}) * (1.0 + 0.3 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
         FROM {table}
         JOIN message m ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         WHERE {table} MATCH ?2
         ORDER BY score
         LIMIT ?3"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params![now, to_match_expr(query), limit],
        |r| {
            let text: String = r.get("text")?;
            Ok(json!({
                "conv_id": r.get::<_, String>("conv_id")?,
                "msg_id": r.get::<_, String>("msg_id")?,
                "source": r.get::<_, String>("source")?,
                "title": r.get::<_, Option<String>>("title")?,
                "role": r.get::<_, String>("role")?,
                "kind": r.get::<_, String>("kind")?,
                "ts": r.get::<_, Option<i64>>("ts")?,
                "score": r.get::<_, f64>("score")?,
                "snippet": snippet(&text, query, 160),
                "resume_cmd": r.get::<_, Option<String>>("resume_cmd")?,
            }))
        },
    )?;
    rows.collect()
}
