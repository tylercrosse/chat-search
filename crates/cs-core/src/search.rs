use crate::highlight;
use crate::query::{Facet, Query};
use rusqlite::{params, params_from_iter, types::Value, Connection};
use serde::Serialize;

const YEAR_MS: f64 = 365.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// How hard age pushes a result down: a score is divided by `1 + DECAY * age_in_years`.
///
/// It divides rather than multiplies because bm25 is negative — multiplying makes an old
/// score *more* negative and therefore ranks it higher, which is what this did until it was
/// measured. At 0.3, a year-old conversation needs roughly a 30% better bm25 to hold its
/// place against a fresh one, and a three-year-old one nearly twice as good.
///
/// Settable per query via [`SearchOptions::decay`] so the eval harness can pool candidates from a
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
    /// The source's own id for this conversation (ADR 2 makes it stable).
    pub native_id: String,
    /// Every way to reopen this conversation, best first.
    ///
    /// Resolved here rather than left to the client because a client reading `--json` cannot
    /// call [`crate::destinations`] — ADR 12 makes this object the contract, so the variants
    /// have to be *in* it. This replaces the single frozen `resume_cmd` string, which could
    /// hold one answer and went stale whenever a CLI changed its syntax (chat-search-me9.3).
    pub destinations: Vec<crate::Destination>,
    pub title: Option<String>,
    pub role: String,
    pub kind: String,
    pub ts: Option<i64>,
    pub score: f64,
    pub snippet: String,
    /// Byte offsets into `snippet` of the words that matched. Empty when the match could not
    /// be located, which is the same fact [`UNLOCATED`] states in prose.
    ///
    /// Carried rather than left to the client because no client can recover it: locating a
    /// stemmed match means asking the index's own tokenizer (see [`crate::highlight`]), and by
    /// the time a `Hit` exists the text has been windowed and its whitespace flattened, so
    /// even the offsets into the *message* would no longer fit. It costs nothing — every
    /// snippet is built from these already.
    pub snippet_spans: Vec<highlight::Span>,
    /// False when the message sits on a branch that was edited away — still searchable,
    /// but not part of the conversation as currently displayed.
    pub on_head_path: bool,
    pub is_sidechain: bool,
    pub thread_key: String,
    pub deleted_upstream: bool,
}

/// Shortest token that may be expanded into a prefix match **when it is the only term**.
///
/// A one- or two-character prefix matches a large fraction of the corpus, and BM25 has to
/// score *every* matching row before it can sort, so the cost is in ranking rather than
/// lookup and no index fixes it. Measured on 40k prose messages: `h*` 2510ms, `ho*` 51ms,
/// `hov*` 16ms, `hove*` 6ms.
///
/// The "only term" qualifier is load-bearing, and was missing until 2026-07-30. Applying the
/// floor to the final token of a *multi-term* query makes the result set **grow** as the
/// query gets longer, which is the opposite of what typing is for: `deep le` became
/// `"deep" "le"` — conversations containing the literal word "le", six of them — while one
/// more keystroke gave `"deep" "lea"*` and 845. The user reported it as "I'd expect longer
/// queries to reduce the result set", which is exactly right.
///
/// A preceding term also removes the reason for the floor. The cost it guards against is an
/// unbounded posting list, and an earlier term bounds it: measured on the 172k-message
/// index, `"le"*` alone matches 14,135 rows, while `"deep" "le"*` matches 845 in 6 ms and
/// `"deep" "l"*` 922 in 35 ms. So the floor applies only when there is nothing else to
/// intersect with.
pub const MIN_PREFIX_LEN: usize = 3;

/// Prefix on a snippet that is the head of the message rather than the text that matched.
///
/// The old behaviour was to return the head silently and without even the leading `…`, so a
/// stemmed hit rendered as a confident summary of a message whose matching word was nowhere
/// in view (chat-search-6eb.20). That is worse than useless here: the eval sheet shows one
/// snippet per candidate as the primary evidence for a grade, and the harvested pick log
/// records a vote cast on what the row showed — a wrong span silently becomes ground truth.
/// So the string says which of the two things it is, in the one channel a `String` has.
///
/// Rare by construction now: it takes a match this crate's own tokenizer cannot find in the
/// text it indexed, which today means fuzzy fallback (chat-search-6eb.10) or a stale index.
pub const UNLOCATED: &str = "⟨no match⟩ ";

/// A window of `text` around what `q` matched, with the offsets it computes anyway.
///
/// The spans index the *returned string*, ellipsis included, so nothing downstream can
/// re-derive them: the window has been cut out of the message and its whitespace flattened,
/// and the term that matched need not appear in the query (`commits` marks `Commit`). A
/// renderer that wants to bold the match therefore has to be handed them.
///
/// Empty and [`UNLOCATED`] are one statement made twice, for two audiences — the string says
/// it to a client reading JSON, the list to a client drawing cells — and they never disagree.
pub fn snippet_marked(text: &str, q: &Query, width: usize) -> (String, Vec<highlight::Span>) {
    let terms = q.marking_terms();
    let (out, spans) = highlight::snippet(text, &terms, width);
    if !spans.is_empty() {
        return (out, spans);
    }
    // Re-cut the head against the reduced budget so the label does not push the line over
    // `width`. No terms means no MATCH, so this is arithmetic, not a second query.
    let (head, _) = highlight::snippet(text, &[], width.saturating_sub(UNLOCATED.chars().count()));
    // Empty rather than `spans`, which is the same value today and would stop being one the
    // moment this branch is entered for any other reason: these offsets were taken before the
    // label was prepended, so each is short by its width and would mark the wrong word.
    (format!("{UNLOCATED}{head}"), Vec::new())
}

/// How to run a search, as distinct from what was asked. The query text itself moves to
/// [`crate::query::Query`]; what remains here is tuning the caller chooses.
pub struct SearchOptions {
    pub limit: i64,
    pub field: Field,
    /// Include messages on branches that were edited away.
    pub include_off_path: bool,
    /// The clock the recency decay is measured against. Required rather than defaulted —
    /// see [`SearchOptions::new`].
    pub now_ms: i64,
    /// See [`REPEAT_WEIGHT`], which is the default.
    pub repeat_weight: f64,
    /// See [`DECAY`], which is the default.
    pub decay: f64,
    /// How many matching messages each returned conversation carries. Formerly a positional
    /// argument to [`search_grouped`], which put one of the ten options somewhere the other
    /// nine could not be read from.
    pub nested: usize,
}

impl SearchOptions {
    /// `now_ms` is an argument rather than a defaulted field because zero does not mean
    /// "the epoch" here, it means "no decay at all": the score divisor is
    /// `1.0 + decay * max(0, now_ms - ts) / YEAR`, and with `now_ms` at zero the `max` is
    /// zero for every real timestamp, so the divisor is exactly 1.0. That silence cost
    /// [`explain`] its whole answer — it reported ranks from a recency-blind ranker while
    /// telling you a shipping one had a ranking problem.
    pub fn new(now_ms: i64) -> Self {
        Self {
            limit: 10,
            field: Field::Prose,
            include_off_path: false,
            now_ms,
            repeat_weight: REPEAT_WEIGHT,
            decay: DECAY,
            nested: 0,
        }
    }
}

/// Append `value` to the bind list and return the placeholder that reads it back.
fn bind(binds: &mut Vec<Value>, value: Value) -> String {
    binds.push(value);
    format!("?{}", binds.len())
}

/// A LIKE pattern matching `value` anywhere in a column, with any wildcards it contains
/// escaped.
///
/// `_` is LIKE's single-character wildcard and paths have underscores in them all the time,
/// so without this `dir:my_app` would also match `my-app` and `myXapp`.
fn like_contains(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for c in value.chars() {
        if matches!(c, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push('%');
    pattern
}

/// The `WHERE` fragment a query's filters add, with their values appended to `binds`.
///
/// Rendered here rather than in [`crate::query`] because it is the half of a filter that is
/// specific to *running* one: the parser decides what a token means, and every meaning it
/// accepts has a clause in this function. The two stay in step by construction — a filter
/// [`crate::query::Filter::is_active`] admits is one this emits, and `rejected()` is exactly
/// the complement, so there is no third list to keep synchronised.
///
/// Every fragment opens with `AND`, so a caller either has a condition already or starts from
/// `WHERE 1 = 1`. Values are bound rather than interpolated: `agent:codex` and `agent:claude`
/// then produce one cached statement instead of two, which is what keeps the typeahead's
/// prepared-statement cache useful.
fn filter_sql(query: &Query, now_ms: i64, binds: &mut Vec<Value>) -> String {
    let mut sql = String::new();

    // `agent:` is equality — a source id is an enum, and a substring match on one would make
    // `agent:claude` silently mean claude-code as well as claude-ai.
    let agents = query.selection(Facet::Agent);
    if !agents.include.is_empty() {
        let slots: Vec<String> =
            agents.include.iter().map(|v| bind(binds, Value::Text(v.clone()))).collect();
        sql.push_str(&format!("\n           AND c.source IN ({})", slots.join(", ")));
    }
    for value in &agents.exclude {
        let slot = bind(binds, Value::Text(value.clone()));
        sql.push_str(&format!("\n           AND c.source <> {slot}"));
    }

    // `dir:` is a case-insensitive substring, because a path is not an enum and nobody types
    // the whole of one. `ifnull` rather than a bare column so an undated-directory row is
    // dropped by an include and *kept* by an exclude: a conversation with no recorded cwd is
    // certainly not inside the directory being excluded, but `NULL NOT LIKE …` is unknown
    // rather than true and would drop it.
    let dirs = query.selection(Facet::Dir);
    if !dirs.include.is_empty() {
        let any: Vec<String> = dirs
            .include
            .iter()
            .map(|v| {
                let slot = bind(binds, Value::Text(like_contains(v)));
                format!("ifnull(lower(c.cwd), '') LIKE {slot} ESCAPE '\\'")
            })
            .collect();
        sql.push_str(&format!("\n           AND ({})", any.join(" OR ")));
    }
    for value in &dirs.exclude {
        let slot = bind(binds, Value::Text(like_contains(value)));
        sql.push_str(&format!(
            "\n           AND ifnull(lower(c.cwd), '') NOT LIKE {slot} ESCAPE '\\'"
        ));
    }

    // `date:` is a half-open window on when the conversation ended.
    for (window, negated) in query.date_windows(now_ms) {
        let mut bounds = Vec::new();
        if let Some(from) = window.from {
            bounds.push(format!("c.ended_at >= {}", bind(binds, Value::Integer(from))));
        }
        if let Some(until) = window.until {
            bounds.push(format!("c.ended_at < {}", bind(binds, Value::Integer(until))));
        }
        if bounds.is_empty() {
            continue;
        }
        let inside = format!("(c.ended_at IS NOT NULL AND {})", bounds.join(" AND "));
        // Negation wraps the whole test rather than flipping each comparison, which is what
        // makes an undated conversation survive `-date:today`: it is not in the window, so
        // it belongs in the complement. Flipped comparisons would leave it NULL and drop it.
        let clause = if negated { format!("NOT {inside}") } else { inside };
        sql.push_str(&format!("\n           AND {clause}"));
    }

    sql
}

pub fn search(conn: &Connection, query: &Query, q: &SearchOptions) -> rusqlite::Result<Vec<Hit>> {
    if !query.is_searchable() {
        return Ok(Vec::new());
    }
    let table = q.field.table();
    let mut binds = vec![
        Value::Integer(q.now_ms),
        Value::Text(query.match_expr()),
        Value::Integer(q.include_off_path as i64),
        Value::Integer(q.limit),
        Value::Real(q.decay),
    ];
    let filters = filter_sql(query, q.now_ms, &mut binds);
    // bm25() and MATCH need the literal table name — an alias raises "no such column".
    let sql = format!(
        "SELECT m.id, m.conv_id, m.role, m.kind, m.ts, m.text, m.on_head_path,
                m.is_sidechain, m.thread_key,
                c.source, c.title, c.native_id, c.deleted_upstream_at,
                bm25({table}) / (1.0 + ?5 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         WHERE {table} MATCH ?2
           AND (?3 = 1 OR m.on_head_path = 1){filters}
         ORDER BY score
         LIMIT ?4"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds.iter()), |r| {
            let text: String = r.get(5)?;
            let (snippet, snippet_spans) = snippet_marked(&text, query, 160);
            Ok(Hit {
                msg_id: r.get(0)?,
                conv_id: r.get(1)?,
                role: r.get(2)?,
                kind: r.get(3)?,
                ts: r.get(4)?,
                snippet,
                snippet_spans,
                on_head_path: r.get::<_, i64>(6)? != 0,
                is_sidechain: r.get::<_, i64>(7)? != 0,
                thread_key: r.get(8)?,
                source: r.get(9)?,
                title: r.get(10)?,
                native_id: r.get(11)?,
                destinations: crate::destinations(&r.get::<_, String>(9)?, &r.get::<_, String>(11)?),
                deleted_upstream: r.get::<_, Option<i64>>(12)?.is_some(),
                score: r.get(13)?,
        })
    })?;
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

/// `now_ms` is threaded in rather than read from the clock so this answers about the ranking
/// that actually ran. It previously built its options with the zero default and therefore
/// reported a rank no shipping ranker produces — while printing a verdict blaming ranking.
pub fn explain(
    conn: &Connection,
    conv_id: &str,
    text: &str,
    now_ms: i64,
) -> rusqlite::Result<Explain> {
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
    for term in text.split_whitespace() {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM message WHERE conv_id=?1 AND kind='prose'
               AND lower(text) LIKE '%' || lower(?2) || '%'",
            params![conv_id, term],
            |r| r.get(0),
        )?;
        term_hits.push((term.to_string(), n));
    }

    // Exact rather than typeahead: `cs explain` is asked about a query someone finished
    // typing, and the prefix reading would answer about a different expression.
    let ranked = search(
        conn,
        &Query::exact(text),
        &SearchOptions { limit: 500, include_off_path: true, ..SearchOptions::new(now_ms) },
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

    /// A clock before every fixture timestamp, so `max(0, now_ms - ts)` is zero and the
    /// recency divisor is exactly 1.0. These tests reason about raw bm25 ordering, so they
    /// want decay off — named rather than a bare `0`, which is what made it a trap.
    const NO_DECAY: i64 = 0;

    /// The snippet a finished query produces, which is what most of these assert on.
    fn snippet(text: &str, query: &str, width: usize) -> String {
        snippet_marked(text, &Query::exact(query), width).0
    }

    /// The snippet a query still being typed produces.
    fn snippet_typed(text: &str, query: &str, width: usize) -> String {
        snippet_marked(text, &Query::typeahead(query), width).0
    }

    fn opts() -> SearchOptions {
        SearchOptions { limit: 10, ..SearchOptions::new(NO_DECAY) }
    }

    #[test]
    fn match_expr_survives_punctuation_that_would_be_a_syntax_error() {
        assert_eq!(Query::exact("foo bar").match_expr(), r#""foo" "bar""#);
        assert_eq!(Query::exact("rust -- borrow*").match_expr(), r#""rust" "borrow""#);
        assert_eq!(Query::exact(r#"say "hi""#).match_expr(), r#""say" "hi""#);
        assert_eq!(Query::exact("   ").match_expr(), "");
    }

    #[test]
    fn snippet_centres_on_the_densest_cluster_not_the_first_match() {
        let text = "a".repeat(300) + " needle " + &"b".repeat(300);
        let s = snippet(&text, "needle", 60);
        assert!(s.contains("needle"), "got: {s}");
        assert!(s.starts_with('…') && s.ends_with('…'));

        // An early aside against the passage the topic lives in. `min(position)` — what this
        // did until chat-search-6eb.20 — shows the aside.
        let filler = "unrelated prose ".repeat(30);
        let text = format!("mentioned needle once {filler} needle needle needle {filler}");
        let s = snippet(&text, "needle", 60);
        assert_eq!(s.matches("needle").count(), 3, "anchored on the aside: {s}");
    }

    #[test]
    fn snippet_is_char_safe_on_multibyte_text() {
        let s = snippet("héllo wörld ünïcode", "wörld", 10);
        assert!(s.contains("wörld"));
    }

    /// One conversation per text, indexed through the real importer path.
    fn indexed(texts: &[&str]) -> Connection {
        let mut conn = crate::open(":memory:").unwrap();
        let convs: Vec<crate::Conversation> = texts
            .iter()
            .enumerate()
            .map(|(i, text)| crate::Conversation {
                source: "codex".into(),
                native_id: format!("c{i}"),
                titles: Default::default(),
                cwd: None,
                git_branch: None,
                model: None,
                surface: None,
                forked_from_native_id: None,
                head_native_id: None,
                messages: vec![crate::model::Message {
                    native_id: "m0".into(),
                    parent_native_id: None,
                    thread_key: "main".into(),
                    is_sidechain: false,
                    is_error: false,
                    seq: 0,
                    role: crate::model::Role::User,
                    kind: crate::model::Kind::Prose,
                    ts: Some(1_700_000_000_000),
                    text: (*text).into(),
                }],
            })
            .collect();
        crate::write_conversations(&mut conn, convs.iter()).unwrap();
        conn
    }

    #[test]
    fn every_row_the_ranker_returns_has_something_to_highlight() {
        // The invariant the whole module exists for. Each pair is one the substring scan
        // failed: the row ranks because `porter unicode61 remove_diacritics 2` folds the two
        // surface forms together, and `lower.find()` sees two different strings.
        let cases: [(&str, &str); 6] = [
            ("commits", "Commit current changes before the rebase"),
            ("learning", "I learned about the borrow checker today"),
            ("running", "it runs every hour on a timer"),
            ("cafe", "we sketched the schema at a café in Lisbon"),
            ("general", "the generated code needed a second pass"),
            // and the literal case, which was the only one covered before
            ("borrow", "the borrow checker again"),
        ];
        let conn = indexed(&cases.map(|(_, text)| text));

        for (query, text) in cases {
            let parsed = Query::exact(query);
            let hits = search(&conn, &parsed, &opts()).unwrap();
            let hit = hits
                .iter()
                .find(|h| h.msg_id.ends_with(":m0") && h.snippet.contains(&text[..12]))
                .unwrap_or_else(|| panic!("{query:?} did not rank {text:?}"));

            let marks = highlight::spans(text, &parsed.marking_terms());
            assert!(!marks.is_empty(), "{query:?} ranked {text:?} and marked nothing");
            assert!(
                !hit.snippet.starts_with(UNLOCATED),
                "{query:?} ranked {text:?} but the snippet disclaims the match: {}",
                hit.snippet
            );
        }
    }

    #[test]
    fn a_snippet_that_cannot_locate_the_match_says_so() {
        // The old fallback returned the head of the message with no leading `…`, which reads
        // as a deliberate summary. Whatever a client does with this, it cannot mistake it for
        // the text that matched.
        let s = snippet("Commit current changes before the rebase", "elephant", 160);
        assert!(s.starts_with(UNLOCATED), "got: {s}");
        assert!(s.contains("Commit current"), "the head is still worth showing");

        // and the label is inside the budget rather than on top of it
        let long = "alpha ".repeat(200);
        assert!(snippet(&long, "elephant", 40).chars().count() <= 40);
    }

    #[test]
    fn a_typeahead_prefix_marks_the_word_the_keystroke_ranked() {
        // `lea` ranks this row as `"lea"*`, which matches the stem `learn`. No message
        // contains the term `lea`, so the exact-term path marks nothing and every in-progress
        // word in the TUI would carry the "no match" label.
        let text = "I learned about the borrow checker";
        assert_eq!(Query::typeahead("lea").match_expr(), "\"lea\"*", "premise of this test");
        assert!(!snippet_typed(text, "lea", 160).starts_with(UNLOCATED));
        assert!(snippet(text, "lea", 160).starts_with(UNLOCATED), "exact `lea`");
        // Below MIN_PREFIX_LEN the ranker does not open the token either, so neither does this.
        assert!(snippet_typed(text, "le", 160).starts_with(UNLOCATED));
    }

    #[test]
    fn a_carried_span_points_at_the_word_of_the_snippet_that_ranked_the_row() {
        // `commits` ranks this on the stem, and the word in the text is `Commit` — so a
        // renderer holding only the string has nothing to search it for. These are the
        // offsets it has to be given instead.
        let (out, marks) = snippet_marked("Commit current changes", &Query::exact("commits"), 160);
        assert_eq!(marks.len(), 1, "{out:?} {marks:?}");
        assert_eq!(&out[marks[0].start..marks[0].end], "Commit");

        // And through a window, where the offsets stop being the message's own: the string
        // starts with an ellipsis and the match sits hundreds of bytes into the text.
        let text = "alpha ".repeat(100) + "café " + &"omega ".repeat(100);
        let (out, marks) = snippet_marked(&text, &Query::exact("cafe"), 40);
        assert_eq!(marks.len(), 1, "{out:?}");
        assert_eq!(&out[marks[0].start..marks[0].end], "café");
    }

    #[test]
    fn an_unlocatable_match_carries_no_spans_beside_its_label() {
        // Both channels have to say the same thing. Spans surviving here would be the offsets
        // of the *unlabelled* window, so every one would be short by the label's width and
        // would mark whatever now sits at those bytes — including the label itself.
        let (out, marks) = snippet_marked("Commit current changes", &Query::exact("elephant"), 160);
        assert!(out.starts_with(UNLOCATED), "got: {out}");
        assert!(marks.is_empty(), "{marks:?}");

        let (out, marks) = snippet_marked("Commit current changes", &Query::exact(""), 160);
        assert!(out.starts_with(UNLOCATED) && marks.is_empty());
    }

    #[test]
    fn a_hit_carries_the_offsets_its_own_snippet_was_marked_with() {
        // The two fields are built from one call, so they cannot describe different windows.
        let conn = indexed(&["Commit current changes before the rebase"]);
        let hits = search(&conn, &Query::exact("commits"), &opts()).unwrap();
        let h = hits.first().expect("`commits` ranks the row");
        assert_eq!(h.snippet_spans.len(), 1, "{:?}", h.snippet);
        let s = h.snippet_spans[0];
        assert_eq!(&h.snippet[s.start..s.end], "Commit");
    }

    #[test]
    fn every_field_the_json_contract_already_promised_still_serialises() {
        // ADR 12: a Raycast extension and a GUI read this object verbatim, so a field may be
        // added but never renamed or dropped. Spelled out rather than derived from the struct,
        // because a rename would otherwise rename this list along with the contract.
        //
        // `resume_cmd` is the one deliberate removal (chat-search-me9.3), and it is a removal
        // rather than a deprecation because leaving it would mean continuing to answer "how do I
        // reopen this" with a string that can hold one answer and goes stale on a CLI syntax
        // change. `native_id` replaces it as the input a client resolves a destination from.
        let conn = indexed(&["Commit current changes before the rebase"]);
        let hits = search(&conn, &Query::exact("commits"), &opts()).unwrap();
        let json = serde_json::to_value(&hits[0]).unwrap();
        assert!(json.get("resume_cmd").is_none(), "the frozen string is gone, not merely unused");
        for field in [
            "conv_id", "msg_id", "source", "native_id", "destinations", "title", "role", "kind",
            "ts", "score", "snippet", "on_head_path", "is_sidechain", "thread_key",
            "deleted_upstream",
        ] {
            assert!(json.get(field).is_some(), "{field} left the JSON contract");
        }
        // What replaced it, in the shape a GUI reads: a list it picks from, not a line it parses.
        assert_eq!(
            json["destinations"],
            serde_json::json!([{ "kind": "terminal", "argv": ["codex", "resume", "c0"] }])
        );
        assert!(json["snippet"].is_string(), "still the bare string it always was");
        // The addition, in the shape a client would have to parse.
        assert_eq!(json["snippet_spans"], serde_json::json!([{ "start": 0, "end": 6 }]));
    }

    #[test]
    fn filter_tokens_are_not_snippet_anchors() {
        // `agent:codex` names a facet. Before chat-search-6eb.20 it contributed `agent` and
        // `codex` as anchors, so a query mentioning the agent centred on the word "codex"
        // wherever it happened to appear.
        let text = "codex ".repeat(40) + "the borrow checker";
        let s = snippet(&text, "agent:codex borrow", 60);
        assert!(s.contains("borrow checker"), "anchored on the filter keyword: {s}");
    }
}

// ---------------------------------------------------------------- grouping

/// A conversation and its best matching messages — the Algolia DocSearch shape: the
/// conversation is the result, matching messages nest beneath it.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub conv_id: String,
    pub source: String,
    /// The source's own id (ADR 2 makes it stable).
    pub native_id: String,
    /// Every way to reopen this conversation, best first. See [`Hit::destinations`].
    pub destinations: Vec<crate::Destination>,
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
/// Settable per query via [`SearchOptions::repeat_weight`]; see [`DECAY`] for why.
pub const REPEAT_WEIGHT: f64 = 0.25;

/// Most recently active conversations, carrying no nested hits.
///
/// A typeahead UI has to draw something before the first keystroke, and below
/// [`MIN_PREFIX_LEN`] a token is matched exactly rather than as a prefix, so one or two
/// characters return noise rather than a narrowing list. Recency is the only ranking
/// available without a query and it is also the one people expect — the conversation you
/// want is usually among the last few you had.
///
/// Takes the values it reads rather than a whole [`SearchOptions`]. The old signature
/// accepted every option and silently ignored seven of them, so a caller setting `decay` here
/// had no way to learn it did nothing.
///
/// Filtered, though nothing is ranked: `date:today` with no search terms is a real question —
/// "what did I work on today" — and answering it with the whole recent list would be the
/// silent-no-op this crate refuses everywhere else.
pub fn recent(
    conn: &Connection,
    query: &Query,
    now_ms: i64,
    limit: i64,
) -> rusqlite::Result<Vec<Group>> {
    let mut binds = vec![Value::Integer(limit)];
    let filters = filter_sql(query, now_ms, &mut binds);
    // `1 = 1` so the fragment's leading `AND` has something to attach to when there are no
    // filters at all, which is the common case here.
    let sql = format!(
        "SELECT c.id, c.source, c.title, c.ended_at, c.user_turns, c.native_id,
                c.deleted_upstream_at, c.msg_count, c.prose_count, c.cwd
         FROM conversation c
         WHERE 1 = 1{filters}
         -- `NULLS LAST` is not portable to older SQLite; this expresses the same order.
         ORDER BY c.ended_at IS NULL, c.ended_at DESC
         LIMIT ?1"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds.iter()), |r| {
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
            native_id: r.get(5)?,
            destinations: crate::destinations(&r.get::<_, String>(1)?, &r.get::<_, String>(5)?),
            deleted_upstream: r.get::<_, Option<i64>>(6)?.is_some(),
            hits: Vec::new(),
        })
    })?;
    rows.collect()
}

/// Conversations matching `q`, best-first, each carrying up to [`SearchOptions::nested`]
/// messages.
///
/// Scores are pulled ungrouped and folded here rather than in SQL: FTS5 auxiliary functions
/// like bm25() cannot be used through a CTE or subquery, so the grouping has to happen after
/// the rows come back.
pub fn search_grouped(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
) -> rusqlite::Result<Vec<Group>> {
    // Both `Empty` and `TooShort` fall back to recency. The client decides how to *say* which
    // of the two it is showing; the routing is the same, and doing it here is what lets the
    // TUI stop blanking its own query text to force this branch.
    if !query.is_searchable() {
        return recent(conn, query, q.now_ms, q.limit);
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
    let mut binds = vec![
        Value::Integer(q.now_ms),
        Value::Text(query.match_expr()),
        Value::Integer(q.include_off_path as i64),
        Value::Integer(ceiling),
        Value::Real(q.decay),
    ];
    let filters = filter_sql(query, q.now_ms, &mut binds);
    let sql = format!(
        "SELECT m.id, m.conv_id, m.seq,
                bm25({table}) / (1.0 + ?5 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         WHERE {table} MATCH ?2
           AND (?3 = 1 OR m.on_head_path = 1){filters}
         ORDER BY score
         LIMIT ?4"
    );
    // Bound rather than interpolated: a decay swept across a range would otherwise mint a
    // new statement per value and defeat the cache this call relies on. The filter fragment
    // varies only with the *shape* of the query's filters, not their values, for the same
    // reason — so a typeahead under one `agent:` filter still hits a single cached statement.
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds.iter()), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, f64>(3)?,
        ))
    })?;

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
        let shown = hits.into_iter().take(q.nested).map(|(id, _, _)| id).collect();
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

    hydrate(conn, query, ranked)
}

/// One conversation that survived ranking, before its display columns are fetched.
struct Ranked {
    conv_id: String,
    score: f64,
    match_count: usize,
    seqs: Vec<i64>,
    /// Message ids to nest under the conversation, capped at `nested`, best-scoring first.
    ///
    /// The order is load-bearing twice over: clients truncate rather than sample
    /// (`cs-tui::rows`), and the group's headline snippet is therefore the one built from the
    /// message that actually won the ranking — which is the message whose densest cluster
    /// [`highlight::snippet`] should be anchoring on (chat-search-6eb.20).
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
fn hydrate(conn: &Connection, query: &Query, ranked: Vec<Ranked>) -> rusqlite::Result<Vec<Group>> {
    let mut meta = conn.prepare_cached(
        "SELECT source, title, native_id, deleted_upstream_at, ended_at, user_turns,
                msg_count, prose_count, cwd
         FROM conversation WHERE id = ?1",
    )?;
    let mut msg = conn.prepare_cached(
        "SELECT m.id, m.conv_id, m.role, m.kind, m.ts, m.text, m.on_head_path,
                m.is_sidechain, m.thread_key,
                c.source, c.title, c.native_id, c.deleted_upstream_at
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
                    r.get::<_, String>(2)?,
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
                let (snippet, snippet_spans) = snippet_marked(&text, query, 160);
                Ok(Hit {
                    msg_id: r.get(0)?,
                    conv_id: r.get(1)?,
                    role: r.get(2)?,
                    kind: r.get(3)?,
                    ts: r.get(4)?,
                    snippet,
                    snippet_spans,
                    on_head_path: r.get::<_, i64>(6)? != 0,
                    is_sidechain: r.get::<_, i64>(7)? != 0,
                    thread_key: r.get(8)?,
                    source: r.get(9)?,
                    title: r.get(10)?,
                    native_id: r.get(11)?,
                    destinations: crate::destinations(&r.get::<_, String>(9)?, &r.get::<_, String>(11)?),
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
            native_id: m.as_ref().map(|m| m.2.clone()).unwrap_or_default(),
            destinations: m
                .as_ref()
                .map(|m| crate::destinations(&m.0, &m.2))
                .unwrap_or_default(),
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

    /// A clock before every fixture timestamp, so `max(0, now_ms - ts)` is zero and the
    /// recency divisor is exactly 1.0. These tests reason about raw bm25 ordering, so they
    /// want decay off — named rather than a bare `0`, which is what made it a trap.
    const NO_DECAY: i64 = 0;

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

        let q = SearchOptions { limit: 10, nested: 3, ..SearchOptions::new(NO_DECAY) };
        // An empty MATCH expression is a syntax error, so the point is that this does not
        // merely return nothing — it returns something useful, which is what a TUI opens on.
        let groups = search_grouped(&conn, &Query::typeahead(""), &q).unwrap();
        let ids: Vec<_> = groups.iter().map(|g| g.conv_id.as_str()).collect();
        assert_eq!(ids, ["c:a", "c:c", "c:b", "c:z"]);
        assert!(groups.iter().all(|g| g.hits.is_empty()), "no query means no hits to nest");
        // The day rides along with the epoch value on this path too — a typeahead UI opens
        // on `recent` before the first keystroke, so it is where a client would first be
        // tempted to derive the date itself.
        assert_eq!(groups[0].ended_date, crate::time::local_ymd(300));
        assert!(groups[3].ended_date.is_none(), "no ended_at, no day to name");

        // and the flat path stays empty rather than raising
        assert!(search(&conn, &Query::typeahead(""), &q).unwrap().is_empty());
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
        let got = recent(&conn, &Query::exact("agent:codex"), NO_DECAY, 10).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|g| g.source == "codex"));

        let capped = recent(&conn, &Query::exact(""), NO_DECAY, 1).unwrap();
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

        let opts = SearchOptions { limit: 5, nested: 3, ..SearchOptions::new(NO_DECAY) };
        let groups = search_grouped(&conn, &Query::exact("borrow"), &opts).unwrap();
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
            let q = SearchOptions {
                limit: 10, nested: 1, repeat_weight: w, decay: 0.0,
                ..SearchOptions::new(NO_DECAY)
            };
            search_grouped(&conn, &Query::exact("widget"), &q).unwrap()[0].conv_id.clone()
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
            let q = SearchOptions {
                limit: 10, nested: 1, decay: d, now_ms: 3 * year,
                ..SearchOptions::new(NO_DECAY)
            };
            search_grouped(&conn, &Query::exact("widget"), &q).unwrap()[0].conv_id.clone()
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

/// The DSL, against SQL rather than against the parser.
///
/// `query::tests` proves a token parses; these prove the parse reaches the database and
/// changes which rows come back. The two halves fail differently — a filter that parses and
/// does not filter is exactly the silent no-op `chat-search-6eb.11` was filed to remove —
/// so both are asserted (`chat-search-6eb.11`).
#[cfg(test)]
mod filter_tests {
    use super::*;

    /// An arbitrary fixed instant. Every date assertion below is built *from* this via the
    /// same civil-time rule the filter uses, never from a literal, so none of them depends
    /// on the zone the test happens to run in.
    const NOW: i64 = 1_784_433_600_000; // 2026-07-19T04:00Z

    fn day_start(ms: i64) -> i64 {
        crate::time::local_day_start(ms).unwrap()
    }

    fn days_from(ms: i64, days: i64) -> i64 {
        crate::time::shift_days_in(&chrono::Local, ms, days).unwrap()
    }

    /// A conversation with all three filterable columns set, carrying one searchable message.
    fn seed(conn: &Connection, id: &str, source: &str, cwd: Option<&str>, ended: i64) {
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, title, ended_at, user_turns, cwd)
             VALUES (?1, ?2, ?1, ?1, ?3, 1, ?4)",
            params![id, source, ended, cwd],
        )
        .unwrap();
        let text = "the borrow checker again";
        conn.execute(
            "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
             VALUES (?1, ?2, 'main', 1, 'user', 'prose', ?3, ?4)",
            params![format!("{id}:m"), id, ended, text],
        )
        .unwrap();
        // fts5 here is contentless, so postings are written explicitly rather than by trigger.
        let rowid = conn.last_insert_rowid();
        conn.execute("INSERT INTO fts_prose(rowid, text) VALUES (?1, ?2)", params![rowid, text])
            .unwrap();
    }

    /// Anchored on the local day, for the named-day filters.
    ///
    /// Every timestamp is derived from `day_start` and `days_from` — the same two rules the
    /// filter itself resolves through — so which civil day a row lands on is the same fact in
    /// the fixture and in the query, in any zone. A literal offset from midnight would not
    /// be: `NOW` is a fixed instant, so its distance from local midnight is whatever the
    /// machine's offset makes it, and an hour after midnight can be twenty hours ago or
    /// four hours from now.
    fn corpus() -> Connection {
        let conn = crate::open(":memory:").unwrap();
        let today = day_start(NOW);
        seed(&conn, "codex-web", "codex", Some("/home/t/dev/web-app"), today);
        seed(&conn, "claude-api", "claude-code", Some("/home/t/dev/API-Server"), today);
        seed(&conn, "gemini-web", "gemini-cli", Some("/home/t/dev/web-app"), days_from(today, -1));
        seed(&conn, "codex-old", "codex", None, days_from(today, -10));
        conn
    }

    /// Anchored on `NOW` itself, for the age filters, which measure from now rather than
    /// from midnight. Same reasoning as [`corpus`]: fixture and filter share an anchor.
    fn aged_corpus() -> Connection {
        let conn = crate::open(":memory:").unwrap();
        seed(&conn, "an-hour", "codex", None, NOW - 3_600_000);
        seed(&conn, "five-hours", "codex", None, NOW - 5 * 3_600_000);
        seed(&conn, "three-days", "codex", None, days_from(NOW, -3));
        seed(&conn, "a-month", "codex", None, days_from(NOW, -30));
        conn
    }

    fn opts() -> SearchOptions {
        SearchOptions { limit: 50, nested: 0, ..SearchOptions::new(NOW) }
    }

    /// Conversation ids a query returns, sorted so the assertion is about the set.
    fn ids(conn: &Connection, text: &str) -> Vec<String> {
        let mut got: Vec<String> = search_grouped(conn, &Query::exact(text), &opts())
            .unwrap()
            .into_iter()
            .map(|g| g.conv_id)
            .collect();
        got.sort();
        got
    }

    #[test]
    fn every_form_in_the_acceptance_criteria_filters_the_result_set() {
        let conn = corpus();
        assert_eq!(ids(&conn, "borrow").len(), 4, "unfiltered, everything matches");

        assert_eq!(ids(&conn, "agent:claude-code,codex borrow"), ["claude-api", "codex-old", "codex-web"]);
        assert_eq!(ids(&conn, "-agent:codex borrow"), ["claude-api", "gemini-web"]);
        assert_eq!(ids(&conn, "dir:!web-app borrow"), ["claude-api", "codex-old"]);
        assert_eq!(ids(&conn, "date:today borrow"), ["claude-api", "codex-web"]);
        assert_eq!(ids(&conn, "date:yesterday borrow"), ["gemini-web"]);
        assert_eq!(ids(&conn, "date:week borrow"), ["claude-api", "codex-web", "gemini-web"]);

        let aged = aged_corpus();
        assert_eq!(ids(&aged, "date:<3h borrow"), ["an-hour"]);
        assert_eq!(ids(&aged, "date:>1d borrow"), ["a-month", "three-days"]);
        // `date:week` is `date:<1w` written the other way, and has to mean the same thing.
        assert_eq!(ids(&aged, "date:week borrow"), ids(&aged, "date:<1w borrow"));
        assert_eq!(ids(&aged, "date:week borrow"), ["an-hour", "five-hours", "three-days"]);
    }

    #[test]
    fn filters_compose_within_one_input_string() {
        // The whole point of the DSL: a TUI has one input box, so the filters have to stack
        // inside it rather than arrive as separate flags.
        let conn = corpus();
        assert_eq!(ids(&conn, "agent:codex,gemini-cli dir:web-app date:today borrow"), ["codex-web"]);
        assert_eq!(ids(&conn, "-agent:codex -dir:web-app borrow"), ["claude-api"]);
    }

    #[test]
    fn dir_matches_a_case_insensitive_substring_while_agent_matches_exactly() {
        let conn = corpus();
        // The cwd is `/home/t/dev/API-Server`; none of these is the whole of it.
        for text in ["dir:api borrow", "dir:API-Server borrow", "dir:/dev/api borrow"] {
            assert_eq!(ids(&conn, text), ["claude-api"], "{text}");
        }
        // A source id is an enum, so a substring of one selects nothing rather than
        // silently meaning both claude-code and any other claude.
        assert_eq!(ids(&conn, "agent:claude borrow"), Vec::<String>::new());
    }

    #[test]
    fn a_wildcard_in_a_dir_value_matches_itself_rather_than_anything() {
        let conn = crate::open(":memory:").unwrap();
        let today = day_start(NOW);
        seed(&conn, "literal", "codex", Some("/home/t/my_app"), today);
        seed(&conn, "wildcard", "codex", Some("/home/t/myXapp"), today);
        // `_` is LIKE's single-character wildcard. Unescaped, this would return both.
        assert_eq!(ids(&conn, "dir:my_app borrow"), ["literal"]);
    }

    #[test]
    fn a_conversation_with_no_directory_survives_an_exclusion_but_not_an_inclusion() {
        // `codex-old` has a NULL cwd. It is certainly not inside web-app, so excluding
        // web-app has to keep it — a bare `NOT LIKE` against NULL is unknown and would drop
        // it, which is the failure mode this is here to pin.
        let conn = corpus();
        assert!(ids(&conn, "dir:!web-app borrow").contains(&"codex-old".to_string()));
        assert!(!ids(&conn, "dir:web-app borrow").contains(&"codex-old".to_string()));
    }

    #[test]
    fn two_date_bounds_intersect_into_a_range() {
        let conn = aged_corpus();
        // Older than a day and younger than a week. Intersecting rather than unioning is
        // what lets two bounds describe a range at all; unioned, this would be everything.
        assert_eq!(ids(&conn, "date:>1d date:<1w borrow"), ["three-days"]);
    }

    #[test]
    fn a_value_nothing_can_select_on_is_reported_and_does_not_filter() {
        // The acceptance criterion's other half: mid-word is the normal state in a
        // typeahead, so a filter nobody finished typing must neither error nor quietly
        // narrow the results — it says so and stays out of the way.
        let conn = corpus();
        for text in ["date:nope borrow", "agent: borrow", "date: borrow"] {
            assert_eq!(ids(&conn, text).len(), 4, "{text} must not filter");
            assert!(!Query::exact(text).rejected().is_empty(), "{text} must say so");
        }
        assert_eq!(Query::exact("date:nope borrow").rejected(), ["date:nope"]);
    }

    #[test]
    fn a_filter_narrows_the_recency_fallback_as_well_as_a_search() {
        // "What did I work on today" is a real question with no search terms in it. Before
        // this, an unsearchable query dropped every filter but the source and answered with
        // the whole recent list.
        let conn = corpus();
        let recent_ids = |text: &str| {
            let mut got: Vec<String> = search_grouped(&conn, &Query::typeahead(text), &opts())
                .unwrap()
                .into_iter()
                .map(|g| g.conv_id)
                .collect();
            got.sort();
            got
        };
        assert_eq!(recent_ids("").len(), 4, "no query and no filter is the whole list");
        assert_eq!(recent_ids("date:today"), ["claude-api", "codex-web"]);
        assert_eq!(recent_ids("dir:web-app"), ["codex-web", "gemini-web"]);
        // `le` is below the prefix floor, so this is the held-query path rather than the
        // blank one, and it has to carry the filter too.
        assert_eq!(recent_ids("date:today le"), ["claude-api", "codex-web"]);
    }

    #[test]
    fn the_flat_path_filters_exactly_as_the_grouped_one_does() {
        // Two call sites, one fragment. They drifted apart once already, which is how
        // `agent:codex` came to return ten claude-code rows.
        let conn = corpus();
        for text in ["agent:codex borrow", "dir:web-app borrow", "date:today borrow"] {
            let flat: std::collections::BTreeSet<String> =
                search(&conn, &Query::exact(text), &opts()).unwrap().into_iter().map(|h| h.conv_id).collect();
            let grouped: std::collections::BTreeSet<String> = ids(&conn, text).into_iter().collect();
            assert_eq!(flat, grouped, "{text}");
        }
    }

    #[test]
    fn a_source_flag_and_a_typed_filter_are_the_same_filter() {
        // `--source` desugars into the query rather than living beside it, so the two
        // spellings cannot disagree — the reconciliation methods `TUI-DESIGN.md` §5 records
        // fast-resume paying for have nowhere to live.
        let conn = corpus();
        let flagged = Query::exact("borrow").with_source(Some("codex"));
        let typed = Query::exact("agent:codex borrow");
        let run = |q: &Query| {
            let mut got: Vec<String> =
                search_grouped(&conn, q, &opts()).unwrap().into_iter().map(|g| g.conv_id).collect();
            got.sort();
            got
        };
        assert_eq!(run(&flagged), run(&typed));
        assert_eq!(run(&flagged), ["codex-old", "codex-web"]);
        // A source named in the text wins: it was typed more recently than the flag was passed.
        let both = Query::exact("agent:gemini-cli borrow").with_source(Some("codex"));
        assert_eq!(run(&both), ["gemini-web"]);
    }
}

/// What a filter costs a keystroke, measured in-process against the real index.
///
/// The CLI cannot answer this: `cs search` opens a 306 MB database per invocation, and that
/// dominates everything the TUI actually pays per keystroke, which is this call and nothing
/// else (ADR 14 — the search is synchronous in the event loop). Measuring through the binary
/// made a filter look 4x more expensive than it is.
#[cfg(test)]
mod filter_cost {
    use super::*;

    #[test]
    #[ignore = "needs a real index; set CS_INDEX to an index.db"]
    fn a_filter_does_not_cost_a_keystroke_its_budget() {
        let Ok(path) = std::env::var("CS_INDEX") else { return };
        let conn = Connection::open(path).expect("readable index");
        let opts = || SearchOptions {
            limit: 50,
            nested: 3,
            ..SearchOptions::new(crate::time::now_ms())
        };

        // Best of seven. The interesting quantity is the floor — a keystroke competing with
        // a backup for the disk is not what a budget is set against.
        let cost = |text: &str| {
            let query = Query::typeahead(text);
            (0..7)
                .map(|_| {
                    let t0 = std::time::Instant::now();
                    let groups = search_grouped(&conn, &query, &opts()).unwrap();
                    std::hint::black_box(groups);
                    t0.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::INFINITY, f64::min)
        };

        // Each pair is the same search with and without a filter, so the difference is the
        // filter and not the term. `the` is the corpus's worst case and is over budget
        // before any filter is added (chat-search-6eb.29, chat-search-6eb.30); it is here to
        // show a filter does not make that worse either.
        let pairs = [
            ("borrow checker", "agent:codex borrow checker"),
            ("borrow checker", "-agent:codex borrow checker"),
            ("borrow checker", "agent:codex,claude-code borrow checker"),
            ("borrow checker", "dir:dev borrow checker"),
            ("borrow checker", "date:week borrow checker"),
            ("borrow checker", "date:<3d borrow checker"),
            ("the", "date:week the"),
        ];
        let mut worst: f64 = 0.0;
        for (plain, filtered) in pairs {
            let (base, with) = (cost(plain), cost(filtered));
            println!("{base:8.1} ms  ->{with:8.1} ms   {filtered}");
            worst = worst.max(with - base);
        }
        println!("worst filter overhead: {worst:.1} ms");
        // Generous against the measurement, tight against the failure it guards: the filter
        // reads two columns off a row already joined, so its cost is a row fetch per
        // candidate and nothing that scales with the corpus. A regression into a per-message
        // subquery or a lost index would land far outside this.
        assert!(worst < 25.0, "a filter added {worst:.1} ms to a keystroke");
    }
}
