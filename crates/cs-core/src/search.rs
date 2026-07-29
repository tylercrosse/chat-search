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
    to_match_expr_opts(query, false)
}

/// Shortest token that may be expanded into a prefix match.
///
/// A one- or two-character prefix matches a large fraction of the corpus, and BM25 has to
/// score *every* matching row before it can sort, so the cost is in ranking rather than
/// lookup and no index fixes it. Measured on 40k prose messages: `h*` 2510ms, `ho*` 51ms,
/// `hov*` 16ms, `hove*` 6ms. Below this length the token is matched exactly instead, which
/// is what typeahead UIs do anyway — results simply start appearing at the third keystroke.
pub const MIN_PREFIX_LEN: usize = 3;

/// With `prefix`, the *final* token becomes a prefix match — the typeahead shape, where
/// every completed word is exact and only the one still being typed is open-ended.
/// A trailing separator means the last word is finished, so no prefix is applied.
pub fn to_match_expr_opts(query: &str, prefix: bool) -> String {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| t.replace('"', "\"\""))
        .collect();
    let ends_open = query
        .chars()
        .last()
        .is_some_and(|c| c.is_alphanumeric() || c == '_');
    let last = terms.len().saturating_sub(1);
    terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if prefix && ends_open && i == last && t.chars().count() >= MIN_PREFIX_LEN {
                format!("\"{t}\"*")
            } else {
                format!("\"{t}\"")
            }
        })
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
    /// Treat the final token as a prefix, for typeahead.
    pub prefix: bool,
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
            prefix: false,
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
                bm25({table}) / (1.0 + 0.3 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
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
        params![q.now_ms, to_match_expr_opts(q.text, q.prefix), q.include_off_path as i64, q.source, q.limit],
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

// ---------------------------------------------------------------- grouping

/// A conversation and its best matching messages — the Algolia DocSearch shape: the
/// conversation is the result, matching messages nest beneath it.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub conv_id: String,
    pub source: String,
    pub title: Option<String>,
    pub ended_at: Option<i64>,
    /// What a human would call a turn: user prose, not raw message count.
    pub user_turns: i64,
    pub score: f64,
    /// Total matching messages, including any not shown.
    pub match_count: usize,
    pub resume_cmd: Option<String>,
    pub deleted_upstream: bool,
    pub hits: Vec<Hit>,
}

/// How much a conversation's *additional* matches count beyond its best one.
///
/// 0.0 would be pure max — one strong hit is the whole score, and a conversation that
/// returns to a topic ten times ranks no higher. 1.0 would be pure sum, which lets long
/// agent sessions win on volume alone. 0.25 keeps the best hit leading while letting
/// sustained discussion break ties.
pub const REPEAT_WEIGHT: f64 = 0.25;

/// Conversations matching `q`, best-first, each carrying up to `nested` messages.
///
/// Scores are pulled ungrouped and folded here rather than in SQL: FTS5 auxiliary functions
/// like bm25() cannot be used through a CTE or subquery, so the grouping has to happen after
/// the rows come back.
pub fn search_grouped(
    conn: &Connection,
    q: &Query,
    nested: usize,
) -> rusqlite::Result<Vec<Group>> {
    // Pull well beyond `limit` conversations' worth of messages, since one conversation can
    // account for many hits and would otherwise crowd out the tail.
    let ceiling = (q.limit * 50).clamp(500, 5_000);
    let hits = search(conn, &Query { limit: ceiling, ..*q })?;

    let mut order: Vec<String> = Vec::new();
    let mut by_conv: std::collections::HashMap<String, Vec<Hit>> = std::collections::HashMap::new();
    for h in hits {
        if !by_conv.contains_key(&h.conv_id) {
            order.push(h.conv_id.clone());
        }
        by_conv.entry(h.conv_id.clone()).or_default().push(h);
    }

    let mut groups = Vec::with_capacity(order.len());
    for conv_id in order {
        let mut hits = by_conv.remove(&conv_id).unwrap_or_default();
        // bm25 is negative and better is more negative, so the best hit is the minimum.
        let best = hits.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let total: f64 = hits.iter().map(|h| h.score).sum();
        let score = best + REPEAT_WEIGHT * (total - best);

        let first = hits.first().cloned();
        let match_count = hits.len();
        hits.truncate(nested);

        groups.push(Group {
            conv_id,
            source: first.as_ref().map(|h| h.source.clone()).unwrap_or_default(),
            title: first.as_ref().and_then(|h| h.title.clone()),
            ended_at: None,
            user_turns: 0,
            score,
            match_count,
            resume_cmd: first.as_ref().and_then(|h| h.resume_cmd.clone()),
            deleted_upstream: first.as_ref().is_some_and(|h| h.deleted_upstream),
            hits,
        });
    }

    // Ties broken by conv_id so the order is stable across runs — results that reshuffle
    // between identical queries read as a bug even when the ranking is the same.
    groups.sort_by(|a, b| {
        a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.conv_id.cmp(&b.conv_id))
    });
    groups.truncate(q.limit as usize);

    // Conversation metadata is fetched only for the groups actually being returned. Doing it
    // during the fold costs one query per *matching* conversation, which a broad typeahead
    // prefix like "h" turns into thousands — measured at 2.5s before this was moved.
    let mut stmt =
        conn.prepare_cached("SELECT ended_at, user_turns FROM conversation WHERE id = ?1")?;
    for g in &mut groups {
        if let Ok((ended_at, user_turns)) =
            stmt.query_row(params![g.conv_id], |r| Ok((r.get(0)?, r.get(1)?)))
        {
            g.ended_at = ended_at;
            g.user_turns = user_turns;
        }
    }
    Ok(groups)
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn prefix_applies_only_to_an_unfinished_final_token() {
        assert_eq!(to_match_expr_opts("borrow check", true), r#""borrow" "check"*"#);
        // trailing space means that word is finished
        assert_eq!(to_match_expr_opts("borrow check ", true), r#""borrow" "check""#);
        // below the floor, matched exactly: a 1-2 char prefix matches most of the corpus
        // and BM25 must score every row before sorting
        assert_eq!(to_match_expr_opts("ho", true), r#""ho""#);
        assert_eq!(to_match_expr_opts("hov", true), r#""hov"*"#);
        // off by default, so ordinary search is unaffected
        assert_eq!(to_match_expr_opts("borrow check", false), r#""borrow" "check""#);
    }

    #[test]
    fn damping_sits_between_best_hit_and_total() {
        // bm25 is negative; better is more negative
        let damped = |ss: &[f64]| {
            let best = ss.iter().cloned().fold(f64::INFINITY, f64::min);
            let total: f64 = ss.iter().sum();
            best + REPEAT_WEIGHT * (total - best)
        };
        let once = [-6.0];
        let four_times = [-6.0, -6.0, -6.0, -6.0];

        // repeated discussion of the same strength improves a conversation's score...
        assert!(damped(&four_times) < damped(&once), "repetition should help");
        // ...but far less than raw sum, which is what stops long sessions winning on volume
        assert!(damped(&four_times) > four_times.iter().sum::<f64>());
        // and a single strong hit still outranks several mediocre ones
        assert!(damped(&[-11.0]) < damped(&four_times));
        // pure max would ignore repetition entirely
        assert_eq!(damped(&once), once[0]);
    }

    #[test]
    fn age_lowers_a_score_rather_than_raising_it() {
        // The decay divides. Multiplying — which is what this did until it was measured —
        // makes an old negative score *more* negative and therefore rank higher.
        let bm25 = -10.0_f64;
        let decayed = |age_years: f64| bm25 / (1.0 + 0.3 * age_years);
        assert!(decayed(3.0) > decayed(0.0), "older must rank lower, not higher");
        assert!((decayed(0.0) - bm25).abs() < f64::EPSILON);
    }
}
