use crate::highlight;
use crate::model::Kind;
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
    /// The fts5 table this field ranks against, for interpolation into SQL — `MATCH` and the
    /// auxiliary functions both refuse an alias, so the literal name has to appear.
    pub(crate) fn table(self) -> &'static str {
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
    snippet_marked_at(text, &highlight::spans(text, &q.marking_terms()), width)
}

/// [`snippet_marked`] for a caller that has already located the matches.
///
/// The ranking path takes this one, with `marks` from [`highlight::spans_for`] — which decides
/// how to locate a whole result set at once, and so cannot be called from inside the loop that
/// builds one snippet at a time.
pub fn snippet_marked_at(
    text: &str,
    marks: &[highlight::Span],
    width: usize,
) -> (String, Vec<highlight::Span>) {
    let (out, spans) = highlight::snippet_at(text, marks, width);
    if !spans.is_empty() {
        return (out, spans);
    }
    // Re-cut the head against the reduced budget so the label does not push the line over
    // `width`. No marks means no MATCH, so this is arithmetic, not a second query.
    let (head, _) = highlight::snippet_at(text, &[], width.saturating_sub(UNLOCATED.chars().count()));
    // Empty rather than `spans`, which is the same value today and would stop being one the
    // moment this branch is entered for any other reason: these offsets were taken before the
    // label was prepended, so each is short by its width and would mark the wrong word.
    (format!("{UNLOCATED}{head}"), Vec::new())
}

/// How to run a search, as distinct from what was asked. The query text itself moves to
/// [`crate::query::Query`]; what remains here is tuning the caller chooses.
///
/// `Clone` because [`crate::answer::Answer`] keeps the whole of it, `now_ms` included, as half
/// of the receipt it settles against — see [`crate::answer::Answer::settle`].
#[derive(Debug, Clone)]
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
    /// argument to the grouped search, which put one of the ten options somewhere the other
    /// nine could not be read from.
    pub nested: usize,
    /// Fill `kind_runs` — what each conversation is made of, for a client that draws it as a
    /// strip ([`crate::answer::Group::kind_runs`] is where it lands).
    ///
    /// Off by default because it is the one field whose cost scales with the *conversations*
    /// returned rather than with the matches in them: it reads every head-path message of
    /// every row. Measured against the real index, it adds 4–30 ms at the 20–50 rows a
    /// terminal holds and 205 ms at 354 rows for a broad prefix — and the TUI, which is the
    /// caller that runs this on every keystroke, draws [`match_density`] rather than a band
    /// strip and would be paying that for nothing.
    ///
    /// A covering index would make it close to free and let this flag go away; that is
    /// chat-search-me9.26, filed with these numbers.
    pub shape: bool,
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
            shape: false,
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

pub(crate) fn search(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
) -> rusqlite::Result<Vec<Hit>> {
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
                bm25({table}) / (1.0 + ?5 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score,
                m.rowid
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
        Ok(Unmarked {
            rowid: r.get(14)?,
            text: r.get(5)?,
            hit: Hit {
                msg_id: r.get(0)?,
                conv_id: r.get(1)?,
                role: r.get(2)?,
                kind: r.get(3)?,
                ts: r.get(4)?,
                snippet: String::new(),
                snippet_spans: Vec::new(),
                on_head_path: r.get::<_, i64>(6)? != 0,
                is_sidechain: r.get::<_, i64>(7)? != 0,
                thread_key: r.get(8)?,
                source: r.get(9)?,
                title: r.get(10)?,
                native_id: r.get(11)?,
                destinations: crate::destinations(&r.get::<_, String>(9)?, &r.get::<_, String>(11)?),
                deleted_upstream: r.get::<_, Option<i64>>(12)?.is_some(),
                score: r.get(13)?,
            },
        })
    })?;
    let pending = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(mark(conn, query, q.field, pending))
}

/// A hit whose snippet has not been built yet.
///
/// Marking is decided for a result set as a whole rather than a row at a time
/// ([`highlight::spans_for`]), so every row has to be in hand before any of them can be
/// marked — which means the text and its rowid outlive the query that fetched them.
/// `hit.snippet` is empty in the meantime, and [`mark`] is the only way out of this state, so
/// no caller ever sees one.
struct Unmarked {
    hit: Hit,
    /// Which row of the fts table this is, for the route that asks the index directly.
    rowid: i64,
    text: String,
}

/// Build every pending hit's snippet, locating the whole set's matches in one decision.
fn mark(conn: &Connection, query: &Query, field: Field, pending: Vec<Unmarked>) -> Vec<Hit> {
    let terms = query.marking_terms();
    let marks = {
        let rows: Vec<(i64, &str)> = pending.iter().map(|p| (p.rowid, p.text.as_str())).collect();
        highlight::spans_for(conn, field, &rows, &terms)
    };
    pending
        .into_iter()
        .zip(marks)
        .map(|(mut p, found)| {
            (p.hit.snippet, p.hit.snippet_spans) = snippet_marked_at(&p.text, &found, 160);
            p.hit
        })
        .collect()
}

/// Why a conversation did *not* come back for a query.
///
/// A false negative has four very different causes — a filter excluded it, the words are only
/// in a kind nobody indexes, the text was never indexed at all, or it was indexed and ranked
/// too low — and they need opposite fixes. Guessing between them is the slowest part of tuning
/// ranking, so the index answers it directly.
#[derive(Debug, Serialize)]
pub struct Explain {
    pub conv_id: String,
    pub exists: bool,
    pub messages: i64,
    pub prose_messages: i64,
    pub indexed_prose: i64,
    /// Messages here whose [`Kind`] carries no postings at all — reasoning, today.
    ///
    /// Reported even when it explains nothing, because "8% of this conversation was never
    /// looked at" is a fact about the answer that a reader cannot recover from the other
    /// counts: `messages` minus `prose_messages` is mostly tool traffic, which *is* indexed.
    pub unindexed_messages: i64,
    pub off_path_messages: i64,
    pub deleted_upstream: bool,
    /// Per query term: how many prose messages in this conversation contain it at all.
    ///
    /// Keyed on [`Query::terms`], not on the raw words. A filter token is not something to
    /// find in the text, and reporting `agent:codex` as a term present in zero messages reads
    /// as a recall problem when it is the filter doing exactly its job.
    pub term_hits: Vec<(String, i64)>,
    /// The same count over the kinds that carry no postings, so a zero above can be told from
    /// a zero that only means "nobody indexed the place this word lives".
    ///
    /// This is the distinction chat-search-8mb was filed for. `gbdt` appears once in
    /// `chatgpt-export:68c2e851`, in a reasoning message, and nowhere else in that
    /// conversation; `term_hits` reported 0 and the verdict read "no message contains any
    /// query term", which is true of the index and false of the conversation the user is
    /// asking about.
    pub unindexed_term_hits: Vec<(String, i64)>,
    /// Whether this conversation is one the query's filters drop, independent of any text.
    ///
    /// The cause `chat-search-6eb.11` introduced and this tool could not see: with a filter in
    /// force the conversation can be excluded before ranking ever looks at it, and every
    /// text-shaped verdict below is then an answer to a question nobody asked.
    pub excluded_by_filter: bool,
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

    // Counted out of fts5's `_docsize` shadow table, which holds one row per document the
    // index actually has postings for. The obvious query — joining `message` to `fts_prose`
    // itself — stopped meaning this when the fts tables became external content: an
    // unconstrained scan of `fts_prose` now returns every row of `message`, so it counted tool
    // traffic as indexed prose and could never report zero. Only a `MATCH` consults the index,
    // and there is no term to match on here.
    //
    // Asking fts5 rather than deriving it from `message.kind` is the point of the number: the
    // indexer's rule is "every prose message gets a posting", so a count that applied that rule
    // a second time would agree with it by construction and catch nothing.
    let indexed_prose: i64 = conn.query_row(
        "SELECT COUNT(*) FROM fts_prose_docsize d JOIN message m ON m.rowid = d.id
         WHERE m.conv_id = ?1",
        params![conv_id],
        |r| r.get(0),
    )?;

    let deleted_upstream: bool = conn.query_row(
        "SELECT deleted_upstream_at IS NOT NULL FROM conversation WHERE id=?1",
        params![conv_id],
        |r| r.get(0),
    ).unwrap_or(false);

    // Exact rather than typeahead: `cs explain` is asked about a query someone finished
    // typing, and the prefix reading would answer about a different expression.
    let parsed = Query::exact(text);

    // Per-term presence via LIKE rather than MATCH: this deliberately bypasses the
    // tokenizer, so a term the stemmer mangled still shows up as present in the text.
    //
    // Over the parsed terms, so `agent:codex` is not looked for in the prose. It never was
    // prose — it is why the conversation is absent, not a word that failed to appear in it.
    // Which kinds carry no postings, derived from the rule rather than named here — see
    // `Kind::is_indexed`. Interpolated because they are `&'static str` off an enum, like
    // `Field::table`; nothing a caller supplies goes near this string.
    let unindexed_kinds: Vec<&str> =
        Kind::ALL.iter().filter(|k| !k.is_indexed()).map(|k| k.as_str()).collect();
    let unindexed_list =
        unindexed_kinds.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");

    let unindexed_messages: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM message WHERE conv_id=?1 AND kind IN ({unindexed_list})"),
        params![conv_id],
        |r| r.get(0),
    )?;

    let mut term_hits = Vec::new();
    let mut unindexed_term_hits = Vec::new();
    for term in parsed.terms() {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM message WHERE conv_id=?1 AND kind='prose'
               AND lower(text) LIKE '%' || lower(?2) || '%'",
            params![conv_id, term],
            |r| r.get(0),
        )?;
        term_hits.push((term.clone(), n));

        // Same LIKE, deliberately: this answers "is the word *there*", and the whole point of
        // asking is that no tokenizer ever looked at these messages. Over-reporting a
        // substring is the safe direction for a diagnostic — `rust` inside `trustworthy` sends
        // a reader to look, where a miss would let them conclude the word does not exist.
        let u: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM message WHERE conv_id=?1 AND kind IN ({unindexed_list})
                   AND lower(text) LIKE '%' || lower(?2) || '%'"
            ),
            params![conv_id, term],
            |r| r.get(0),
        )?;
        unindexed_term_hits.push((term.clone(), u));
    }

    // Asked of the filters alone, with no MATCH in the way, so the answer holds even for a
    // query with no searchable terms — which is precisely when the text-shaped verdicts have
    // nothing to say and the filter is the whole story.
    let mut binds = vec![Value::Text(conv_id.to_string())];
    let filters = filter_sql(&parsed, now_ms, &mut binds);
    let excluded_by_filter = exists
        && !filters.is_empty()
        && !conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM conversation c WHERE c.id = ?1{filters})"),
            params_from_iter(binds.iter()),
            |r| r.get::<_, i64>(0).map(|n| n > 0),
        )?;

    let ranked = search(
        conn,
        &parsed,
        &SearchOptions { limit: 500, include_off_path: true, ..SearchOptions::new(now_ms) },
    )?;
    let best_rank = ranked.iter().position(|h| h.conv_id == conv_id);
    let best_score = best_rank.map(|i| ranked[i].score);

    let verdict = if !exists {
        "not in the index — the importer never produced this conversation".into()
    } else if excluded_by_filter {
        // Ahead of every text-shaped branch. A filtered-out conversation is never ranked, so
        // those branches would report on text that was never consulted — and this tool exists
        // to stop exactly that guess (chat-search-6eb.36).
        "excluded by a filter in the query — nothing here is about the text".into()
    } else if indexed_prose == 0 {
        "conversation exists but has no indexed prose — all of it is tool traffic".into()
    } else if term_hits.is_empty() {
        // Filters that all match, and no terms to rank on. Not a recall problem: there was
        // never a word to fail to find.
        "the query is only filters, and this conversation passes them — nothing was ranked".into()
    } else if term_hits.iter().all(|(_, n)| *n == 0)
        && unindexed_term_hits.iter().any(|(_, n)| *n > 0)
    {
        // Ahead of the recall branch below, which would otherwise say "no message contains any
        // query term" about a conversation where a message plainly does (chat-search-8mb).
        // That sentence was true of the index and false of the thing the user asked about, and
        // it is the more misleading of the two answers: it sends someone to fix ranking or
        // stemming for a word the ranker was never shown.
        let kinds = unindexed_kinds.join(", ");
        format!(
            "the term is only in messages of kind {kinds}, which carry no postings — \
             search cannot find this however it is ranked"
        )
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
        unindexed_messages,
        off_path_messages: off_path,
        deleted_upstream,
        term_hits,
        unindexed_term_hits,
        excluded_by_filter,
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

/// The conversations one row stands for, when it stands for more than itself.
///
/// Google Takeout exports an activity log with no conversation key in it, so a twenty-turn
/// Gemini chat is twenty conversations in the index and would be twenty rows in a result set.
/// [`crate::sittings`] reads the gaps between them and puts them back together at read time;
/// this is that fold, reported rather than assumed, so a client can say *this row is several
/// records we believe were one sitting* instead of quietly presenting a reconstruction as a
/// thing the export recorded.
///
/// `None` for every row that is one conversation, which is the whole corpus bar the Gemini
/// Apps and AI Mode records.
#[derive(Debug, Clone, Serialize)]
pub struct Sitting {
    /// Every conversation folded into this row, earliest first. The first is
    /// [`Group::conv_id`], and each is a real permanent id that `cs show` opens.
    pub members: Vec<String>,
    /// The silence that delimited it. Carried so the row can say what produced the fold
    /// rather than implying the export drew the boundary — see [`crate::sittings::GAP_MS`].
    pub gap_ms: i64,
}

/// A conversation and its best matching messages, as the ranking builds it.
///
/// Crate-private: what a client reads is [`crate::answer::Group`], which this is one `From`
/// away from. The two are kept apart rather than merged because this one carries whatever the
/// ranking needs to carry, and that is not a decision the wire should have to follow.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Group {
    /// The conversation this row points at. When [`Group::sitting`] is set, this is the
    /// conversation that *opened* the sitting — the earliest record, whose prompt is the
    /// title — and the rest are named there.
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
    /// tomorrow for an evening conversation — which is precisely how a shell client's jq and
    /// the binary's formatter came to disagree.
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
    /// What the conversation is made of, in reading order, run-length encoded:
    /// `[["user",1],["agent",1],["tool",34]]`.
    ///
    /// The shape a client draws as a strip on every result row. It answers the question a
    /// title and a match count cannot — whether this was a question and an answer, or six
    /// hours of an agent running tools — and it answers it for 354 rows at once, which is why
    /// it is here rather than left to `cs show`: that is one process and one whole transcript
    /// per conversation, against a list that redraws on a keystroke.
    ///
    /// **The positions are [`crate::blocks::READING_ORDER`], and only what a reader draws.**
    /// Successful tool results are omitted ([`crate::blocks::drawn`]) rather than counted,
    /// because a strip position a reader cannot click on is a lie about where they are. So the
    /// run lengths sum to the drawn message count, which is *not* [`Group::msg_count`] and not
    /// the denominator [`Group::match_seqs`] uses — see chat-search-me9.25, which is the gap
    /// between those two coordinate spaces.
    ///
    /// **Empty unless [`SearchOptions::shape`] asked for it**, which is not the same thing as
    /// a conversation with nothing in it, and a client that cannot tell those apart will draw
    /// an empty strip and believe it. Filling it costs a read of every message in every row
    /// returned; the flag carries the numbers.
    ///
    /// Once asked for, it is a property of the conversation rather than of the query, so it
    /// survives the query being cleared — the strip is what makes the no-query list
    /// triageable in the first place.
    pub kind_runs: Vec<crate::blocks::Run>,
    /// Set when this row is several activity-log records read back as one chat. See
    /// [`Sitting`]; every count above is the sitting's, not the opening record's.
    pub sitting: Option<Sitting>,
    pub deleted_upstream: bool,
    pub hits: Vec<Hit>,
}

/// One conversation's drawn messages, as bands, in reading order — see [`Group::kind_runs`].
///
/// Bands rather than runs, because a row can be several conversations ([`Sitting`]) and two
/// run lists cannot be concatenated without re-encoding the seam between them. Run-length
/// encoding is therefore left to the one caller that knows how many conversations it is
/// looking at.
///
/// Three narrow columns and no `text`. That matters more than it looks: `message` stores its
/// body inline, so a `SELECT` naming `text` walks the whole 324 MB of it, and the top 354
/// results for a broad query hold 108k messages between them.
///
/// One prepared statement reused across the page, for the same reason [`hydrate`] does it —
/// an `IN (...)` list sized to the result count would be a new statement per query and would
/// lose the cache on every keystroke.
fn drawn_bands(conn: &Connection, conv_id: &str) -> rusqlite::Result<Vec<crate::blocks::Band>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT role, kind, is_error FROM message
         WHERE conv_id = ?1 AND on_head_path = 1
         {}",
        crate::blocks::READING_ORDER
    ))?;
    let rows = stmt
        .query_map(params![conv_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .iter()
        .filter(|(_, kind, is_error)| crate::blocks::drawn(kind, *is_error))
        .map(|(role, kind, _)| crate::blocks::band(role, kind))
        .collect())
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
///
/// One row per sitting, like the ranked path: the browse list is where the shredding shows
/// worst, since 1,271 Gemini records unfolded would be a third of everything a reader scrolls
/// past. The row is the conversation that *opened* the sitting, dated and counted by the
/// whole of it. One consequence worth naming: a `date:` filter is still tested against each
/// conversation's own `ended_at`, so a sitting that began before midnight and ran past it is
/// judged on the record that opened it rather than on the whole span.
pub(crate) fn recent(
    conn: &Connection,
    query: &Query,
    now_ms: i64,
    limit: i64,
    shape: bool,
) -> rusqlite::Result<Vec<Group>> {
    crate::sittings::ensure(conn)?;
    let mut binds = vec![Value::Integer(limit)];
    let filters = filter_sql(query, now_ms, &mut binds);
    // Named once: it is both what the row displays and what the list is ordered by, and those
    // two disagreeing is how a "most recent" list ends up sorted by a date it does not show.
    let ended = crate::sittings::total("ended_at");
    let sql = format!(
        "SELECT c.id, c.source, c.title, {ended}, {turns}, c.native_id,
                c.deleted_upstream_at, {msgs}, {prose}, c.cwd
         FROM conversation c
         {join}
         WHERE {openers}{filters}
         -- `NULLS LAST` is not portable to older SQLite; this expresses the same order.
         ORDER BY {ended} IS NULL, {ended} DESC
         LIMIT ?1",
        turns = crate::sittings::total("user_turns"),
        msgs = crate::sittings::total("msg_count"),
        prose = crate::sittings::total("prose_count"),
        join = crate::sittings::OF_CONVERSATION,
        openers = crate::sittings::OPENERS_ONLY,
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
            // No query, so nothing matched and there is nowhere to put a mark.
            match_seqs: Vec::new(),
            // Both filled below, once the statement holding this row is done with.
            kind_runs: Vec::new(),
            sitting: None,
            native_id: r.get(5)?,
            destinations: crate::destinations(&r.get::<_, String>(1)?, &r.get::<_, String>(5)?),
            deleted_upstream: r.get::<_, Option<i64>>(6)?.is_some(),
            hits: Vec::new(),
        })
    })?;
    let mut groups = rows.collect::<rusqlite::Result<Vec<Group>>>()?;
    drop(stmt);
    fill_sittings(conn, &mut groups)?;
    // The shape is a property of the conversation rather than of a query, so a client that
    // asked for it gets it here too. Without that, clearing the query would blank the one
    // column that made the list triageable — backwards, since this list is exactly where a
    // reader has no query to sort by.
    if shape {
        fill_shape(conn, &mut groups)?;
    }
    Ok(groups)
}

/// Name the records behind every row that is a sitting rather than a conversation.
///
/// Ahead of [`fill_shape`], which reads the membership rather than looking it up a second
/// time. Unconditional, where the shape is opt-in: this is one indexed lookup per row and
/// only for the rows that are folds, and a client cannot ask for it because it cannot know
/// which rows those are until it has the answer.
fn fill_sittings(conn: &Connection, groups: &mut [Group]) -> rusqlite::Result<()> {
    for group in groups {
        let members = crate::sittings::members(conn, &group.conv_id)?;
        if !members.is_empty() {
            group.sitting = Some(Sitting { members, gap_ms: crate::sittings::GAP_MS });
        }
    }
    Ok(())
}

/// Give every group its [`Group::kind_runs`].
///
/// A sitting's shape is its records' shapes end to end, run-length encoded across the seams
/// so a stretch of prose spanning three records reads as one run rather than three. Built by
/// concatenating the bands rather than by widening [`kind_runs`]'s query, because the order
/// messages are read in lives in [`crate::blocks::READING_ORDER`] and a second `ORDER BY`
/// that had to interleave the fold would be that rule written down twice.
fn fill_shape(conn: &Connection, groups: &mut [Group]) -> rusqlite::Result<()> {
    for group in groups {
        let bands = match &group.sitting {
            Some(sitting) => {
                let mut bands = Vec::new();
                for member in &sitting.members {
                    bands.extend(drawn_bands(conn, member)?);
                }
                bands
            }
            None => drawn_bands(conn, &group.conv_id)?,
        };
        group.kind_runs = crate::blocks::runs(bands);
    }
    Ok(())
}

/// Conversations matching `q`, best-first, with no count beside them.
///
/// Test-only. It was the public way in until chat-search-me9.36.3, and what retired it is that
/// the count is free: [`search_grouped_counted`] answers it off work the ranking pass had to do
/// anyway, so a ranking whose size cannot be reported is not a thing any caller wants. Plenty of
/// tests are about the order of the rows and nothing else, and this keeps them saying so.
#[cfg(test)]
fn grouped(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
) -> rusqlite::Result<Vec<Group>> {
    Ok(search_grouped_counted(conn, query, q)?.groups)
}

/// How many rows a query selects, as far as a search could tell for free.
///
/// Rows rather than conversations, and the two are the same number everywhere except the
/// Google Takeout activity log, where several conversations can be one [`Sitting`]. A total
/// counted the other way would say 40 beside twelve rows on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Total {
    /// The whole number. The pass that produced it saw every match, so nothing is missing.
    Exact(usize),
    /// At least this many, and how many more is not known: the ranking scan stopped at its
    /// ceiling rather than at the end of the matches.
    ///
    /// The number carried is a floor and a poor one — it is the conversations the best few
    /// thousand messages happened to come from, which on this corpus runs about half the
    /// truth. It is here to be *ranged*, not displayed as though it were the answer.
    /// [`count_matching`] settles it.
    AtLeast(usize),
}

/// A result set and what the search learned about its true size on the way past.
pub(crate) struct Counted {
    /// The conversations `limit` left room for, best-first.
    pub groups: Vec<Group>,
    /// Never below `groups.len()`, and [`Total::Exact`] with exactly that value whenever
    /// nothing was left out.
    pub matched: Total,
}

/// [`search_grouped`], and how many conversations it had to leave behind.
///
/// The count is the one thing `limit` hides that the rows cannot recover: a hundred results out
/// of a hundred and a hundred out of two thousand are the same hundred rows on screen, and only
/// a number taken against the unlimited set says which one is being read.
///
/// **This never runs a second query.** Everything it can answer, it answers off work the
/// ranking pass had to do anyway — which is every query narrow enough for the scan to reach the
/// end of its matches, meaning every query anyone finishes typing. The rest come back
/// [`Total::AtLeast`], for [`count_matching`] to settle whenever the caller judges the answer
/// worth the second pass. In a typeahead client that judgement is "once the user stops typing":
/// a broad prefix is the expensive case *and* the one whose total nobody is reading yet, so
/// spending the milliseconds there is spending them at the one moment they buy nothing.
pub(crate) fn search_grouped_counted(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
) -> rusqlite::Result<Counted> {
    if !query.is_searchable() {
        let groups = recent(conn, query, q.now_ms, q.limit, q.shape)?;
        // Always exact, and always cheap enough to take here: this branch ranks nothing, so
        // the count is a scan of `conversation` — thousands of rows, not the millions of
        // postings a term matches. Nothing to defer.
        let matched = if (groups.len() as i64) < q.limit {
            // Same rule as the ranked branch below: a short list is its own total, because
            // `recent` stops for nothing but `limit`.
            groups.len()
        } else {
            count_filtered(conn, query, q.now_ms)?
        };
        return Ok(Counted { groups, matched: Total::Exact(matched) });
    }
    let (ranked, truncated) = rank(conn, query, q)?;
    let matched =
        if truncated { Total::AtLeast(ranked.len()) } else { Total::Exact(ranked.len()) };
    Ok(Counted { groups: best_of(conn, query, q, ranked)?, matched })
}

/// The `limit` best of a ranking, with their display columns fetched.
fn best_of(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
    mut ranked: Vec<Ranked>,
) -> rusqlite::Result<Vec<Group>> {
    ranked.truncate(q.limit as usize);
    hydrate(conn, query, q.field, q.shape, ranked)
}

/// How many conversations hold a message this query matches, `limit` ignored.
///
/// What settles a [`Total::AtLeast`]. Call it when a search has come back unsettled and the
/// answer is about to be read — it is a second pass over the postings the ranking just walked,
/// which on this corpus is 5–36 ms for the broad prefixes that produce an unsettled total in
/// the first place. Pass the same [`SearchOptions`] the search was given, `now_ms` included: a
/// `date:` filter resolved against a different instant counts a different set of conversations
/// than the one on screen.
///
/// The predicate is [`rank`]'s minus the scoring and the ceiling, and it has to stay that way:
/// a count over any other set would describe a result list nobody is looking at. Nothing here
/// touches `message.text`, so the cost is the posting list and the rowid lookups, not the
/// widest column in the schema.
pub(crate) fn count_matching(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
) -> rusqlite::Result<usize> {
    crate::sittings::ensure(conn)?;
    let table = q.field.table();
    let mut binds =
        vec![Value::Text(query.match_expr()), Value::Integer(q.include_off_path as i64)];
    let filters = filter_sql(query, q.now_ms, &mut binds);
    // Distinct *rows*, which is why the fold is joined in here as well as in `rank`: counting
    // conversations would put a number beside the list that the list cannot add up to.
    let sql = format!(
        "SELECT COUNT(DISTINCT {group_id})
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         {join}
         WHERE {table} MATCH ?1
           AND (?2 = 1 OR m.on_head_path = 1){filters}",
        group_id = crate::sittings::GROUP_ID,
        join = crate::sittings::OF_MESSAGE,
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let n: i64 = stmt.query_row(params_from_iter(binds.iter()), |r| r.get(0))?;
    Ok(n as usize)
}

/// How many conversations survive the query's filters, for the branch that ranks nothing.
///
/// `date:today` with no terms is a real question with a real total, and that total is not the
/// corpus size — which is the only number a client could otherwise put beside the list.
fn count_filtered(conn: &Connection, query: &Query, now_ms: i64) -> rusqlite::Result<usize> {
    let mut binds = Vec::new();
    let filters = filter_sql(query, now_ms, &mut binds);
    // Counts what `recent` lists, join for join: one row per sitting, so the header and the
    // list below it cannot disagree.
    let sql = format!(
        "SELECT COUNT(*) FROM conversation c
         {join}
         WHERE {openers}{filters}",
        join = crate::sittings::OF_CONVERSATION,
        openers = crate::sittings::OPENERS_ONLY,
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let n: i64 = stmt.query_row(params_from_iter(binds.iter()), |r| r.get(0))?;
    Ok(n as usize)
}

/// Every matching message, scored and folded into rows, best-first.
///
/// Scores are pulled ungrouped and folded here rather than in SQL: FTS5 auxiliary functions
/// like bm25() cannot be used through a CTE or subquery, so the grouping has to happen after
/// the rows come back.
///
/// **What a row is comes from SQL, not from here.** The grouping key is
/// [`crate::sittings::GROUP_ID`] — a message's conversation, or the sitting its conversation
/// belongs to when the export shredded one chat into many. Doing it at this level rather than
/// merging finished groups afterwards is what makes the fold free of consequences: the score,
/// the match count and the positions are computed once, over the messages of the whole
/// sitting, by code that never learns a sitting exists.
///
/// The flag reports that the scan stopped at its ceiling rather than at the end of the
/// matches. That is the difference between "these are the conversations that matched" and
/// "these are the ones the best few thousand messages came from", and only the caller counting
/// them can tell whether it matters.
fn rank(
    conn: &Connection,
    query: &Query,
    q: &SearchOptions,
) -> rusqlite::Result<(Vec<Ranked>, bool)> {
    crate::sittings::ensure(conn)?;
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
        "SELECT m.id, {group_id}, {position},
                bm25({table}) / (1.0 + ?5 * (max(0, ?1 - ifnull(m.ts, ?1)) / {YEAR_MS})) AS score
         FROM {table}
         JOIN message m      ON m.rowid = {table}.rowid
         JOIN conversation c ON c.id = m.conv_id
         {join}
         WHERE {table} MATCH ?2
           AND (?3 = 1 OR m.on_head_path = 1){filters}
         ORDER BY score
         LIMIT ?4",
        group_id = crate::sittings::GROUP_ID,
        position = crate::sittings::POSITION,
        join = crate::sittings::OF_MESSAGE,
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

    // Keyed by row, which is a conversation id for all but the folded activity records — see
    // `sittings::GROUP_ID`, where the distinction is made and then stops mattering.
    let mut order: Vec<String> = Vec::new();
    let mut by_row: std::collections::HashMap<String, Vec<(String, i64, f64)>> = Default::default();
    let mut scanned = 0i64;
    for row in rows {
        let (msg_id, row_id, seq, score) = row?;
        scanned += 1;
        if !by_row.contains_key(&row_id) {
            order.push(row_id.clone());
        }
        by_row.entry(row_id).or_default().push((msg_id, seq, score));
    }

    let mut ranked: Vec<Ranked> = Vec::with_capacity(order.len());
    for conv_id in order {
        let hits = by_row.remove(&conv_id).unwrap_or_default();
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

    // A full scan is indistinguishable from one that stopped exactly on the boundary, so this
    // errs towards "truncated" — which costs a count query that returns the number already in
    // hand, rather than reporting a total that is quietly short.
    Ok((ranked, scanned >= ceiling))
}

/// One row that survived ranking, before its display columns are fetched.
struct Ranked {
    /// The conversation the row points at — the opener, when the row is a [`Sitting`]. Always
    /// a real id: nothing here is coined, which is the whole reason the fold can be a
    /// heuristic without touching ADR 16.
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
/// Rendered here rather than by each client for the same reason as the local day on a group:
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
fn hydrate(
    conn: &Connection,
    query: &Query,
    field: Field,
    shape: bool,
    ranked: Vec<Ranked>,
) -> rusqlite::Result<Vec<Group>> {
    // Every total is the row's, not the conversation's: a sitting is dated by the last record
    // in it and counted across all of them, while the title, the source and the id stay the
    // opener's. `match_density` divides by `msg_count`, and the positions `rank` produced are
    // already numbered across the sitting, so taking one record's count here would draw every
    // mark in the first tenth of the strip.
    let mut meta = conn.prepare_cached(&format!(
        "SELECT c.source, c.title, c.native_id, c.deleted_upstream_at, {ended}, {turns},
                {msgs}, {prose}, c.cwd
         FROM conversation c {join} WHERE c.id = ?1",
        ended = crate::sittings::total("ended_at"),
        turns = crate::sittings::total("user_turns"),
        msgs = crate::sittings::total("msg_count"),
        prose = crate::sittings::total("prose_count"),
        join = crate::sittings::OF_CONVERSATION,
    ))?;
    let mut msg = conn.prepare_cached(
        "SELECT m.id, m.conv_id, m.role, m.kind, m.ts, m.text, m.on_head_path,
                m.is_sidechain, m.thread_key,
                c.source, c.title, c.native_id, c.deleted_upstream_at, m.rowid
         FROM message m JOIN conversation c ON c.id = m.conv_id
         WHERE m.id = ?1",
    )?;
    let mut groups: Vec<Group> = Vec::with_capacity(ranked.len());
    // Every hit on the page, with the group it belongs to. Held back rather than marked in
    // place because the route taken to locate a match is chosen for the set as a whole
    // ([`highlight::spans_for`]), and at `limit 50 × nested 3` that set is 150 messages.
    let mut pending: Vec<(usize, Unmarked)> = Vec::new();
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

        for id in shown {
            if let Ok(u) = msg.query_row(params![id], |r| {
                Ok(Unmarked {
                    rowid: r.get(13)?,
                    text: r.get(5)?,
                    hit: Hit {
                        msg_id: r.get(0)?,
                        conv_id: r.get(1)?,
                        role: r.get(2)?,
                        kind: r.get(3)?,
                        ts: r.get(4)?,
                        snippet: String::new(),
                        snippet_spans: Vec::new(),
                        on_head_path: r.get::<_, i64>(6)? != 0,
                        is_sidechain: r.get::<_, i64>(7)? != 0,
                        thread_key: r.get(8)?,
                        source: r.get(9)?,
                        title: r.get(10)?,
                        native_id: r.get(11)?,
                        destinations: crate::destinations(
                            &r.get::<_, String>(9)?,
                            &r.get::<_, String>(11)?,
                        ),
                        deleted_upstream: r.get::<_, Option<i64>>(12)?.is_some(),
                        score,
                    },
                })
            }) {
                // The index this group is about to take, so the hits can find their way home
                // after the whole page is marked at once.
                pending.push((groups.len(), u));
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
            // Both filled below, with `meta` and `msg` released.
            kind_runs: Vec::new(),
            sitting: None,
            hits: Vec::new(),
        });
    }

    // Released before marking, which reaches for the statement cache itself. Nothing below
    // needs them, and leaving two statements checked out across that call is a trap for
    // whoever adds the third.
    drop(msg);
    drop(meta);

    fill_sittings(conn, &mut groups)?;
    if shape {
        fill_shape(conn, &mut groups)?;
    }

    // Order within a group survives because `pending` was built in `shown` order and this
    // walks it in the same order — and that order is load-bearing, since the first hit is the
    // message that won the ranking and clients truncate rather than sample.
    let (whose, unmarked): (Vec<usize>, Vec<Unmarked>) = pending.into_iter().unzip();
    for (g, hit) in whose.into_iter().zip(mark(conn, query, field, unmarked)) {
        groups[g].hits.push(hit);
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
        let groups = grouped(&conn, &Query::typeahead(""), &q).unwrap();
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
        let got = recent(&conn, &Query::exact("agent:codex"), NO_DECAY, 10, false).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|g| g.source == "codex"));

        let capped = recent(&conn, &Query::exact(""), NO_DECAY, 1, false).unwrap();
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
        // The indexer owns the postings rather than a trigger, so a fixture writes them too.
        let rowid = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO fts_prose(rowid, text) VALUES (?1, 'the borrow checker again')",
            params![rowid],
        )
        .unwrap();

        let opts = SearchOptions { limit: 5, nested: 3, ..SearchOptions::new(NO_DECAY) };
        let groups = grouped(&conn, &Query::exact("borrow"), &opts).unwrap();
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

    /// One message of a hand-shaped conversation: thread, sidechain, seq, role, kind, error.
    type Shaped<'a> = (&'a str, bool, i64, &'a str, &'a str, bool, &'a str);

    /// A conversation whose *structure* is the fixture, rather than its text.
    ///
    /// Everything the seeder above hard-codes — one thread, one role, one kind — is what the
    /// shape is made of, so it has to be spelled out here.
    fn seed_shaped(conn: &Connection, conv_id: &str, msgs: &[Shaped]) {
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, title, ended_at, user_turns,
                                      msg_count)
             VALUES (?1, 'claude-code', ?1, ?1, 1000, 1, ?2)",
            params![conv_id, msgs.len() as i64],
        )
        .unwrap();
        for (i, (thread, side, seq, role, kind, is_error, text)) in msgs.iter().enumerate() {
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, is_sidechain, seq, role, kind,
                                     is_error, ts, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1000, ?9)",
                params![
                    format!("{conv_id}:m{i}"), conv_id, thread, *side as i64, seq, role, kind,
                    *is_error as i64, text
                ],
            )
            .unwrap();
            // Prose postings only, which is where the indexer puts prose (ADR 5). A fixture
            // that indexed everything would rank rows the real thing never returns.
            if *kind == "prose" {
                let rowid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO fts_prose(rowid, text) VALUES (?1, ?2)",
                    params![rowid, text],
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn the_shape_is_read_in_the_same_order_the_transcript_is() {
        use crate::blocks::{Band, Run};
        // A conversation with a subagent in it — 4.2% of this corpus, and the only case where
        // the two possible orders disagree. `seq` restarts at 0 per thread (ADR 4), so a strip
        // ordered by `seq` alone would open with both threads' first turns side by side and
        // every position after that would name a different message than `cs show` does.
        let conn = crate::open(":memory:").unwrap();
        seed_shaped(&conn, "c:1", &[
            ("main", false, 0, "user", "prose", false, "the borrow checker"),
            ("main", false, 1, "assistant", "prose", false, "here is what I found"),
            ("main", false, 2, "assistant", "tool_call", false, "Read(schema.rs)"),
            ("sub-a", true, 0, "user", "prose", false, "check the tests too"),
            ("sub-a", true, 1, "assistant", "reasoning", false, "planning the check"),
        ]);

        let opts =
            SearchOptions { limit: 5, nested: 1, shape: true, ..SearchOptions::new(NO_DECAY) };
        let groups = grouped(&conn, &Query::exact("borrow"), &opts).unwrap();
        let shape = &groups[0].kind_runs;

        // Main thread entire, then the sidechain entire.
        assert_eq!(
            shape,
            &[Run(Band::User, 1), Run(Band::Agent, 1), Run(Band::Tool, 1), Run(Band::User, 1),
              Run(Band::Reasoning, 1)]
        );
        assert_ne!(shape[0], Run(Band::User, 2), "by `seq` alone the two openings merge");

        // And pinned against the transcript rather than only against a literal, because the
        // requirement is that these two agree — not that either matches something written
        // down once. If `blocks::load` reorders, this fails with it.
        let blocks = crate::blocks::load(&conn, "c:1", &[]).unwrap();
        assert_eq!(
            shape,
            &crate::blocks::runs(blocks.iter().filter(|b| b.drawn()).map(|b| b.band()))
        );
    }

    #[test]
    fn the_shape_counts_only_the_messages_a_reader_can_point_at() {
        use crate::blocks::Run;
        // Successful tool results are 34% of this corpus and are not drawn, so counting them
        // as strip positions would put every mark after the first tool call a third of the way
        // from where it belongs. A failed one stays, because it is a row.
        let conn = crate::open(":memory:").unwrap();
        seed_shaped(&conn, "c:1", &[
            ("main", false, 0, "user", "prose", false, "the borrow checker"),
            ("main", false, 1, "assistant", "tool_call", false, "Read(schema.rs)"),
            ("main", false, 2, "tool", "tool_result", false, "1.2 KB"),
            ("main", false, 3, "assistant", "tool_call", false, "Read(missing.rs)"),
            ("main", false, 4, "tool", "tool_result", true, "error: no such file"),
        ]);

        let opts =
            SearchOptions { limit: 5, nested: 1, shape: true, ..SearchOptions::new(NO_DECAY) };
        let groups = grouped(&conn, &Query::exact("borrow"), &opts).unwrap();
        let group = &groups[0];

        let drawn = crate::blocks::load(&conn, "c:1", &[])
            .unwrap()
            .iter()
            .filter(|b| b.drawn())
            .count();
        assert_eq!(group.kind_runs.iter().map(|Run(_, n)| n).sum::<usize>(), drawn);
        assert_eq!(drawn, 4, "one of the five results is a failure and stays");
        assert!(
            drawn < group.msg_count as usize,
            "`msg_count` counts what the strip deliberately does not — a client that used it \
             as the denominator would draw the shape short"
        );
    }

    #[test]
    fn a_search_that_did_not_ask_for_the_shape_does_not_pay_for_it() {
        // The one field that is off by default, and the reason is a measurement: filling it
        // reads every head-path message of every row returned, which is 4–30 ms at the 20–50
        // rows a terminal holds and 205 ms at 354 rows for a broad prefix. The TUI runs this
        // on every keystroke and draws `match_density` rather than a band strip.
        //
        // Pinned because the failure is silent in both directions — a default-on flag costs
        // the typeahead nothing visible, and an empty `kind_runs` reads exactly like a
        // conversation with no messages.
        let conn = crate::open(":memory:").unwrap();
        seed_shaped(&conn, "c:1", &[
            ("main", false, 0, "user", "prose", false, "the borrow checker"),
        ]);
        let opts = SearchOptions { limit: 5, nested: 1, ..SearchOptions::new(NO_DECAY) };
        let quiet = grouped(&conn, &Query::exact("borrow"), &opts).unwrap();
        assert!(quiet[0].kind_runs.is_empty(), "nobody asked for it");

        let asked = grouped(
            &conn,
            &Query::exact("borrow"),
            &SearchOptions { shape: true, ..opts },
        )
        .unwrap();
        assert_eq!(asked[0].kind_runs.len(), 1, "and asking is all it takes");
    }

    #[test]
    fn a_conversation_carries_its_shape_with_no_query_to_go_with_it() {
        // The difference between the two positional fields: `match_seqs` is a property of the
        // query and empties when there is none, while the shape is a property of the
        // conversation. The no-query list is exactly where a reader has nothing else to sort
        // on, so blanking the strip there would be backwards.
        let conn = crate::open(":memory:").unwrap();
        seed_shaped(&conn, "c:1", &[
            ("main", false, 0, "user", "prose", false, "what did I do yesterday"),
            ("main", false, 1, "assistant", "tool_call", false, "Bash(git log)"),
        ]);

        let groups = recent(&conn, &Query::exact(""), NO_DECAY, 10, true).unwrap();
        assert!(groups[0].match_seqs.is_empty(), "nothing was searched for");
        assert_eq!(
            serde_json::to_value(&groups[0].kind_runs).unwrap(),
            serde_json::json!([["user", 1], ["tool", 1]])
        );
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
            grouped(&conn, &Query::exact("widget"), &q).unwrap()[0].conv_id.clone()
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
            grouped(&conn, &Query::exact("widget"), &q).unwrap()[0].conv_id.clone()
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

    #[test]
    fn the_total_says_how_many_conversations_the_limit_left_out() {
        let conn = crate::open(":memory:").unwrap();
        for i in 0..5 {
            seed(&conn, &format!("c:{i}"), 1_000, &["widget"]);
        }
        let q = SearchOptions { limit: 2, ..SearchOptions::new(NO_DECAY) };
        let counted = search_grouped_counted(&conn, &Query::exact("widget"), &q).unwrap();
        assert_eq!(counted.groups.len(), 2, "the page is what limit allows");
        assert_eq!(counted.matched, Total::Exact(5), "the total is what the query selects");
    }

    #[test]
    fn a_result_set_the_limit_did_not_touch_is_its_own_total() {
        // The case that must not cost a second query: the ranking pass already saw every
        // matching message, so the conversations it returned are all of them.
        let conn = crate::open(":memory:").unwrap();
        for i in 0..5 {
            seed(&conn, &format!("c:{i}"), 1_000, &["widget"]);
        }
        let q = SearchOptions { limit: 50, ..SearchOptions::new(NO_DECAY) };
        let counted = search_grouped_counted(&conn, &Query::exact("widget"), &q).unwrap();
        assert_eq!(counted.matched, Total::Exact(counted.groups.len()));
        assert_eq!(counted.matched, Total::Exact(5));
    }

    #[test]
    fn a_scan_that_stopped_at_its_ceiling_reports_a_floor_rather_than_inventing_a_total() {
        // The ranking pulls a bounded number of messages, so on a broad query the
        // conversations it saw are not the conversations that matched: 500 of these 600 are
        // all it ever looks at. Publishing that as the total would be publishing the ceiling
        // as though it were a fact about the corpus.
        let conn = crate::open(":memory:").unwrap();
        for i in 0..600 {
            seed(&conn, &format!("c:{i:03}"), 1_000, &["widget"]);
        }
        let q = SearchOptions { limit: 10, ..SearchOptions::new(NO_DECAY) };
        let counted = search_grouped_counted(&conn, &Query::exact("widget"), &q).unwrap();
        assert_eq!(counted.groups.len(), 10);
        let Total::AtLeast(floor) = counted.matched else {
            panic!("a truncated scan cannot know the total: {:?}", counted.matched);
        };
        assert!(floor <= 500, "and what it does know is bounded by the scan: {floor}");

        // Which the caller settles when it decides the answer is worth a second pass.
        assert_eq!(
            count_matching(&conn, &Query::exact("widget"), &q).unwrap(),
            600,
            "every conversation that matched, not the 500 the ranking scanned"
        );
    }

    #[test]
    fn searching_never_pays_for_a_total_it_was_not_handed() {
        // The guarantee the TUI's keystroke path is built on: this call runs the ranking and
        // nothing else. `count_cost` measures it; this states it, so a future edit that moves
        // the count back inline fails here rather than only on someone's machine.
        let conn = crate::open(":memory:").unwrap();
        for i in 0..600 {
            seed(&conn, &format!("c:{i:03}"), 1_000, &["widget"]);
        }
        let q = SearchOptions { limit: 10, ..SearchOptions::new(NO_DECAY) };
        let plain = grouped(&conn, &Query::exact("widget"), &q).unwrap();
        let counted = search_grouped_counted(&conn, &Query::exact("widget"), &q).unwrap();
        let ids = |gs: &[Group]| gs.iter().map(|g| g.conv_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&plain), ids(&counted.groups), "one ranking, two views of it");
    }

    #[test]
    fn a_blank_query_is_counted_against_the_corpus_it_is_listing() {
        // Nothing is ranked here, so the total cannot come from the ranking pass. It still
        // has to be the size of the list being drawn from rather than the size of the page.
        let conn = crate::open(":memory:").unwrap();
        for i in 0..5 {
            seed(&conn, &format!("c:{i}"), 1_000 + i, &["widget"]);
        }
        let q = SearchOptions { limit: 2, ..SearchOptions::new(NO_DECAY) };
        let counted = search_grouped_counted(&conn, &Query::typeahead(""), &q).unwrap();
        assert_eq!(counted.groups.len(), 2);
        // Exact, and taken inline: this branch counts `conversation` rows rather than
        // postings, so there is nothing here worth deferring.
        assert_eq!(counted.matched, Total::Exact(5));
    }
}

/// What a search returns once an activity log has been read back as the chats it was.
///
/// [`crate::sittings`] proves the fold itself — where a sitting starts and stops. These prove
/// that the fold *arrives*: that a result set counts, positions, dates and totals a sitting as
/// one row rather than as the several conversations the index holds. The two failures are
/// different, and the second is the one chat-search-o1i.5 is about.
#[cfg(test)]
mod sitting_tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    /// One activity record: a conversation of `turns`, at one instant, under one surface.
    ///
    /// The counts on `conversation` are set by hand rather than left at their defaults,
    /// because summing them is exactly what a sitting does — a fixture that left them zero
    /// would agree with the code about nothing.
    fn record(conn: &Connection, id: &str, surface: &str, ended: i64, turns: &[(&str, &str)]) {
        let prose = turns.iter().filter(|(_, kind)| *kind == "prose").count() as i64;
        conn.execute(
            "INSERT INTO conversation(id, source, native_id, surface, title, started_at,
                                      ended_at, msg_count, prose_count, user_turns)
             VALUES (?1, 'google-takeout', ?1, ?2, ?1, ?3, ?3, ?4, ?5, 1)",
            params![id, surface, ended, turns.len() as i64, prose],
        )
        .unwrap();
        for (i, (text, kind)) in turns.iter().enumerate() {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
                 VALUES (?1, ?2, 'main', ?3, ?4, ?5, ?6, ?7)",
                params![format!("{id}:m{i}"), id, i as i64, role, kind, ended, text],
            )
            .unwrap();
            if *kind == "prose" {
                let rowid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO fts_prose(rowid, text) VALUES (?1, ?2)",
                    params![rowid, text],
                )
                .unwrap();
            }
        }
    }

    /// Three records inside one 30-minute window, all about the same thing, plus a fourth an
    /// hour later that is a different sitting and a fifth that is a different product.
    fn corpus() -> Connection {
        let conn = crate::open(":memory:").unwrap();
        record(&conn, "g:1", "gemini-apps", 0, &[("what is a monad", "prose"), ("a monoid", "prose")]);
        record(&conn, "g:2", "gemini-apps", 10 * MINUTE, &[("monad laws", "prose"), ("three of them", "prose")]);
        record(&conn, "g:3", "gemini-apps", 25 * MINUTE, &[("monad in rust", "prose"), ("no higher kinds", "prose")]);
        record(&conn, "g:4", "gemini-apps", 120 * MINUTE, &[("monad again", "prose"), ("still a monoid", "prose")]);
        record(&conn, "a:1", "ai-mode", 12 * MINUTE, &[("monad definition", "prose"), ("endofunctors", "prose")]);
        conn
    }

    fn opts() -> SearchOptions {
        SearchOptions { limit: 20, nested: 3, ..SearchOptions::new(0) }
    }

    #[test]
    fn several_records_of_one_chat_come_back_as_one_row_rather_than_as_several() {
        // The whole of chat-search-o1i.5. Five conversations hold the word; three of them were
        // one sitting at the keyboard, so the reader is owed three rows and not five.
        let conn = corpus();
        let groups = search_grouped(&conn, &Query::exact("monad"), &opts()).unwrap();
        let ids: Vec<&str> = groups.iter().map(|g| g.conv_id.as_str()).collect();
        assert_eq!(ids.len(), 3, "got {ids:?}");

        let folded = groups.iter().find(|g| g.sitting.is_some()).unwrap();
        // Keyed on the record that *opened* it, whose prompt is the row's title. Any other
        // choice would title the row with something said in the middle of the conversation.
        assert_eq!(folded.conv_id, "g:1");
        assert_eq!(folded.title.as_deref(), Some("g:1"));
        assert_eq!(folded.sitting.as_ref().unwrap().members, ["g:1", "g:2", "g:3"]);
        // Reported rather than implied: a client has to be able to say *this row is three
        // records we believe were one sitting*, and on what basis.
        assert_eq!(folded.sitting.as_ref().unwrap().gap_ms, crate::sittings::GAP_MS);

        // The hour-long silence and the other product are their own rows, and neither is a
        // fold, so both say so by carrying nothing.
        for id in ["g:4", "a:1"] {
            let row = groups.iter().find(|g| g.conv_id == id).unwrap();
            assert!(row.sitting.is_none(), "{id} is one conversation and should say so");
        }
    }

    #[test]
    fn a_sitting_is_counted_and_positioned_as_the_conversation_it_was() {
        let conn = corpus();
        let groups = search_grouped(&conn, &Query::exact("monad"), &opts()).unwrap();
        let folded = groups.iter().find(|g| g.conv_id == "g:1").unwrap();

        // Six messages and three turns, not the opener's two and one.
        assert_eq!((folded.msg_count, folded.prose_count, folded.user_turns), (6, 6, 3));
        // Positions run across the sitting rather than restarting per record, which is what
        // makes them a coordinate the strip can be drawn in: seq 0 of `g:3` is position 4.
        assert_eq!(folded.match_seqs, [0, 2, 4]);
        assert_eq!(folded.match_count, 3);
        // And the two halves agree, which is what breaks first if either is taken from the
        // opening record alone: three marks spread across the strip. Positions numbered per
        // record against a sitting-wide total would draw `▄·········`, and a sitting-wide
        // numbering against one record's total would run off the end of it.
        assert_eq!(match_density(&folded.match_seqs, folded.msg_count), "▁··▁··▁···");

        // Dated by the last record in the sitting: "when was this" means when it ended.
        assert_eq!(folded.ended_at, Some(25 * MINUTE));
    }

    #[test]
    fn the_nested_hits_stay_with_the_records_they_came_from() {
        // A sitting is a way of *reading* the index, not a rewrite of it. The row is folded;
        // the messages under it still name the conversation that actually holds them, which is
        // what lets a client open the right one.
        let conn = corpus();
        let groups = search_grouped(&conn, &Query::exact("monad"), &opts()).unwrap();
        let folded = groups.iter().find(|g| g.conv_id == "g:1").unwrap();
        let mut from: Vec<&str> = folded.hits.iter().map(|h| h.conv_id.as_str()).collect();
        from.sort_unstable();
        assert_eq!(from, ["g:1", "g:2", "g:3"]);
    }

    #[test]
    fn the_total_beside_a_result_set_counts_rows_and_not_records() {
        // A header saying "5 matched" over three rows is the same bug as the duplicate rows,
        // one layer up: the number and the list have to be counting the same thing.
        let conn = corpus();
        let query = Query::exact("monad");
        let counted = search_grouped_counted(&conn, &query, &opts()).unwrap();
        assert_eq!(counted.groups.len(), 3);
        assert_eq!(counted.matched, Total::Exact(3));
        // And the settling pass, which is a different query and therefore a second chance to
        // count the wrong thing.
        assert_eq!(count_matching(&conn, &query, &opts()).unwrap(), 3);
    }

    #[test]
    fn the_browse_list_folds_too_and_its_total_agrees_with_it() {
        // The list a typeahead opens on, where the shredding shows worst: unfolded, the
        // activity log is a third of everything a reader scrolls past.
        let conn = corpus();
        let groups = recent(&conn, &Query::exact(""), 0, 20, false).unwrap();
        let ids: Vec<&str> = groups.iter().map(|g| g.conv_id.as_str()).collect();
        // Ordered by when each row *ended*, so the sitting sorts on its last record rather
        // than on the one that opened it two hours earlier.
        assert_eq!(ids, ["g:4", "g:1", "a:1"]);
        assert_eq!(groups[1].sitting.as_ref().unwrap().members, ["g:1", "g:2", "g:3"]);

        // Filling the limit forces the count onto the second branch, which is the one that
        // asks the database rather than reading `groups.len()`.
        let counted = search_grouped_counted(
            &conn,
            &Query::exact(""),
            &SearchOptions { limit: 3, ..opts() },
        )
        .unwrap();
        assert_eq!(counted.matched, Total::Exact(3), "the browse total counts rows");
    }

    #[test]
    fn a_sittings_shape_is_its_records_end_to_end_with_the_seams_encoded_away() {
        use crate::blocks::{Band, Run};
        let conn = corpus();
        let shaped = SearchOptions { shape: true, ..opts() };
        let groups = search_grouped(&conn, &Query::exact("monad"), &shaped).unwrap();
        let folded = groups.iter().find(|g| g.conv_id == "g:1").unwrap();
        // Six bands, from three records of two — the opening record's shape alone would be
        // two, and a strip drawn from it would say "one question" about a nine-turn sitting.
        assert_eq!(
            folded.kind_runs,
            [
                Run(Band::User, 1), Run(Band::Agent, 1), Run(Band::User, 1),
                Run(Band::Agent, 1), Run(Band::User, 1), Run(Band::Agent, 1),
            ]
        );

        // And the seam is re-encoded rather than merely concatenated: two records that both
        // end and begin in the same band are one run across the join, not two. This is why
        // the bands are joined and run-length encoded once at the end, instead of each
        // record's runs being appended to the last record's.
        let conn = crate::open(":memory:").unwrap();
        record(&conn, "g:1", "gemini-apps", 0, &[("monad", "prose")]);
        record(&conn, "g:2", "gemini-apps", MINUTE, &[("monad again", "prose")]);
        let groups = search_grouped(&conn, &Query::exact("monad"), &shaped).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind_runs, [Run(Band::User, 2)]);
    }

    #[test]
    fn a_corpus_with_no_activity_log_in_it_is_unchanged_by_any_of_this() {
        // The fold has to be invisible to the 70% of this corpus that arrives as transcripts.
        // Both tables are empty for them, so every join finds nothing and every row is its
        // own conversation.
        let conn = crate::open(":memory:").unwrap();
        for (i, id) in ["c:1", "c:2"].iter().enumerate() {
            conn.execute(
                "INSERT INTO conversation(id, source, native_id, title, ended_at, msg_count,
                                          prose_count, user_turns)
                 VALUES (?1, 'codex', ?1, ?1, ?2, 1, 1, 1)",
                params![id, i as i64],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
                 VALUES (?1, ?2, 'main', 0, 'user', 'prose', 0, 'monad')",
                params![format!("{id}:m0"), id],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            conn.execute("INSERT INTO fts_prose(rowid, text) VALUES (?1, 'monad')", params![rowid])
                .unwrap();
        }
        let groups = search_grouped(&conn, &Query::exact("monad"), &opts()).unwrap();
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().all(|g| g.sitting.is_none()));
        assert!(groups.iter().all(|g| g.msg_count == 1));
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
        // The indexer owns the postings rather than a trigger, so a fixture writes them too.
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
        let mut got: Vec<String> = grouped(conn, &Query::exact(text), &opts())
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
    fn a_filtered_browse_is_counted_against_the_filter_and_not_the_corpus() {
        // Nothing ranks here, so the total cannot come from the ranking pass — and the number
        // nearest to hand is the corpus size, which would be the same silent no-op this module
        // exists to catch: it would say four while the list is drawn from two.
        let conn = corpus();
        let q = SearchOptions { limit: 1, ..opts() };
        let counted = search_grouped_counted(&conn, &Query::exact("agent:codex"), &q).unwrap();
        assert_eq!(counted.groups.len(), 1, "the page is what limit allows");
        assert_eq!(counted.matched, Total::Exact(2), "the codex half of a four-conversation corpus");
    }

    #[test]
    fn a_filter_narrows_the_recency_fallback_as_well_as_a_search() {
        // "What did I work on today" is a real question with no search terms in it. Before
        // this, an unsearchable query dropped every filter but the source and answered with
        // the whole recent list.
        let conn = corpus();
        let recent_ids = |text: &str| {
            let mut got: Vec<String> = grouped(&conn, &Query::typeahead(text), &opts())
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
                grouped(&conn, q, &opts()).unwrap().into_iter().map(|g| g.conv_id).collect();
            got.sort();
            got
        };
        assert_eq!(run(&flagged), run(&typed));
        assert_eq!(run(&flagged), ["codex-old", "codex-web"]);
        // A source named in the text wins: it was typed more recently than the flag was passed.
        let both = Query::exact("agent:gemini-cli borrow").with_source(Some("codex"));
        assert_eq!(run(&both), ["gemini-web"]);
    }

    // ---- chat-search-6eb.36: the diagnostic tool must know filters exist ----

    #[test]
    fn a_filter_token_is_never_reported_as_a_word_missing_from_the_text() {
        // It was, by splitting the raw query on whitespace: `agent:codex` came back as a term
        // present in zero messages, which is the signature of a recall problem and is nothing
        // like what happened.
        let conn = corpus();
        let e = explain(&conn, "claude-api", "agent:codex borrow", NOW).unwrap();
        let terms: Vec<&str> = e.term_hits.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(terms, ["borrow"], "only real terms are looked for in the prose");
    }

    #[test]
    fn a_conversation_a_filter_dropped_is_told_so_rather_than_blamed_on_the_stemmer() {
        // `claude-api` contains the text and is excluded by `agent:codex`. Ranking never saw
        // it, so every text-shaped verdict below is an answer about text nobody consulted —
        // and the one it used to give named the tokenizer, which is a different bug entirely.
        let conn = corpus();
        let e = explain(&conn, "claude-api", "agent:codex borrow", NOW).unwrap();
        assert!(e.excluded_by_filter);
        assert!(e.verdict.contains("excluded by a filter"), "got {:?}", e.verdict);
        assert!(!e.verdict.contains("tokenizer"), "the stemmer is not what dropped it");
        assert_eq!(e.term_hits[0].1, 1, "and the text was there all along");
    }

    #[test]
    fn a_query_of_filters_alone_is_not_a_recall_problem() {
        // No terms at all, so the all-terms-missing branch was vacuously true and reported a
        // recall failure for a query that never asked for a word.
        let conn = corpus();
        let passes = explain(&conn, "codex-web", "agent:codex", NOW).unwrap();
        assert!(!passes.excluded_by_filter, "codex-web is a codex conversation");
        assert!(passes.verdict.contains("only filters"), "got {:?}", passes.verdict);

        let dropped = explain(&conn, "claude-api", "agent:codex", NOW).unwrap();
        assert!(dropped.excluded_by_filter);
        assert!(dropped.verdict.contains("excluded by a filter"), "got {:?}", dropped.verdict);
    }

    #[test]
    fn an_unfiltered_query_still_reports_on_text_and_ranking() {
        // The regression guard on the branch order: adding a cause ahead of the others must
        // not capture the queries that have no filter at all.
        let conn = corpus();
        let e = explain(&conn, "claude-api", "borrow", NOW).unwrap();
        assert!(!e.excluded_by_filter, "no filter in the query, so nothing to be excluded by");
        assert!(e.verdict.contains("rank"), "got {:?}", e.verdict);

        let absent = explain(&conn, "claude-api", "kubernetes", NOW).unwrap();
        assert!(!absent.excluded_by_filter);
        assert!(absent.verdict.contains("recall problem"), "got {:?}", absent.verdict);
    }

    #[test]
    fn a_filter_that_selects_nothing_at_all_is_not_charged_to_this_conversation() {
        // `date:nope` is rejected by the parser and never reaches the SQL, so it cannot be
        // the reason anything is missing. Reporting it as an exclusion would send someone
        // looking for a filter that was never in force.
        let conn = corpus();
        let e = explain(&conn, "claude-api", "date:nope borrow", NOW).unwrap();
        assert!(!e.excluded_by_filter);
        assert!(e.verdict.contains("rank"), "got {:?}", e.verdict);
    }

    #[test]
    fn indexed_prose_counts_the_postings_fts5_holds_not_the_rows_it_can_read() {
        // Since `fts_prose` became external content, an unconstrained scan of it returns every
        // row of `message` — so the obvious `fts_prose JOIN message ON rowid` counted tool
        // traffic as indexed prose and could never report zero. On the real corpus that turned
        // a 175 into a 937.
        //
        // The number is worth having only while it can disagree with `message.kind`: it exists
        // to catch an index that does not hold what the indexer thinks it wrote. So it is asked
        // of fts5 rather than re-derived from the rule the indexer already applied.
        let conn = crate::open(":memory:").unwrap();
        conn.execute(
            "INSERT INTO conversation(id, source, native_id) VALUES ('c', 'codex', 'c')",
            [],
        )
        .unwrap();
        for (i, (kind, text)) in [
            ("prose", "the borrow checker again"),
            ("tool_call", "Bash(cargo build)"),
            ("tool_result", "error: cannot borrow as mutable"),
        ]
        .iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
                 VALUES (?1, 'c', 'main', ?2, 'user', ?3, 1700000000000, ?4)",
                params![format!("m{i}"), i as i64, kind, text],
            )
            .unwrap();
            let rowid = conn.last_insert_rowid();
            let table = if *kind == "prose" { "fts_prose" } else { "fts_tools" };
            conn.execute(
                &format!("INSERT INTO {table}(rowid, text) VALUES (?1, ?2)"),
                params![rowid, text],
            )
            .unwrap();
        }

        let e = explain(&conn, "c", "borrow", NOW).unwrap();
        assert_eq!(e.messages, 3);
        assert_eq!(e.prose_messages, 1);
        assert_eq!(e.indexed_prose, 1, "the other two are tool traffic and are not in fts_prose");
    }

    /// A conversation whose only mention of `gbdt` is in a reasoning message — the shape of
    /// `chatgpt-export:68c2e851`, which is what chat-search-8mb was filed about.
    fn only_in_reasoning() -> Connection {
        let conn = crate::open(":memory:").unwrap();
        conn.execute(
            "INSERT INTO conversation(id, source, native_id) VALUES ('c', 'chatgpt-export', 'c')",
            [],
        )
        .unwrap();
        for (i, (kind, text)) in [
            ("prose", "comparing the tree models on this dataset"),
            ("reasoning", "gbdt will beat the linear baseline here"),
        ]
        .iter()
        .enumerate()
        {
            conn.execute(
                "INSERT INTO message(id, conv_id, thread_key, seq, role, kind, ts, text)
                 VALUES (?1, 'c', 'main', ?2, 'assistant', ?3, 1700000000000, ?4)",
                params![format!("m{i}"), i as i64, kind, text],
            )
            .unwrap();
            // Postings only for the kinds the indexer writes them for — the point of the
            // fixture is a message that is in `message` and in no fts table.
            if *kind == "prose" {
                let rowid = conn.last_insert_rowid();
                conn.execute(
                    "INSERT INTO fts_prose(rowid, text) VALUES (?1, ?2)",
                    params![rowid, text],
                )
                .unwrap();
            }
        }
        conn
    }

    #[test]
    fn explain_tells_a_word_that_is_absent_from_one_that_is_merely_unindexed() {
        // The bug: `term_hits` counted prose alone, so a word living only in reasoning came
        // back 0 and the verdict read "no message contains any query term". True of the index,
        // false of the conversation — and it sends the reader to fix stemming or ranking for a
        // word the ranker was never shown.
        let conn = only_in_reasoning();
        let e = explain(&conn, "c", "gbdt", NOW).unwrap();

        assert_eq!(e.term_hits, vec![("gbdt".to_string(), 0)], "no *indexed* message has it");
        assert_eq!(
            e.unindexed_term_hits,
            vec![("gbdt".to_string(), 1)],
            "but the conversation plainly does"
        );
        assert!(
            e.verdict.contains("reasoning") && e.verdict.contains("no postings"),
            "the verdict has to name the cause: {:?}",
            e.verdict
        );
        assert!(
            !e.verdict.contains("no message contains"),
            "and must not still claim the word is absent: {:?}",
            e.verdict
        );
    }

    #[test]
    fn a_word_that_is_in_no_message_at_all_still_reports_a_recall_problem() {
        // The guard on the branch order. The new cause sits ahead of the recall verdict, so it
        // must not swallow the case that verdict is actually for.
        let conn = only_in_reasoning();
        let e = explain(&conn, "c", "kubernetes", NOW).unwrap();
        assert_eq!(e.unindexed_term_hits, vec![("kubernetes".to_string(), 0)]);
        assert!(e.verdict.contains("recall problem"), "got {:?}", e.verdict);
    }

    #[test]
    fn a_word_the_ranker_did_see_is_not_blamed_on_the_unindexed_kinds() {
        // `comparing` is in the prose. The reasoning branch must not fire just because the
        // conversation happens to contain reasoning at all.
        let conn = only_in_reasoning();
        let e = explain(&conn, "c", "comparing", NOW).unwrap();
        assert_eq!(e.term_hits, vec![("comparing".to_string(), 1)]);
        assert!(!e.verdict.contains("no postings"), "got {:?}", e.verdict);
    }

    #[test]
    fn unindexed_messages_are_counted_because_no_other_number_reveals_them() {
        // `messages - prose_messages` is mostly tool traffic, which *is* indexed, so a reader
        // cannot subtract their way to "how much of this was never looked at".
        let conn = only_in_reasoning();
        let e = explain(&conn, "c", "gbdt", NOW).unwrap();
        assert_eq!(e.messages, 2);
        assert_eq!(e.prose_messages, 1);
        assert_eq!(e.unindexed_messages, 1);
    }

    #[test]
    fn the_kinds_explain_calls_unindexed_are_the_ones_the_indexer_skips() {
        // Two statements of one rule is the failure this codebase names explicitly. `explain`
        // derives its list from `Kind::is_indexed`; this is the assertion that the derivation
        // is the same rule `index::write_one` applies, not a lookalike.
        let unindexed: Vec<&str> =
            Kind::ALL.iter().filter(|k| !k.is_indexed()).map(|k| k.as_str()).collect();
        assert_eq!(unindexed, vec!["reasoning"], "if this changes, chat-search-8mb changes");
        assert!(Kind::Prose.is_indexed() && Kind::ToolCall.is_indexed());
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
                    let groups = grouped(&conn, &query, &opts()).unwrap();
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

/// What the header's match total costs, measured the same way and for the same reason as
/// [`filter_cost`].
///
/// Two quantities, and the split between them is the design. A keystroke must pay *nothing* —
/// [`search_grouped_counted`] answers off the ranking pass or not at all. Settling what it
/// could not answer is a second pass over the postings, and that cost is real; it is charged
/// to the pause after typing instead, where the number is actually read.
#[cfg(test)]
mod count_cost {
    use super::*;

    #[test]
    #[ignore = "needs a real index; set CS_INDEX to an index.db"]
    fn counting_the_whole_result_set_does_not_cost_a_keystroke_its_budget() {
        let Ok(path) = std::env::var("CS_INDEX") else { return };
        let conn = Connection::open(path).expect("readable index");
        // The TUI's own shape, which is the only caller that asks for a count: `limit` 50 as
        // `cs tui` defaults it, and `nested: 0` because per keystroke it draws no snippets.
        // The limit is load-bearing here — it sets the scan ceiling, and therefore how often
        // the count is free.
        let opts =
            || SearchOptions { limit: 50, nested: 0, ..SearchOptions::new(crate::time::now_ms()) };

        // Best of seven, floor taken, exactly as `filter_cost` does — see its note on why the
        // minimum is the honest number.
        let cost = |text: &str, counting: bool| {
            let query = Query::typeahead(text);
            (0..7)
                .map(|_| {
                    let t0 = std::time::Instant::now();
                    if counting {
                        std::hint::black_box(
                            search_grouped_counted(&conn, &query, &opts()).unwrap().matched,
                        );
                    } else {
                        std::hint::black_box(grouped(&conn, &query, &opts()).unwrap().len());
                    }
                    t0.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::INFINITY, f64::min)
        };

        // What settling costs, for the queries that need it. Charged to the pause, not here.
        let settle = |text: &str| {
            let query = Query::typeahead(text);
            (0..7)
                .map(|_| {
                    let t0 = std::time::Instant::now();
                    std::hint::black_box(count_matching(&conn, &query, &opts()).unwrap());
                    t0.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::INFINITY, f64::min)
        };

        let (mut keystroke, mut pause) = (0.0f64, 0.0f64);
        println!("   search    +count   settle    total   query");
        // The blank query first: it is the frame the TUI opens on, and it takes the branch
        // that ranks nothing, so it is the one place a count could go wrong for free.
        for text in ["", "borrow checker", "fts", "ind", "rus", "tes", "con", "the"] {
            let (plain, with) = (cost(text, false), cost(text, true));
            let counted = search_grouped_counted(&conn, &Query::typeahead(text), &opts()).unwrap();
            let (mark, later) = match counted.matched {
                Total::Exact(n) => (n.to_string(), 0.0),
                Total::AtLeast(_) => {
                    let settled =
                        count_matching(&conn, &Query::typeahead(text), &opts()).unwrap();
                    (format!("…{settled}"), settle(text))
                }
            };
            println!("{plain:8.1} {with:9.1} {later:8.1} {mark:>8}   {text}");
            keystroke = keystroke.max(with - plain);
            pause = pause.max(later);
        }
        println!("worst keystroke overhead: {keystroke:.1} ms; worst settle: {pause:.1} ms");
        // The invariant the whole split exists to hold: a keystroke pays for the ranking and
        // nothing else, whatever the query. This is the only place an inline count would show
        // up — it costs nothing a correctness test can see.
        assert!(keystroke < 3.0, "a keystroke paid {keystroke:.1} ms towards a total");
        // And the deferred half stays bounded: it is one pass over the postings the ranking
        // just walked, so it cannot run away from the search that precedes it.
        assert!(pause < 60.0, "settling a total took {pause:.1} ms");
    }
}

/// What reading the activity log back as chats costs, and what it buys.
///
/// Measured the same way and for the same reason as [`filter_cost`]. Three quantities:
///
/// * **`ensure`, warm.** Charged to every search, so it is the number that has to be near
///   zero. It is two cached statements — a temp-schema lookup and a count over
///   `idx_conversation_source` — deliberately in place of loading a map per query.
/// * **`ensure`, cold.** Charged to the first search on a connection, and again after a
///   reindex. This is why the fold is a table built once and not a map rebuilt per keystroke.
/// * **the fold against the joins alone.** Emptying the two tables leaves every `LEFT JOIN`
///   in place and finding nothing, which is exactly what a corpus with no Google Takeout in
///   it pays. The difference between that and the populated fold is what the Gemini records
///   cost — and beside it, the rows they save.
///
/// What this cannot measure from inside is the joins themselves, since removing them is a
/// different build. Against `63a8b31` on the same 4,377-conversation index, at `limit` 50:
/// `the` 65.0 -> 70.5 ms, `con` 41.3 -> 43.4, `tes` 29.4 -> 31.0, `rus` 23.2 -> 23.3,
/// `ind` 13.4 -> 12.9, blank 0.9 -> 1.9. So the joins are 5–8% on the broadest prefixes and
/// nothing on the narrow ones, against 17% fewer rows for `the` (4,243 -> 3,511).
///
/// This is the third `--ignored` benchmark in this file and they contend: run them with
/// `--test-threads=1`, or each on its own. Three timing loops racing for the same index put
/// [`count_cost`] four times over its budget on `63a8b31` too, so a failure from running them
/// together is measuring the harness rather than the code.
#[cfg(test)]
mod sitting_cost {
    use super::*;

    #[test]
    #[ignore = "needs a real index; set CS_INDEX to an index.db"]
    fn folding_an_activity_log_does_not_cost_a_keystroke_its_budget() {
        let Ok(path) = std::env::var("CS_INDEX") else { return };
        let conn = Connection::open(path).expect("readable index");
        // The TUI's own shape, as [`count_cost`] uses it: `limit` 50, and `nested: 0` because
        // per keystroke it draws no snippets. Building them swings ±10 ms between runs on a
        // narrow query and none of that swing is the fold, so it would be noise standing in
        // front of the thing being measured.
        let opts =
            || SearchOptions { limit: 50, nested: 0, ..SearchOptions::new(crate::time::now_ms()) };

        let ms = |f: &dyn Fn()| {
            let t0 = std::time::Instant::now();
            f();
            t0.elapsed().as_secs_f64() * 1000.0
        };
        // Best of seven, floor taken, exactly as `filter_cost` does — see its note on why the
        // minimum is the honest number.
        let best = |f: &dyn Fn()| (0..7).map(|_| ms(f)).fold(f64::INFINITY, f64::min);

        let cold = ms(&|| crate::sittings::ensure(&conn).unwrap());
        let warm = best(&|| crate::sittings::ensure(&conn).unwrap());
        println!("ensure: {cold:.1} ms cold, {warm:.2} ms warm");

        let search = |text: &str| {
            let query = Query::typeahead(text);
            best(&|| {
                std::hint::black_box(search_grouped(&conn, &query, &opts()).unwrap());
            })
        };
        // The whole row count, settled: a blank query takes the branch that ranks nothing and
        // has no `MATCH` expression to count with, and a broad prefix comes back a floor.
        let total = |text: &str| {
            let query = Query::typeahead(text);
            match search_grouped_counted(&conn, &query, &opts()).unwrap().matched {
                Total::Exact(n) => n,
                Total::AtLeast(_) => count_matching(&conn, &query, &opts()).unwrap(),
            }
        };

        let queries = ["", "borrow checker", "gemini", "monad", "ind", "the"];
        // Warmed before either pass. The two passes cannot run at once, so whichever goes
        // first would otherwise be charged for pulling the postings off disk — which on a
        // 345 MB index is larger than the difference being measured, and lands entirely on
        // the fold, since the fold has to be measured before the tables are emptied.
        for text in queries {
            std::hint::black_box(search_grouped(&conn, &Query::typeahead(text), &opts()).unwrap());
        }
        let with: Vec<(f64, usize)> = queries.iter().map(|t| (search(t), total(t))).collect();

        // The tables emptied rather than dropped: `ensure` rebuilds on a changed fingerprint,
        // not on an empty table, so this leaves every join in place and finding nothing —
        // which is the shape of the query plan for a corpus that has no activity log at all.
        conn.execute_batch("DELETE FROM conv_sitting; DELETE FROM sitting;").unwrap();
        let mut worst = 0.0f64;
        println!("    joins     fold    rows   folded   query");
        for (text, (fold_ms, fold_n)) in queries.iter().zip(with) {
            let bare_ms = search(text);
            let bare_n = total(text);
            println!("{bare_ms:9.1}{fold_ms:9.1}{bare_n:8}{fold_n:9}   {text}");
            worst = worst.max(fold_ms - bare_ms);
        }
        println!("worst the fold added: {worst:.1} ms");

        // The invariant the temp table exists to hold: what every search pays is a lookup, not
        // a rebuild, and a rebuild is what the first one pays instead.
        // Measured at 0.04 ms warm and 8–24 ms cold — 132 ms once, on a cold page cache,
        // which is what the ceiling below leaves room for.
        assert!(warm < 1.0, "every search paid {warm:.2} ms to check the fold was current");
        assert!(cold < 300.0, "building the fold took {cold:.1} ms");
        // And the fold itself stays inside the noise of the joins that carry it: measured at
        // 0.9–2.9 ms across runs. It is a thousand-row table joined by primary key, so
        // anything much above this means a plan changed, not that there is more Gemini in the
        // corpus.
        assert!(worst < 8.0, "the fold added {worst:.1} ms to a keystroke");
    }
}
