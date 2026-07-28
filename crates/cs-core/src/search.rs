use rusqlite::{params, Connection};
use serde::Serialize;

const YEAR_MS: f64 = 365.0 * 24.0 * 60.0 * 60.0 * 1000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Prose,
    Tools,
}

impl Field {
    fn table(self) -> &'static str {
        match self {
            Field::Prose => "fts_prose",
            Field::Tools => "fts_tools",
        }
    }
}

/// One search result. These field names are a contract: a Raycast extension or GUI consumes
/// this JSON verbatim, so changes should be additive (ADR 12).
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub conv_id: String,
    pub msg_id: String,
    pub source: String,
    pub title: Option<String>,
    pub role: String,
    pub kind: String,
    pub ts: Option<i64>,
    pub score: f64,
    pub snippet: String,
    pub resume_cmd: Option<String>,
    /// False when the message sits on a branch that was edited away — still searchable,
    /// but not part of the conversation as currently displayed.
    pub on_head_path: bool,
    pub is_sidechain: bool,
    pub thread_key: String,
    pub deleted_upstream: bool,
}

/// Raw user input is not a valid FTS5 MATCH expression — a bare `-`, `*` or quote is a
/// syntax error. Tokenise and re-quote each term; implicit AND between them.
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

    let chars: Vec<char> = flat.chars().collect();
    let start = match at {
        None => 0,
        Some(byte_at) => flat[..byte_at].chars().count().saturating_sub(width / 3),
    };
    let end = (start + width).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    )
}

pub struct Query<'a> {
    pub text: &'a str,
    pub limit: i64,
    pub field: Field,
    pub source: Option<&'a str>,
    /// Include messages on branches that were edited away.
    pub include_off_path: bool,
    pub now_ms: i64,
}

impl<'a> Query<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            limit: 10,
            field: Field::Prose,
            source: None,
            include_off_path: false,
            now_ms: 0,
        }
    }
}

pub fn search(conn: &Connection, q: &Query) -> rusqlite::Result<Vec<Hit>> {
    let table = q.field.table();
    // bm25() and MATCH need the literal table name — an alias raises "no such column".
    let sql = format!(
        "SELECT m.id, m.conv_id, m.role, m.kind, m.ts, m.text, m.on_head_path,
                m.is_sidechain, m.thread_key,
                c.source, c.title, c.resume_cmd, c.deleted_upstream_at,
                bm25({table}) * (1.0 + 0.3 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         WHERE {table} MATCH ?2
           AND (?3 = 1 OR m.on_head_path = 1)
           AND (?4 IS NULL OR c.source = ?4)
         ORDER BY score
         LIMIT ?5"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![q.now_ms, to_match_expr(q.text), q.include_off_path as i64, q.source, q.limit],
        |r| {
            let text: String = r.get(5)?;
            Ok(Hit {
                msg_id: r.get(0)?,
                conv_id: r.get(1)?,
                role: r.get(2)?,
                kind: r.get(3)?,
                ts: r.get(4)?,
                snippet: snippet(&text, q.text, 160),
                on_head_path: r.get::<_, i64>(6)? != 0,
                is_sidechain: r.get::<_, i64>(7)? != 0,
                thread_key: r.get(8)?,
                source: r.get(9)?,
                title: r.get(10)?,
                resume_cmd: r.get(11)?,
                deleted_upstream: r.get::<_, Option<i64>>(12)?.is_some(),
                score: r.get(13)?,
            })
        },
    )?;
    rows.collect()
}

/// Why a conversation did *not* come back for a query.
///
/// A false negative has two very different causes — the text was never indexed, or it was
/// indexed and ranked too low — and they need opposite fixes. Guessing between them is the
/// slowest part of tuning ranking, so the index answers it directly.
#[derive(Debug, Serialize)]
pub struct Explain {
    pub conv_id: String,
    pub exists: bool,
    pub messages: i64,
    pub prose_messages: i64,
    pub indexed_prose: i64,
    pub off_path_messages: i64,
    pub deleted_upstream: bool,
    /// Per query term: how many prose messages in this conversation contain it at all.
    pub term_hits: Vec<(String, i64)>,
    /// Best score this conversation achieves for the query, if any message matches.
    pub best_score: Option<f64>,
    pub best_rank: Option<usize>,
    pub verdict: String,
}

pub fn explain(conn: &Connection, conv_id: &str, query: &str) -> rusqlite::Result<Explain> {
    let exists: bool =
        conn.query_row("SELECT COUNT(*) FROM conversation WHERE id=?1", params![conv_id], |r| {
            Ok(r.get::<_, i64>(0)? > 0)
        })?;

    let (messages, prose_messages, off_path): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(kind='prose'),0),
                COALESCE(SUM(on_head_path=0),0)
         FROM message WHERE conv_id=?1",
        params![conv_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;

    let indexed_prose: i64 = conn.query_row(
        "SELECT COUNT(*) FROM fts_prose f JOIN message m ON m.rowid=f.rowid WHERE m.conv_id=?1",
        params![conv_id],
        |r| r.get(0),
    )?;

    let deleted_upstream: bool = conn.query_row(
        "SELECT deleted_upstream_at IS NOT NULL FROM conversation WHERE id=?1",
        params![conv_id],
        |r| r.get(0),
    ).unwrap_or(false);

    // Per-term presence via LIKE rather than MATCH: this deliberately bypasses the
    // tokenizer, so a term the stemmer mangled still shows up as present in the text.
    let mut term_hits = Vec::new();
    for term in query.split_whitespace() {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM message WHERE conv_id=?1 AND kind='prose'
               AND lower(text) LIKE '%' || lower(?2) || '%'",
            params![conv_id, term],
            |r| r.get(0),
        )?;
        term_hits.push((term.to_string(), n));
    }

    let ranked = search(
        conn,
        &Query { text: query, limit: 500, include_off_path: true, ..Query::new(query) },
    )?;
    let best_rank = ranked.iter().position(|h| h.conv_id == conv_id);
    let best_score = best_rank.map(|i| ranked[i].score);

    let verdict = if !exists {
        "not in the index — the importer never produced this conversation".into()
    } else if indexed_prose == 0 {
        "conversation exists but has no indexed prose — all of it is tool traffic".into()
    } else if term_hits.iter().all(|(_, n)| *n == 0) {
        "no message contains any query term — this is a recall problem, not ranking".into()
    } else if best_rank.is_none() {
        "terms appear in the text but FTS did not match — likely tokenizer or stemming".into()
    } else if off_path > 0 && best_rank.is_some() {
        format!("matched at rank {} (searching off-path too)", best_rank.unwrap() + 1)
    } else {
        format!("matched at rank {} — this is a ranking problem", best_rank.unwrap() + 1)
    };

    Ok(Explain {
        conv_id: conv_id.to_string(),
        exists,
        messages,
        prose_messages,
        indexed_prose,
        off_path_messages: off_path,
        deleted_upstream,
        term_hits,
        best_score,
        best_rank: best_rank.map(|i| i + 1),
        verdict,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_expr_survives_punctuation_that_would_be_a_syntax_error() {
        assert_eq!(to_match_expr("foo bar"), r#""foo" "bar""#);
        assert_eq!(to_match_expr("rust -- borrow*"), r#""rust" "borrow""#);
        assert_eq!(to_match_expr(r#"say "hi""#), r#""say" "hi""#);
        assert_eq!(to_match_expr("   "), "");
    }

    #[test]
    fn snippet_centres_on_the_first_matching_term() {
        let text = "a".repeat(300) + " needle " + &"b".repeat(300);
        let s = snippet(&text, "needle", 60);
        assert!(s.contains("needle"), "got: {s}");
        assert!(s.starts_with('…') && s.ends_with('…'));
    }

    #[test]
    fn snippet_is_char_safe_on_multibyte_text() {
        let s = snippet("héllo wörld ünïcode", "wörld", 10);
        assert!(s.contains("wörld"));
    }
}
