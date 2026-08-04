//! `cs eval` — the query set, the judging loop, and the score.
//!
//! Split across two files on purpose, because they have opposite lifecycles.
//!
//! The **set** (`ranking.toml`) is written by hand and never rewritten by this tool, so the
//! comments explaining why a query is in it survive. The **judgements**
//! (`ranking.judgments.jsonl`) are append-only and machine-written, last entry winning per
//! `(query id, conversation id)` — the same fold the archive manifest uses. A judging
//! session can therefore be abandoned halfway without losing the answers already given, and
//! changing your mind about a conversation is a new line rather than an edit.

use anyhow::{bail, Context, Result};
use cs_core::eval::{report, score_query, Grade, Judged, QueryScore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Results per query the judge pools and the score reads. Ten is what a person scrolls
/// before giving up, which is the thing being measured.
pub const DEPTH: usize = 10;

// ------------------------------------------------------------------ the set

#[derive(Debug, Deserialize)]
pub struct EvalSet {
    #[serde(default)]
    pub query: Vec<EvalQuery>,
}

#[derive(Debug, Deserialize)]
pub struct EvalQuery {
    /// Stable handle for the query, so judgements survive a reworded `q`.
    pub id: String,
    pub q: String,
    /// Why this query is in the set. Read back during judging as a reminder of what was
    /// being looked for, which is the thing that decays fastest between sessions.
    #[serde(default)]
    pub note: Option<String>,
}

impl EvalSet {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let set: EvalSet = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;

        let mut seen = std::collections::HashSet::new();
        for q in &set.query {
            if !seen.insert(&q.id) {
                bail!("duplicate query id {:?} in {}", q.id, path.display());
            }
            if q.q.trim().is_empty() {
                bail!("query {:?} in {} has no text", q.id, path.display());
            }
        }
        if set.query.is_empty() {
            bail!("{} contains no queries", path.display());
        }
        Ok(set)
    }
}

// ----------------------------------------------------------- the judgements

/// One answer, as given. `q` rides along so a judgement made against a since-reworded query
/// can be spotted rather than silently reused.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Judgement {
    pub id: String,
    pub q: String,
    pub conv_id: String,
    pub grade: u8,
    pub ts: i64,
}

/// Where judgements for a set live: `<set>.judgments.jsonl` beside it.
pub fn judgments_path(set_path: &Path) -> PathBuf {
    let stem = set_path.file_stem().map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "eval".into());
    set_path.with_file_name(format!("{stem}.judgments.jsonl"))
}

/// The folded current state: the most recent grade per `(query id, conversation id)`.
///
/// A malformed line is skipped rather than fatal. The file is append-only and hand-editable,
/// and losing a whole judging history to one bad line would be a worse failure than ignoring
/// it — the count of skipped lines is reported so it cannot pass unnoticed.
pub fn load_judgements(path: &Path) -> Result<(HashMap<String, HashMap<String, Grade>>, usize)> {
    let mut out: HashMap<String, HashMap<String, Grade>> = HashMap::new();
    let mut skipped = 0usize;
    let Ok(file) = std::fs::File::open(path) else {
        return Ok((out, 0)); // nothing judged yet is a normal state, not an error
    };
    for line in std::io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Judgement>(&line) {
            Ok(j) => match Grade::from_u8(j.grade) {
                Some(g) => {
                    out.entry(j.id).or_default().insert(j.conv_id, g);
                }
                None => skipped += 1,
            },
            Err(_) => skipped += 1,
        }
    }
    Ok((out, skipped))
}

fn append_judgement(path: &Path, j: &Judgement) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(f, "{}", serde_json::to_string(j)?)?;
    Ok(())
}

// --------------------------------------------------------------- the pool

/// A ranking configuration to pool candidates from.
///
/// Judging only what the shipping ranker returns is how a tuning run proves the shipping
/// ranker optimal: a conversation the current constants bury is never seen, never judged,
/// and so counts as irrelevant for every configuration tried afterwards. These three differ
/// in exactly the two constants chat-search-6eb.13 exists to move.
struct Variant {
    name: &'static str,
    repeat_weight: f64,
    decay: f64,
}

fn pool_variants() -> Vec<Variant> {
    vec![
        Variant { name: "default", repeat_weight: cs_core::REPEAT_WEIGHT, decay: cs_core::DECAY },
        // Recency-blind: surfaces the old conversation that the decay pushed under a newer,
        // weaker one. Without this the decay can only ever look correct.
        Variant { name: "no-recency", repeat_weight: cs_core::REPEAT_WEIGHT, decay: 0.0 },
        // Pure max: surfaces the short, precise conversation that a long agent session
        // outweighed on volume. Without this REPEAT_WEIGHT can only ever look correct.
        Variant { name: "pure-max", repeat_weight: 0.0, decay: cs_core::DECAY },
    ]
}

fn run_query(
    conn: &rusqlite::Connection,
    text: &str,
    limit: i64,
    repeat_weight: f64,
    decay: f64,
) -> Result<Vec<cs_core::Group>> {
    let q = cs_core::SearchOptions {
        limit,
        repeat_weight,
        decay,
        nested: 1,
        ..cs_core::SearchOptions::new(cs_core::now_ms())
    };
    Ok(cs_core::search_grouped(conn, &cs_core::Query::exact(text), &q)?)
}

/// Every conversation any variant surfaced in its top `depth`, deduplicated by id.
///
/// Whole `Group`s rather than ids, because the sheet wants what a real result row shows —
/// title, source, local date, turn count and the snippet the query actually matched.
fn pool(conn: &rusqlite::Connection, text: &str, depth: usize) -> Result<Vec<cs_core::Group>> {
    let mut out: Vec<cs_core::Group> = Vec::new();
    for v in pool_variants() {
        for g in run_query(conn, text, depth as i64, v.repeat_weight, v.decay)? {
            if !out.iter().any(|o| o.conv_id == g.conv_id) {
                out.push(g);
            }
        }
        let _ = v.name;
    }
    Ok(out)
}

// -------------------------------------------------------------- the sheet

/// One sheet per query, under `sheets/` beside the set.
///
/// Per query rather than one big file so that re-emitting after a ranking change rewrites
/// only the queries whose pool actually moved, and so a half-finished pass is visible as
/// which files still contain a `_`.
pub fn sheets_dir(set_path: &Path) -> PathBuf {
    set_path.with_file_name("sheets")
}

fn sheet_path(set_path: &Path, query_id: &str) -> PathBuf {
    sheets_dir(set_path).join(format!("{query_id}.md"))
}

/// The character in the grade column meaning "not decided yet".
const UNGRADED: char = '_';

/// Render a sheet for one query.
///
/// Candidates are ordered by date, newest first, and carry no rank. Two reasons, and the
/// second is the one that matters. Ordering by rank would anchor the judgement to the
/// ranking being measured, which is the bias the whole pool exists to avoid. And a
/// rank-ordered sheet reshuffles every time the ranking changes, so a half-graded file
/// becomes unreadable exactly when you re-emit it; date order holds still.
fn render_sheet(eq: &EvalQuery, groups: &[cs_core::Group], have: &HashMap<String, Grade>) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {} · {:?}\n#\n", eq.id, eq.q));
    if let Some(n) = &eq.note {
        s.push_str(&format!("# {n}\n#\n"));
    }
    s.push_str("# Put a grade in the first column of each block:\n");
    // Generated from the enum rather than typed out, so the legend on the sheet and the
    // grades the parser accepts cannot drift apart.
    let legend: Vec<String> = [Grade::Exact, Grade::Useful, Grade::Related, Grade::Irrelevant]
        .iter()
        .map(|g| format!("{} {}", g.as_u8(), g.name()))
        .collect();
    s.push_str(&format!("#   {}\n", legend.join(" · ")));
    s.push_str(&format!(
        "# Leave {UNGRADED} to decide later. Everything outside the grade column is ignored,\n"
    ));
    s.push_str("# so notes to yourself are safe anywhere.\n#\n");
    s.push_str(&format!("# {} candidates · cs eval collect when done\n\n", groups.len()));

    for g in groups {
        let grade = have.get(&g.conv_id).map_or(UNGRADED, |v| {
            char::from_digit(v.as_u8() as u32, 10).unwrap_or(UNGRADED)
        });
        s.push_str(&format!("{grade}  {}\n", g.conv_id));
        s.push_str(&format!("   {}\n", trunc(g.title.as_deref().unwrap_or("(untitled)"), 92)));
        // Where the matches sit matters more than a third snippet would. Three hits bunched
        // at the start of a forty-message conversation is the subject; three at the very end
        // is an aside raised on the way out, and the two deserve different grades.
        s.push_str(&format!(
            "   {} · {} · {} turns · {} of {} prose matched  {}\n",
            g.source,
            g.ended_date.as_deref().unwrap_or("????-??-??"),
            g.user_turns,
            g.match_count,
            g.prose_count,
            cs_core::match_density(&g.match_seqs, g.msg_count),
        ));
        // The snippet the ranking actually matched on, not the conversation's opening line.
        // Judging "is this relevant to my query" against text picked without reference to the
        // query was the interactive loop's worst habit.
        if let Some(h) = g.hits.first() {
            s.push_str(&format!("   {}\n", trunc(&h.snippet, 150)));
        }
        s.push('\n');
    }
    s
}

/// One graded row read back off a sheet.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    conv_id: String,
    grade: Option<Grade>,
}

/// Pull the grade column out of a sheet.
///
/// A row is a line whose first character is a grade digit or `_`, followed by whitespace and
/// a conversation id. Everything else is ignored, so the header, the blank lines, the title
/// and snippet lines, and any note added while judging all pass through harmlessly.
fn parse_sheet(text: &str) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (n, line) in text.lines().enumerate() {
        let mut chars = line.chars();
        let Some(first) = chars.next() else { continue };
        if first != UNGRADED && !first.is_ascii_digit() {
            continue;
        }
        // A row is the grade character, then whitespace, then the id. Without the whitespace
        // this would also swallow indented prose that happens to start with a digit.
        let rest = chars.as_str();
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(conv_id) = rest.split_whitespace().next() else { continue };

        let grade = match first {
            UNGRADED => None,
            d => match d.to_digit(10).and_then(|v| Grade::from_u8(v as u8)) {
                Some(g) => Some(g),
                // Deliberately fatal. A stray '7' is a typo in the one column that matters,
                // and quietly skipping the line would drop a judgement that was made.
                None => bail!(
                    "line {}: {d:?} is not a grade — use 0, 1, 2, 3 or {UNGRADED}",
                    n + 1
                ),
            },
        };
        if !seen.insert(conv_id.to_string()) {
            bail!("line {}: {conv_id} appears twice in this sheet", n + 1);
        }
        rows.push(Row { conv_id: conv_id.to_string(), grade });
    }
    Ok(rows)
}

// ---------------------------------------------------------------- commands

pub fn sheet(
    config_path: &Path,
    db_path: Option<PathBuf>,
    set_path: &Path,
    depth: usize,
    only: Option<&str>,
    force: bool,
) -> Result<()> {
    let set = EvalSet::load(set_path)?;
    let conn = open_index(config_path, db_path)?;
    let jpath = judgments_path(set_path);
    let (judged, skipped) = load_judgements(&jpath)?;
    if skipped > 0 {
        println!("  warning: {skipped} unreadable line(s) in {} were ignored", jpath.display());
    }

    let queries: Vec<&EvalQuery> =
        set.query.iter().filter(|q| only.is_none_or(|o| o == q.id)).collect();
    if queries.is_empty() {
        bail!("no query matches id {:?}", only.unwrap_or(""));
    }

    let dir = sheets_dir(set_path);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let empty = HashMap::new();
    let (mut written, mut kept) = (0usize, 0usize);
    for eq in queries {
        let path = sheet_path(set_path, &eq.id);
        let have = judged.get(&eq.id).unwrap_or(&empty);

        // Refuse to overwrite grades that were typed into a sheet and never collected. This
        // is the one way the flow can lose work, and it costs a read to prevent.
        if !force && path.exists() {
            let existing = parse_sheet(&std::fs::read_to_string(&path)?)?;
            let uncollected = existing
                .iter()
                .filter(|r| r.grade.is_some() && have.get(&r.conv_id) != r.grade.as_ref())
                .count();
            if uncollected > 0 {
                println!(
                    "  {:<24} {uncollected} ungathered grade(s) — run `cs eval collect` first",
                    eq.id
                );
                kept += 1;
                continue;
            }
        }

        let mut groups = pool(&conn, &eq.q, depth)?;
        // Newest first, ties by id so the file is byte-stable across re-emits.
        groups.sort_by(|a, b| b.ended_at.cmp(&a.ended_at).then_with(|| a.conv_id.cmp(&b.conv_id)));
        std::fs::write(&path, render_sheet(eq, &groups, have))
            .with_context(|| format!("writing {}", path.display()))?;
        written += 1;
    }

    println!("{written} sheet(s) in {}", dir.display());
    if kept > 0 {
        println!("  {kept} left alone to avoid discarding ungathered grades");
    }
    if written > 0 {
        println!("  edit the grade column, then: cs eval collect --set {}", set_path.display());
    }
    Ok(())
}

pub fn collect(
    config_path: &Path,
    db_path: Option<PathBuf>,
    set_path: &Path,
    allow_unknown: bool,
) -> Result<()> {
    let set = EvalSet::load(set_path)?;
    let conn = open_index(config_path, db_path)?;
    let jpath = judgments_path(set_path);
    let (judged, _) = load_judgements(&jpath)?;
    let dir = sheets_dir(set_path);

    let empty = HashMap::new();
    let mut pending: Vec<Judgement> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut unchanged = 0usize;
    let mut read = 0usize;

    for eq in &set.query {
        let path = sheet_path(set_path, &eq.id);
        if !path.exists() {
            continue;
        }
        read += 1;
        let rows = parse_sheet(&std::fs::read_to_string(&path)?)
            .with_context(|| format!("reading {}", path.display()))?;
        let have = judged.get(&eq.id).unwrap_or(&empty);

        for r in rows {
            let Some(g) = r.grade else { continue };
            if have.get(&r.conv_id) == Some(&g) {
                unchanged += 1;
                continue;
            }
            // Ids are written into the sheet by `cs eval sheet`, so one the index does not
            // know is almost always a hand-edit that mangled it rather than a real drift.
            if !conversation_exists(&conn, &r.conv_id)? {
                unknown.push(format!("{}: {}", eq.id, r.conv_id));
            }
            pending.push(Judgement {
                id: eq.id.clone(),
                q: eq.q.clone(),
                conv_id: r.conv_id,
                grade: g.as_u8(),
                ts: cs_core::now_ms(),
            });
        }
    }

    if read == 0 {
        println!("no sheets in {}", dir.display());
        println!("  cs eval sheet --set {}", set_path.display());
        return Ok(());
    }

    if !unknown.is_empty() && !allow_unknown {
        // Nothing is written, so fixing the sheet and re-running loses nothing.
        println!("{} graded id(s) are not in this index:", unknown.len());
        for u in unknown.iter().take(10) {
            println!("  {u}");
        }
        if unknown.len() > 10 {
            println!("  … and {} more", unknown.len() - 10);
        }
        bail!(
            "refusing to record {} judgement(s). Fix the ids, or pass --allow-unknown if the \
             index really has moved on.",
            pending.len()
        );
    }

    for j in &pending {
        append_judgement(&jpath, j)?;
    }

    println!("{} judgement(s) from {read} sheet(s)", pending.len());
    if unchanged > 0 {
        println!("  {unchanged} already recorded at the same grade, so nothing was rewritten");
    }
    if !unknown.is_empty() {
        println!("  {} recorded against ids not in this index", unknown.len());
    }
    if !pending.is_empty() {
        println!("  cs eval run --set {}", set_path.display());
    }
    Ok(())
}

pub fn run(
    config_path: &Path,
    db_path: Option<PathBuf>,
    set_path: &Path,
    depth: usize,
    repeat_weight: f64,
    decay: f64,
    json: bool,
) -> Result<()> {
    let set = EvalSet::load(set_path)?;
    let conn = open_index(config_path, db_path)?;
    let jpath = judgments_path(set_path);
    let (judged, skipped) = load_judgements(&jpath)?;

    let empty = HashMap::new();
    let mut scores: Vec<QueryScore> = Vec::with_capacity(set.query.len());
    // A judged conversation the index no longer holds means the set and the index have
    // drifted apart — a reimport that changed native ids, or a source that stopped being
    // indexed. Every such id scores as a miss, so it has to be said out loud.
    let mut dangling: Vec<String> = Vec::new();

    let t0 = std::time::Instant::now();
    for eq in &set.query {
        let grades = judged.get(&eq.id).unwrap_or(&empty);
        for conv_id in grades.keys() {
            if !conversation_exists(&conn, conv_id)? {
                dangling.push(format!("{}:{}", eq.id, conv_id));
            }
        }
        let returned: Vec<String> = run_query(&conn, &eq.q, depth as i64, repeat_weight, decay)?
            .into_iter()
            .map(|g| g.conv_id)
            .collect();
        scores.push(score_query(
            &Judged { id: &eq.id, text: &eq.q, grades, returned: &returned },
            depth,
        ));
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let rep = report(scores, depth);

    if json {
        println!("{:#}", serde_json::json!({
            "set": set_path, "depth": depth,
            "repeat_weight": repeat_weight, "decay": decay,
            "ms": (ms * 100.0).round() / 100.0,
            "unreadable_judgement_lines": skipped,
            "dangling": dangling,
            "report": rep,
        }));
        return Ok(());
    }

    println!("  {:<28} {:>7} {:>6} {:>5} {}", "query", "nDCG", "1/rank", "top", "");
    for s in &rep.queries {
        let ndcg = s.ndcg.map(|v| format!("{v:.3}")).unwrap_or_else(|| "  —  ".into());
        let rr = s.reciprocal_rank.map(|v| format!("{v:.2}")).unwrap_or_else(|| " — ".into());
        let rank = s.first_hit_rank.map(|r| r.to_string()).unwrap_or_else(|| "—".into());
        let mut flags = Vec::new();
        if s.ndcg.is_none() { flags.push("unjudged".to_string()) }
        if s.unjudged > 0 { flags.push(format!("{} new", s.unjudged)) }
        if !s.missed.is_empty() { flags.push(format!("{} missed", s.missed.len())) }
        println!("  {:<28} {ndcg:>7} {rr:>6} {rank:>5}  {}", trunc(&s.id, 28), flags.join(" · "));
    }

    println!();
    println!("  nDCG@{:<3}      {:.3}   over {} scorable quer(y/ies)", rep.depth, rep.ndcg, rep.scored);
    println!("  MRR           {:.3}", rep.mrr);
    println!("  success@1     {:.0}%", rep.success_at_1 * 100.0);
    println!("  success@5     {:.0}%", rep.success_at_5 * 100.0);
    println!("  exact@1       {:.0}%", rep.exact_at_1 * 100.0);
    println!("  coverage      {:.0}%   of the top {} carries a judgement", rep.coverage * 100.0, rep.depth);
    println!("\n  repeat_weight {repeat_weight} · decay {decay} · {:.0} ms", ms);

    // Everything below is a reason not to believe the numbers above, so it goes last.
    if rep.unscorable > 0 {
        println!("\n  {} quer(y/ies) have no positive judgement and are excluded from nDCG.", rep.unscorable);
        println!("  cs eval sheet --set {}", set_path.display());
    }
    if rep.coverage < 0.8 {
        println!("\n  warning: coverage is {:.0}%. Below ~80% the pool is too thin for these", rep.coverage * 100.0);
        println!("  numbers to compare configurations — unjudged results score as irrelevant.");
    }
    if skipped > 0 {
        println!("\n  warning: {skipped} unreadable line(s) in {}", jpath.display());
    }
    if !dangling.is_empty() {
        println!("\n  warning: {} judged conversation(s) are not in this index, and every one", dangling.len());
        println!("  of them scores as a miss. The set and the index have drifted apart:");
        for d in dangling.iter().take(5) {
            println!("    {d}");
        }
        if dangling.len() > 5 {
            println!("    … and {} more", dangling.len() - 5);
        }
    }
    Ok(())
}

// ----------------------------------------------------------------- helpers

/// Whether the index holds this conversation.
///
/// The sheet gets everything it displays from the `Group`s the pool already returned, so the
/// only thing left to ask the index is whether a hand-typed id names anything real.
fn conversation_exists(conn: &rusqlite::Connection, conv_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM conversation WHERE id = ?1)",
        rusqlite::params![conv_id],
        |r| r.get(0),
    )?)
}

/// Truncate on char boundaries, not bytes — titles are full of em dashes and code points
/// well outside ASCII, and slicing one in half panics.
fn trunc(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    format!("{}…", chars[..n.saturating_sub(1)].iter().collect::<String>())
}

fn open_index(config_path: &Path, db_path: Option<PathBuf>) -> Result<rusqlite::Connection> {
    let cfg = cs_archive::Config::load(config_path)?;
    let path = db_path.unwrap_or_else(|| cfg.default_db());
    rusqlite::Connection::open(&path)
        .with_context(|| format!("opening {} (run `cs index` first?)", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cs-eval-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn judgements_live_beside_the_set_they_belong_to() {
        assert_eq!(
            judgments_path(Path::new("/e/ranking.toml")),
            PathBuf::from("/e/ranking.judgments.jsonl")
        );
    }

    #[test]
    fn the_last_grade_for_a_conversation_wins() {
        // Changing your mind is an append, not an edit — which is the only reason it is
        // safe to abandon a judging session halfway.
        let dir = tmpdir();
        let p = dir.join("r.judgments.jsonl");
        let j = |conv: &str, grade: u8, ts: i64| Judgement {
            id: "q1".into(), q: "borrow checker".into(),
            conv_id: conv.into(), grade, ts,
        };
        append_judgement(&p, &j("codex:a", 1, 1)).unwrap();
        append_judgement(&p, &j("codex:b", 3, 2)).unwrap();
        append_judgement(&p, &j("codex:a", 3, 3)).unwrap();

        let (got, skipped) = load_judgements(&p).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(got["q1"]["codex:a"], Grade::Exact, "the later line wins");
        assert_eq!(got["q1"]["codex:b"], Grade::Exact);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_line_costs_that_line_and_not_the_history() {
        let dir = tmpdir();
        let p = dir.join("r.judgments.jsonl");
        std::fs::write(&p, concat!(
            r#"{"id":"q1","q":"x","conv_id":"codex:a","grade":3,"ts":1}"#, "\n",
            "not json at all\n",
            "\n",
            r#"{"id":"q1","q":"x","conv_id":"codex:b","grade":9,"ts":2}"#, "\n",
            r#"{"id":"q1","q":"x","conv_id":"codex:c","grade":2,"ts":3}"#, "\n",
        )).unwrap();

        let (got, skipped) = load_judgements(&p).unwrap();
        assert_eq!(skipped, 2, "the garbage line and the out-of-range grade");
        assert_eq!(got["q1"].len(), 2);
        assert_eq!(got["q1"]["codex:a"], Grade::Exact);
        assert_eq!(got["q1"]["codex:c"], Grade::Useful);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nothing_judged_yet_is_a_normal_state_rather_than_an_error() {
        let (got, skipped) = load_judgements(Path::new("/nonexistent/r.judgments.jsonl")).unwrap();
        assert!(got.is_empty());
        assert_eq!(skipped, 0);
    }

    #[test]
    fn a_set_that_would_silently_misattribute_judgements_is_rejected() {
        let dir = tmpdir();
        let dup = dir.join("dup.toml");
        std::fs::write(&dup, "[[query]]\nid='a'\nq='one'\n[[query]]\nid='a'\nq='two'\n").unwrap();
        let err = EvalSet::load(&dup).unwrap_err().to_string();
        assert!(err.contains("duplicate query id"), "got: {err}");

        let blank = dir.join("blank.toml");
        std::fs::write(&blank, "[[query]]\nid='a'\nq='   '\n").unwrap();
        assert!(EvalSet::load(&blank).unwrap_err().to_string().contains("no text"));

        let none = dir.join("none.toml");
        std::fs::write(&none, "# nothing here yet\n").unwrap();
        assert!(EvalSet::load(&none).unwrap_err().to_string().contains("no queries"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_set_round_trips_with_its_notes() {
        let dir = tmpdir();
        let p = dir.join("r.toml");
        std::fs::write(&p, concat!(
            "[[query]]\n",
            "id = 'borrow-checker'\n",
            "q = 'borrow checker lifetimes'\n",
            "note = 'the one where it finally clicked'\n\n",
            "[[query]]\n",
            "id = 'fts5-ranking'\n",
            "q = 'fts5 bm25'\n",
        )).unwrap();
        let set = EvalSet::load(&p).unwrap();
        assert_eq!(set.query.len(), 2);
        assert_eq!(set.query[0].note.as_deref(), Some("the one where it finally clicked"));
        assert_eq!(set.query[1].note, None, "a note is optional");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_title() {
        assert_eq!(trunc("hello", 10), "hello");
        let em = "a — b — c — d";
        let cut = trunc(em, 5);
        assert!(cut.ends_with('…') && cut.chars().count() == 5);
    }

    fn row(conv_id: &str, grade: Option<Grade>) -> Row {
        Row { conv_id: conv_id.to_string(), grade }
    }

    fn group(conv_id: &str, title: &str) -> cs_core::Group {
        cs_core::Group {
            conv_id: conv_id.into(),
            source: "codex".into(),
            title: Some(title.into()),
            cwd: None,
            ended_at: Some(0),
            ended_date: Some("2026-01-01".into()),
            user_turns: 3,
            msg_count: 10,
            prose_count: 6,
            score: -1.0,
            match_count: 1,
            match_seqs: vec![2],
            kind_runs: Vec::new(),
            native_id: "c1".into(),
            destinations: Vec::new(),
            deleted_upstream: false,
            hits: Vec::new(),
        }
    }

    #[test]
    fn the_grade_column_is_read_and_everything_else_on_the_sheet_is_not() {
        // The whole point of the format: only column one carries meaning, so the header, the
        // titles, the snippets and any note added while judging are all inert.
        let sheet = "\
# camping · \"camping trip\"
#   3 exact · 2 useful · 1 related · 0 irrelevant

3  chatgpt-export:aaa
   Camping Trip Options: Seattle
   chatgpt-export · 2024-05-22 · 3 turns
   …want to go on a two-night camping trip in June…

_  chatgpt-export:bbb
   Camino Packing Guide
   2 of these are about the Camino, not camping — check later

0  codex:ccc
   Refine Early Career Approach
";
        assert_eq!(parse_sheet(sheet).unwrap(), vec![
            row("chatgpt-export:aaa", Some(Grade::Exact)),
            row("chatgpt-export:bbb", None),
            row("codex:ccc", Some(Grade::Irrelevant)),
        ]);
    }

    #[test]
    fn an_indented_line_that_starts_with_a_digit_is_prose_not_a_grade() {
        // "   2 of these are about…" above must not read as grading something "of".
        // The rule is the grade sits in column one, so anything indented is text.
        let rows = parse_sheet("   3 this is a snippet that opens with a number\n").unwrap();
        assert!(rows.is_empty());
        // And a bare digit with no id is not a row either.
        assert!(parse_sheet("3\n").unwrap().is_empty());
    }

    #[test]
    fn a_typo_in_the_grade_column_is_refused_rather_than_skipped() {
        // Silently ignoring '7' would drop a judgement that was actually made, and the only
        // symptom would be coverage quietly failing to rise.
        let err = parse_sheet("7  codex:a\n").unwrap_err().to_string();
        assert!(err.contains("line 1") && err.contains("not a grade"), "got: {err}");

        // A conversation listed twice cannot be resolved by picking one, so it is refused too.
        let dup = parse_sheet("3  codex:a\n1  codex:a\n").unwrap_err().to_string();
        assert!(dup.contains("appears twice"), "got: {dup}");
    }

    #[test]
    fn a_rendered_sheet_reads_back_as_what_went_into_it() {
        let eq = EvalQuery {
            id: "camping".into(),
            q: "camping trip".into(),
            note: Some("the whole non-technical category in miniature".into()),
        };
        let groups = vec![group("chatgpt-export:aaa", "Camping Trip Options"),
                          group("codex:bbb", "Something else")];
        let have: HashMap<String, Grade> =
            [("chatgpt-export:aaa".to_string(), Grade::Exact)].into_iter().collect();

        let text = render_sheet(&eq, &groups, &have);
        assert!(text.contains("the whole non-technical category in miniature"), "note carries over");
        assert_eq!(parse_sheet(&text).unwrap(), vec![
            // Already graded, so the sheet arrives pre-filled rather than asking again.
            row("chatgpt-export:aaa", Some(Grade::Exact)),
            row("codex:bbb", None),
        ]);
    }

    #[test]
    fn sheets_sit_beside_the_set_they_belong_to() {
        assert_eq!(sheets_dir(Path::new("/e/ranking.toml")), PathBuf::from("/e/sheets"));
        assert_eq!(
            sheet_path(Path::new("/e/ranking.toml"), "camping"),
            PathBuf::from("/e/sheets/camping.md")
        );
    }

    #[test]
    fn the_pool_varies_exactly_the_two_constants_the_tuning_issue_exists_to_move() {
        let vs = pool_variants();
        assert_eq!(vs.len(), 3);
        assert!(vs.iter().any(|v| v.decay == 0.0), "a recency-blind ranker must be pooled");
        assert!(vs.iter().any(|v| v.repeat_weight == 0.0), "a pure-max ranker must be pooled");
        assert!(vs.iter().any(|v| v.name == "default"
            && v.decay == cs_core::DECAY
            && v.repeat_weight == cs_core::REPEAT_WEIGHT));
    }
}
