use rusqlite::{params, Connection};
use serde::Serialize;

const YEAR_MS: f64 = 365.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// How hard age pushes a result down: a score is divided by `1 + DECAY * age_in_years`.
///
/// It divides rather than multiplies because bm25 is negative — multiplying makes an old
/// score *more* negative and therefore ranks it higher, which is what this did until it was
/// measured. At 0.3, a year-old conversation needs roughly a 30% better bm25 to hold its
/// place against a fresh one, and a three-year-old one nearly twice as good.
///
/// Settable per query via [`Query::decay`] so the eval harness can pool candidates from a
/// recency-blind ranker as well as this one — judging only what today's constants return is
/// how a tuning run learns that today's constants are optimal (chat-search-6eb.13).
pub const DECAY: f64 = 0.3;

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
    /// See [`REPEAT_WEIGHT`], which is the default.
    pub repeat_weight: f64,
    /// See [`DECAY`], which is the default.
    pub decay: f64,
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
            repeat_weight: REPEAT_WEIGHT,
            decay: DECAY,
        }
    }
}

/// True when a query carries no searchable term — empty, whitespace, or only punctuation.
///
/// Checked against the *tokenised* form rather than the raw string, because `"-"` and `"??"`
/// are non-empty input that still produce no terms, and an empty FTS5 MATCH expression is a
/// syntax error rather than an empty result.
pub fn is_blank(query: &str) -> bool {
    to_match_expr_opts(query, false).is_empty()
}

pub fn search(conn: &Connection, q: &Query) -> rusqlite::Result<Vec<Hit>> {
    if is_blank(q.text) {
        return Ok(Vec::new());
    }
    let table = q.field.table();
    // bm25() and MATCH need the literal table name — an alias raises "no such column".
    let sql = format!(
        "SELECT m.id, m.conv_id, m.role, m.kind, m.ts, m.text, m.on_head_path,
                m.is_sidechain, m.thread_key,
                c.source, c.title, c.resume_cmd, c.deleted_upstream_at,
                bm25({table}) / (1.0 + ?6 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
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
        params![q.now_ms, to_match_expr_opts(q.text, q.prefix), q.include_off_path as i64, q.source, q.limit, q.decay],
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
    /// `ended_at` as a local `YYYY-MM-DD`, rendered here so no client re-derives it.
    ///
    /// Redundant with `ended_at` on purpose: every client wants the day, and each one that
    /// computes it from the epoch value gets its own chance to compute it in UTC and name
    /// tomorrow for an evening conversation — which is precisely how cs-fzf's jq and the
    /// binary's formatter came to disagree.
    pub ended_date: Option<String>,
    /// What a human would call a turn: user prose, not raw message count.
    pub user_turns: i64,
    /// Every message, tool traffic included. The denominator [`Group::match_seqs`] positions
    /// are relative to.
    pub msg_count: i64,
    /// Prose messages only, which is what a prose search could possibly have matched.
    pub prose_count: i64,
    /// Working directory the conversation ran in, for sources that have one.
    ///
    /// `None` for every ChatGPT conversation, which is 2,011 of the corpus — a client must
    /// render that as "this source has no such thing" rather than as missing data
    /// (chat-search-6eb.26). Derived, so ADR 16 forbids it ever reaching an id.
    pub cwd: Option<String>,
    pub score: f64,
    /// Total matching messages, including any not shown.
    pub match_count: usize,
    /// Where the matches sit, as 0-based message positions, ascending.
    ///
    /// Three matches at the start of a forty-message conversation and three at the very end
    /// mean different things — the topic, or an aside someone raised on the way out. A long
    /// conversation covering several subjects is common enough here that a match count alone
    /// cannot answer "is this conversation about my query": 58% of conversations over 15
    /// turns span more than four hours, so most of them are several sittings.
    pub match_seqs: Vec<i64>,
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
///
/// Settable per query via [`Query::repeat_weight`]; see [`DECAY`] for why.
pub const REPEAT_WEIGHT: f64 = 0.25;

/// Most recently active conversations, carrying no nested hits.
///
/// A typeahead UI has to draw something before the first keystroke, and below
/// [`MIN_PREFIX_LEN`] a token is matched exactly rather than as a prefix, so one or two
/// characters return noise rather than a narrowing list. Recency is the only ranking
/// available without a query and it is also the one people expect — the conversation you
/// want is usually among the last few you had.
pub fn recent(conn: &Connection, q: &Query) -> rusqlite::Result<Vec<Group>> {
    let mut stmt = conn.prepare(
        "SELECT id, source, title, ended_at, user_turns, resume_cmd, deleted_upstream_at,
                msg_count, prose_count, cwd
         FROM conversation
         WHERE (?1 IS NULL OR source = ?1)
         -- `NULLS LAST` is not portable to older SQLite; this expresses the same order.
         ORDER BY ended_at IS NULL, ended_at DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![q.source, q.limit], |r| {
        let ended_at: Option<i64> = r.get(3)?;
        Ok(Group {
            conv_id: r.get(0)?,
            source: r.get(1)?,
            title: r.get(2)?,
            ended_at,
            ended_date: ended_at.and_then(crate::time::local_ymd),
            user_turns: r.get(4)?,
            msg_count: r.get(7)?,
            prose_count: r.get(8)?,
            cwd: r.get(9)?,
            score: 0.0,
            match_count: 0,
            // No query, so nothing matched and there is no shape to draw.
            match_seqs: Vec::new(),
            resume_cmd: r.get(5)?,
            deleted_upstream: r.get::<_, Option<i64>>(6)?.is_some(),
            hits: Vec::new(),
        })
    })?;
    rows.collect()
}

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
    if is_blank(q.text) {
        return recent(conn, q);
    }
    // Pull well beyond `limit` conversations' worth of messages, since one conversation can
    // account for many hits and would otherwise crowd out the tail.
    let ceiling = (q.limit * 50).clamp(500, 5_000);

    // Ranking reads no message text. `message.text` is by far the widest column, and a broad
    // prefix puts thousands of rows through this query while only `limit * nested` of them are
    // ever shown — so text is fetched in `hydrate` afterwards, for the survivors only.
    // Warm, at limit=200, against building a snippet per candidate: "the" 432ms -> 79ms,
    // "cod" 380ms -> 32ms, "code" 469ms -> 98ms.
    let table = q.field.table();
    let sql = format!(
        "SELECT m.id, m.conv_id, m.seq,
                bm25({table}) / (1.0 + ?6 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         WHERE {table} MATCH ?2
           AND (?3 = 1 OR m.on_head_path = 1)
           AND (?4 IS NULL OR c.source = ?4)
         ORDER BY score
         LIMIT ?5"
    );
    // Bound rather than interpolated: a decay swept across a range would otherwise mint a
    // new statement per value and defeat the cache this call relies on.
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(
        params![
            q.now_ms,
            to_match_expr_opts(q.text, q.prefix),
            q.include_off_path as i64,
            q.source,
            ceiling,
            q.decay
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, f64>(3)?,
            ))
        },
    )?;

    let mut order: Vec<String> = Vec::new();
    let mut by_conv: std::collections::HashMap<String, Vec<(String, i64, f64)>> = Default::default();
    for row in rows {
        let (msg_id, conv_id, seq, score) = row?;
        if !by_conv.contains_key(&conv_id) {
            order.push(conv_id.clone());
        }
        by_conv.entry(conv_id).or_default().push((msg_id, seq, score));
    }

    let mut ranked: Vec<Ranked> = Vec::with_capacity(order.len());
    for conv_id in order {
        let hits = by_conv.remove(&conv_id).unwrap_or_default();
        // bm25 is negative and better is more negative, so the best hit is the minimum.
        let best = hits.iter().map(|(_, _, s)| *s).fold(f64::INFINITY, f64::min);
        let total: f64 = hits.iter().map(|(_, _, s)| *s).sum();
        let score = best + q.repeat_weight * (total - best);
        let match_count = hits.len();
        // Every match's position, not just the shown ones: the point of the strip is the
        // shape of the whole conversation, which `nested` would otherwise truncate to three.
        let mut seqs: Vec<i64> = hits.iter().map(|(_, seq, _)| *seq).collect();
        seqs.sort_unstable();
        let shown = hits.into_iter().take(nested).map(|(id, _, _)| id).collect();
        ranked.push(Ranked { conv_id, score, match_count, seqs, shown });
    }

    // Ties broken by conv_id so the order is stable across runs — results that reshuffle
    // between identical queries read as a bug even when the ranking is the same.
    ranked.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.conv_id.cmp(&b.conv_id))
    });
    ranked.truncate(q.limit as usize);

    hydrate(conn, q, ranked)
}

/// One conversation that survived ranking, before its display columns are fetched.
struct Ranked {
    conv_id: String,
    score: f64,
    match_count: usize,
    seqs: Vec<i64>,
    /// Message ids to nest under the conversation, capped at `nested`.
    shown: Vec<String>,
}

/// Ten cells showing where in a conversation the matches fall, densest marked heaviest.
///
/// Rendered here rather than by each client for the same reason as [`Group::ended_date`]:
/// every surface wants it, and a second implementation is a second chance to get the
/// bucketing subtly different from the first.
pub fn match_density(seqs: &[i64], msg_count: i64) -> String {
    const CELLS: usize = 10;
    const LEVELS: [char; 4] = ['·', '▁', '▄', '█'];
    if seqs.is_empty() || msg_count <= 0 {
        return LEVELS[0].to_string().repeat(CELLS);
    }
    let mut buckets = [0usize; CELLS];
    for &s in seqs {
        // `seq` is 0-based and dense per conversation, so this is a straight proportion.
        // Clamped anyway: a seq at or past msg_count would otherwise index off the end.
        let b = ((s.max(0) as usize) * CELLS / (msg_count as usize).max(1)).min(CELLS - 1);
        buckets[b] += 1;
    }
    buckets
        .iter()
        .map(|&n| match n {
            0 => LEVELS[0],
            1 => LEVELS[1],
            2..=3 => LEVELS[2],
            _ => LEVELS[3],
        })
        .collect()
}

/// Turn ranked `(conv_id, score, match_count, shown message ids)` into full `Group`s.
///
/// Split out from ranking so the expensive columns — message text, and the snippet built from
/// it — are touched only for rows that survived. One prepared statement reused across the
/// survivors beats an `IN (...)` list, which would need rebuilding per query and lose the
/// statement cache.
fn hydrate(conn: &Connection, q: &Query, ranked: Vec<Ranked>) -> rusqlite::Result<Vec<Group>> {
    let mut meta = conn.prepare_cached(
        "SELECT source, title, resume_cmd, deleted_upstream_at, ended_at, user_turns,
                msg_count, prose_count, cwd
         FROM conversation WHERE id = ?1",
    )?;
    let mut msg = conn.prepare_cached(
        "SELECT m.id, m.conv_id, m.role, m.kind, m.ts, m.text, m.on_head_path,
                m.is_sidechain, m.thread_key,
                c.source, c.title, c.resume_cmd, c.deleted_upstream_at
         FROM message m JOIN conversation c ON c.id = m.conv_id
         WHERE m.id = ?1",
    )?;

    let mut groups = Vec::with_capacity(ranked.len());
    for Ranked { conv_id, score, match_count, seqs, shown } in ranked {
        let m = meta
            .query_row(params![conv_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<i64>>(3)?.is_some(),
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, Option<String>>(8)?,
                ))
            })
            .ok();

        let mut hits = Vec::with_capacity(shown.len());
        for id in shown {
            if let Ok(h) = msg.query_row(params![id], |r| {
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
                    score,
                })
            }) {
                hits.push(h);
            }
        }

        let ended_at = m.as_ref().and_then(|m| m.4);
        groups.push(Group {
            conv_id,
            source: m.as_ref().map(|m| m.0.clone()).unwrap_or_default(),
            title: m.as_ref().and_then(|m| m.1.clone()),
            resume_cmd: m.as_ref().and_then(|m| m.2.clone()),
            deleted_upstream: m.as_ref().is_some_and(|m| m.3),
            ended_at,
            ended_date: ended_at.and_then(crate::time::local_ymd),
            user_turns: m.as_ref().map(|m| m.5).unwrap_or(0),
            msg_count: m.as_ref().map(|m| m.6).unwrap_or(0),
            prose_count: m.as_ref().map(|m| m.7).unwrap_or(0),
            cwd: m.as_ref().and_then(|m| m.8.clone()),
            score,
            match_count,
            match_seqs: seqs,
            hits,
        });
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
    fn a_query_with_no_terms_is_blank_however_it_is_spelled() {
        for q in ["", "   ", "-", "??", "  *  "] {
            assert!(is_blank(q), "{q:?} yields no FTS terms, so it cannot be MATCHed");
        }
        assert!(!is_blank("hov"));
        assert!(!is_blank("a"));
    }

    #[test]
    fn a_blank_query_falls_back_to_recent_conversations() {
        let conn = crate::open(":memory:").unwrap();
        // ended_at descending, with the NULL sorted last rather than first
        for (id, ended) in [("c:a", Some(300i64)), ("c:b", Some(100)), ("c:z", None), ("c:c", Some(200))] {
            conn.execute(
                "INSERT INTO conversation(id, source, native_id, title, ended_at, user_turns)
                 VALUES (?1, 'codex', ?1, ?1, ?2, 3)",
                params![id, ended],
            )
            .unwrap();
        }

        let q = Query { limit: 10, ..Query::new("") };
        // An empty MATCH expression is a syntax error, so the point is that this does not
        // merely return nothing — it returns something useful, which is what a TUI opens on.
        let groups = search_grouped(&conn, &q, 3).unwrap();
        let ids: Vec<_> = groups.iter().map(|g| g.conv_id.as_str()).collect();
        assert_eq!(ids, ["c:a", "c:c", "c:b", "c:z"]);
        assert!(groups.iter().all(|g| g.hits.is_empty()), "no query means no hits to nest");
        // The day rides along with the epoch value on this path too — a typeahead UI opens
        // on `recent` before the first keystroke, so it is where a client would first be
        // tempted to derive the date itself.
        assert_eq!(groups[0].ended_date, crate::time::local_ymd(300));
        assert!(groups[3].ended_date.is_none(), "no ended_at, no day to name");

        // and the flat path stays empty rather than raising
        assert!(search(&conn, &q).unwrap().is_empty());
    }

    #[test]
    fn recent_respects_the_source_filter_and_limit() {
        let conn = crate::open(":memory:").unwrap();
        for (id, source) in [("a", "codex"), ("b", "claude-code"), ("c", "codex")] {
            conn.execute(
                "INSERT INTO conversation(id, source, native_id, ended_at, user_turns)
                 VALUES (?1, ?2, ?1, 1, 1)",
                params![id, source],
            )
            .unwrap();
        }
        let got = recent(&conn, &Query { limit: 10, source: Some("codex"), ..Query::new("") }).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|g| g.source == "codex"));

        let capped = recent(&conn, &Query { limit: 1, ..Query::new("") }).unwrap();
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn a_matched_conversation_carries_its_local_day_as_well_as_its_timestamp() {
        let conn = crate::open(":memory:").unwrap();
        // 2026-07-18 21:00 PDT: the evening case that used to be filed under the 19th. What
        // day that is depends on the machine's zone, so the assertion is that the rendered
        // field agrees with the one rule rather than a literal — time.rs pins a zone and
        // checks the rule itself.
        let ended = 1_784_433_600_000i64; // 2026-07-19T04:00Z
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, title, ended_at, user_turns)
             VALUES ('c:1', 'codex', 'n1', 'borrow checker', ?1, 2)",
            params![ended],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
             VALUES ('m:1', 'c:1', 'main', 1, 'user', 'prose', ?1, 'the borrow checker again')",
            params![ended],
        )
        .unwrap();
        // fts5 here is contentless, so postings are written explicitly rather than by trigger.
        let rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO fts_prose(rowid, text) VALUES (?1, 'the borrow checker again')",
            params![rowid],
        )
        .unwrap();

        let groups = search_grouped(&conn, &Query { limit: 5, ..Query::new("borrow") }, 3).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].ended_at, Some(ended));
        assert_eq!(groups[0].ended_date, crate::time::local_ymd(ended));
        assert!(groups[0].ended_date.is_some(), "an indexed conversation always has a day");
    }

    /// A conversation whose messages are `texts`, indexed and searchable.
    fn seed(conn: &Connection, conv_id: &str, ended: i64, texts: &[&str]) {
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, title, ended_at, user_turns)
             VALUES (?1, 'codex', ?1, ?1, ?2, 1)",
            params![conv_id, ended],
        )
        .unwrap();
        for (i, text) in texts.iter().enumerate() {
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
                 VALUES (?1, ?2, 'main', ?3, 'user', 'prose', ?4, ?5)",
                params![format!("{conv_id}:m{i}"), conv_id, i as i64, ended, text],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            conn.execute("INSERT INTO fts_prose(rowid, text) VALUES (?1, ?2)", params![rowid, text])
                .unwrap();
        }
    }

    #[test]
    fn the_density_strip_distinguishes_the_subject_from_an_aside() {
        // The whole reason it exists: same match count, same conversation length, and the
        // two deserve different grades because one is what the conversation was about and
        // the other is something raised on the way out.
        let subject = match_density(&[0, 1, 2, 3], 40);
        let aside = match_density(&[36, 37, 38, 39], 40);
        assert_ne!(subject, aside);
        assert!(subject.starts_with('█') && subject.ends_with('·'), "got {subject}");
        assert!(aside.starts_with('·') && aside.ends_with('█'), "got {aside}");
        assert_eq!(subject.chars().count(), 10);
    }

    #[test]
    fn the_strip_is_ten_cells_whatever_it_is_given() {
        // It is rendered into a fixed-width column, so a ragged one would break alignment.
        for (seqs, n) in [
            (vec![], 40),
            (vec![0], 1),
            (vec![0, 0, 0, 0, 0], 3),
            ((0..500).collect::<Vec<i64>>(), 500),
            (vec![-5, 9_999], 40), // out of range both ways: clamped, never panicking
            (vec![3], 0),          // a conversation claiming no messages
        ] {
            assert_eq!(match_density(&seqs, n).chars().count(), 10, "{seqs:?} of {n}");
        }
        assert_eq!(match_density(&[], 40), "··········", "nothing matched, nothing drawn");
    }

    #[test]
    fn repeat_weight_actually_moves_the_order_it_claims_to() {
        // Guards a sweep over a dead knob. `cs eval run --repeat-weight` would report a flat
        // line across every value if this stopped being plumbed through, and a flat line
        // reads as "the current value is fine" rather than as a broken experiment.
        let conn = crate::open(":memory:").unwrap();
        // One short, dense hit against four long, diluted ones — the exact contest the
        // constant exists to arbitrate.
        seed(&conn, "c:one-strong", 1_000, &["widget widget widget"]);
        let diluted = "widget among a great many other entirely unrelated filler words here";
        seed(&conn, "c:many-weak", 1_000, &[diluted, diluted, diluted, diluted]);

        let top = |w: f64| {
            let q = Query { limit: 10, repeat_weight: w, decay: 0.0, ..Query::new("widget") };
            search_grouped(&conn, &q, 1).unwrap()[0].conv_id.clone()
        };
        assert_eq!(top(0.0), "c:one-strong", "pure max: the single best hit is the whole score");
        assert_eq!(top(1.0), "c:many-weak", "pure sum: volume wins, which is what damping prevents");
    }

    #[test]
    fn the_decay_actually_moves_the_order_it_claims_to() {
        let conn = crate::open(":memory:").unwrap();
        let text = "widget";
        let year = 365 * 24 * 60 * 60 * 1000i64;
        // Identical text, three years apart. Ids chosen so the *older* one wins the
        // conv_id tiebreak, which is what makes the decay=0 case prove something: without
        // decay the two scores are equal and alphabetical order decides.
        seed(&conn, "c:a-old", 0, &[text]);
        seed(&conn, "c:b-new", 3 * year, &[text]);

        let top = |d: f64| {
            let q = Query {
                limit: 10, decay: d, now_ms: 3 * year, ..Query::new("widget")
            };
            search_grouped(&conn, &q, 1).unwrap()[0].conv_id.clone()
        };
        assert_eq!(top(0.0), "c:a-old", "recency-blind: equal scores, tie broken by id");
        assert_eq!(top(DECAY), "c:b-new", "three years of age is enough to lose the top slot");
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
