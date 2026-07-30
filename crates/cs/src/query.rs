//! `cs index`, `cs search` and `cs explain` — everything downstream of the archive.

use anyhow::{Context, Result};
use cs_archive::{machine, ArchiveReader, Config, Layout};
use cs_core::model::Conversation;
use std::path::{Path, PathBuf};

/// Which importer handles a source. Keyed on the source id, which is permanent (ADR 16).
fn import_source(source_id: &str, logical_path: &str, bytes: &[u8]) -> Vec<Conversation> {
    match source_id {
        "codex" => cs_import::codex::import(logical_path, bytes).into_iter().collect(),
        "claude-code" => cs_import::claude_code::import(logical_path, bytes).into_iter().collect(),
        "chatgpt-export" => cs_import::chatgpt_export::import_all(bytes),
        "gemini-cli" => cs_import::gemini::import(logical_path, bytes).into_iter().collect(),
        _ => Vec::new(),
    }
}

pub fn index(
    config_path: &Path,
    db_path: Option<PathBuf>,
    tool_text_limit: usize,
    json: bool,
) -> Result<()> {
    let opts = cs_core::IndexOptions { tool_text_limit };
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    let reader = ArchiveReader::new(m.dir(&cfg.archive_root));
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());

    let started = std::time::Instant::now();
    let mut conn = cs_core::open_fresh(db_path.to_str().context("db path is not utf-8")?)
        .context("opening index")?;

    let mut reports = Vec::new();
    let mut totals = cs_core::IndexStats::default();

    for source in &cfg.sources {
        let files = reader.files(&source.id)?;
        // A configured source that contributes nothing still gets a row. Skipping it
        // silently is how 2,011 ChatGPT conversations went missing from a run that
        // reported success (chat-search-6eb.7): the absence of a row reads as "no such
        // source", which is indistinguishable from "nothing archived yet".
        if files.is_empty() {
            reports.push(empty_source_report(&source.id, source.layout));
            continue;
        }
        let t0 = std::time::Instant::now();
        // Accumulated per source rather than across all of them: one source at a time keeps
        // peak memory to the largest source rather than the whole corpus.
        let mut convs = Vec::new();
        let mut failed = 0u64;
        for f in &files {
            match reader.read(f) {
                Ok(bytes) => convs.extend(import_source(&source.id, &f.logical_path, &bytes)),
                Err(_) => failed += 1,
            }
        }
        let stats = cs_core::write_conversations_with(&mut conn, convs.iter(), opts)
            .with_context(|| format!("indexing {}", source.id))?;
        reports.push(source_report(&source.id, files.len(), failed, &stats, t0));
        accumulate(&mut totals, &stats);
    }

    let ms = started.elapsed().as_millis() as u64;
    if json {
        println!("{:#}", serde_json::json!({
            "db": db_path, "sources": reports,
            "conversations": totals.conversations, "messages": totals.messages,
            "prose": totals.prose, "merged": totals.merged,
            "duplicates": totals.duplicates, "ms": ms,
        }));
    } else {
        println!("  {:<16} {:>6} {:>8} {:>10} {:>8} {:>7}",
                 "source", "files", "convs", "messages", "prose", "ms");
        for r in &reports {
            let g = |k: &str| r[k].as_u64().unwrap_or(0);
            let id = r["source"].as_str().unwrap_or("?");
            match r["skipped"].as_str() {
                Some(why) => println!("  {id:<16} {:>6} {why}", "—"),
                None => println!("  {:<16} {:>6} {:>8} {:>10} {:>8} {:>7}",
                                 id, g("files"), g("conversations"), g("messages"),
                                 g("prose"), g("ms")),
            }
        }
        println!("\n  {} conversations · {} messages · {} prose · {} ms",
                 totals.conversations, totals.messages, totals.prose, ms);
        // Ordinary and expected — one conversation often spans several transcript files, and
        // a re-downloaded export re-delivers the lot. Worth stating so the number above is
        // not read as a loss when it is larger than the file count would suggest.
        if totals.merged > 0 {
            println!("  {} conversation(s) arrived more than once and were folded together",
                     totals.merged);
        }
        println!("  index: {}", db_path.display());
        if ms > 60_000 {
            println!("\n  warning: rebuild exceeded 60s — see ADR 1 'revisit when'");
        }
    }
    Ok(())
}

/// A row for a source the archive holds nothing for, carrying why.
///
/// The two reasons are not the same problem and want different fixes: a bundle source has
/// no capture path yet (ADR 18), while a mirror source with no archived files means
/// `cs archive` has not run, or its globs match nothing.
fn empty_source_report(id: &str, layout: Layout) -> serde_json::Value {
    let skipped = match layout {
        Layout::Bundle => "bundle layout not implemented — never captured",
        Layout::Mirror => "nothing archived — run `cs archive`, or check the source's globs",
    };
    serde_json::json!({
        "source": id, "files": 0, "unreadable": 0,
        "conversations": 0, "messages": 0, "prose": 0,
        "duplicates": 0, "text_bytes": 0, "ms": 0,
        "skipped": skipped,
    })
}

fn source_report(
    id: &str, files: usize, failed: u64, s: &cs_core::IndexStats, t0: std::time::Instant,
) -> serde_json::Value {
    serde_json::json!({
        "source": id, "files": files, "unreadable": failed,
        "conversations": s.conversations, "messages": s.messages, "prose": s.prose,
        "merged": s.merged, "duplicates": s.duplicates, "text_bytes": s.text_bytes,
        "ms": t0.elapsed().as_millis() as u64,
    })
}

fn accumulate(total: &mut cs_core::IndexStats, s: &cs_core::IndexStats) {
    total.conversations += s.conversations;
    total.merged += s.merged;
    total.messages += s.messages;
    total.prose += s.prose;
    total.duplicates += s.duplicates;
    total.text_bytes += s.text_bytes;
}

#[allow(clippy::too_many_arguments)]
pub fn search(
    config_path: &Path,
    db_path: Option<PathBuf>,
    text: &str,
    limit: i64,
    source: Option<&str>,
    tools: bool,
    include_off_path: bool,
    prefix: bool,
    nested: usize,
    flat: bool,
    json: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let t0 = std::time::Instant::now();
    let conn = rusqlite_open(&db_path)?;
    let q = cs_core::Query {
        text,
        limit,
        field: if tools { cs_core::Field::Tools } else { cs_core::Field::Prose },
        source,
        include_off_path,
        prefix,
        now_ms: cs_core::now_ms(),
        ..cs_core::Query::new(text)
    };

    if !flat {
        let groups = cs_core::search_grouped(&conn, &q, nested)?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Typeahead is excluded: `--prefix` fires once per keystroke, so logging it would
        // bury the handful of real queries under every prefix of each of them. A client
        // doing typeahead records the finished query with `cs pick` instead.
        if !prefix {
            log_search(&cfg, text, source, &groups, ms);
        }
        return render_groups(&groups, text, ms, json);
    }

    let hits = cs_core::search(&conn, &q)?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    if json {
        // Field names here are a contract a GUI consumes verbatim — additive changes only.
        println!("{:#}", serde_json::json!({
            "query": text, "ms": (ms * 100.0).round() / 100.0, "count": hits.len(),
            "results": hits,
        }));
    } else if hits.is_empty() {
        println!("no results for {text:?}");
        println!("  if you expected one, `cs explain <conv-id> {text:?}` says why it missed");
    } else {
        for (i, h) in hits.iter().enumerate() {
            let mut flags = Vec::new();
            if !h.on_head_path { flags.push("edited-away") }
            if h.is_sidechain { flags.push("subagent") }
            if h.deleted_upstream { flags.push("deleted-upstream") }
            let flag = if flags.is_empty() { String::new() } else { format!("  [{}]", flags.join(" ")) };
            println!("{:>2}. {} · {}{}", i + 1, h.source, h.title.as_deref().unwrap_or("(untitled)"), flag);
            println!("    {}", h.snippet);
            if let Some(cmd) = &h.resume_cmd {
                println!("    {cmd}");
            }
            println!();
        }
        println!("{} results · {:.1} ms", hits.len(), ms);
    }
    Ok(())
}

/// Record a search. Never fails the search it is describing — see `querylog::append`.
fn log_search(cfg: &Config, text: &str, source: Option<&str>, groups: &[cs_core::Group], ms: f64) {
    if !cfg.log_queries || cs_core::is_blank(text) {
        return;
    }
    let shown = cs_core::querylog::truncate_shown(
        groups.iter().map(|g| g.conv_id.clone()).collect(),
    );
    let _ = cs_core::querylog::append(&cfg.query_log(), &cs_core::querylog::Event::Search {
        ts: cs_core::now_ms(),
        q: text.to_string(),
        source: source.map(String::from),
        shown,
        n: groups.len(),
        ms: (ms * 100.0).round() / 100.0,
    });
}

/// Record that a search ended in opening a conversation, and print its resume command.
///
/// The half of the log that carries ground truth. A search on its own says what was wanted;
/// this says what answered it, which is a relevance judgement nobody had to sit down and make.
///
/// The result list is recomputed here rather than passed in, because the caller is a shell
/// script holding a chosen line and not much else. It costs a few milliseconds and it keeps
/// the recorded rank honest about what this ranking would return, which is what a later
/// tuning run needs to compare against.
pub fn pick(
    config_path: &Path,
    db_path: Option<PathBuf>,
    conv_id: &str,
    text: &str,
    source: Option<&str>,
    limit: i64,
    quiet: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let conn = rusqlite_open(&db_path)?;

    let (mut shown, n) = if cs_core::is_blank(text) {
        // Picked off the no-query recent list. Worth recording — it says the conversation
        // was wanted without anything being typed — but there is no ranking to place it in.
        (Vec::new(), 0)
    } else {
        let q = cs_core::Query {
            text,
            limit,
            source,
            now_ms: cs_core::now_ms(),
            ..cs_core::Query::new(text)
        };
        let groups = cs_core::search_grouped(&conn, &q, 0)?;
        let ids: Vec<String> = groups.iter().map(|g| g.conv_id.clone()).collect();
        let n = ids.len();
        (ids, n)
    };
    let rank = shown.iter().position(|c| c == conv_id).map(|i| i + 1);
    shown = cs_core::querylog::truncate_shown(shown);

    if cfg.log_queries {
        let _ = cs_core::querylog::append(&cfg.query_log(), &cs_core::querylog::Event::Pick {
            ts: cs_core::now_ms(),
            q: text.to_string(),
            source: source.map(String::from),
            conv_id: conv_id.to_string(),
            rank,
            shown,
            n,
        });
    }

    // Printing the resume command is what makes this usable as a drop-in for reading
    // `resume_cmd` out of the JSON: a client pipes through `cs pick` and gets the same
    // string it already had, with the selection recorded on the way past.
    if !quiet {
        let cmd: Option<String> = conn
            .query_row(
                "SELECT resume_cmd FROM conversation WHERE id = ?1",
                rusqlite::params![conv_id],
                |r| r.get(0),
            )
            .unwrap_or(None);
        if let Some(cmd) = cmd {
            println!("{cmd}");
        }
    }
    Ok(())
}

/// What has been searched for, and what answered it.
pub fn needs(config_path: &Path, limit: usize, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let path = cfg.query_log();
    let (events, skipped) = cs_core::querylog::load(&path)?;
    let needs = cs_core::querylog::needs(&events);

    if json {
        println!("{:#}", serde_json::json!({
            "log": path, "events": events.len(), "unreadable": skipped,
            "needs": needs.iter().take(limit).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if events.is_empty() {
        println!("nothing logged yet — {}", path.display());
        println!("  `cs search` records what it was asked for; `cs pick` records what you opened");
        return Ok(());
    }

    println!("  {:<34} {:>8} {:>7}  {}", "query", "searches", "picks", "opened");
    for n in needs.iter().take(limit) {
        let picks: usize = n.picked.iter().map(|(_, c)| c).sum();
        let top = n.picked.first().map(|(c, _)| c.as_str()).unwrap_or("—");
        println!("  {:<34} {:>8} {:>7}  {}", trunc(&n.q, 34), n.searches, picks, trunc(top, 40));
    }
    let picked: usize = needs.iter().map(|n| n.picked.len()).sum();
    println!("\n  {} distinct quer(y/ies) · {} event(s) · {picked} answered", needs.len(), events.len());
    if skipped > 0 {
        println!("  warning: {skipped} unreadable line(s) in {}", path.display());
    }
    Ok(())
}

fn trunc(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    format!("{}…", chars[..n.saturating_sub(1)].iter().collect::<String>())
}

pub fn explain(config_path: &Path, db_path: Option<PathBuf>, conv_id: &str, query: &str) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let conn = rusqlite_open(&db_path)?;
    let e = cs_core::explain(&conn, conv_id, query)?;
    println!("{:#}", serde_json::to_value(&e)?);
    Ok(())
}

fn rusqlite_open(path: &Path) -> Result<rusqlite::Connection> {
    rusqlite::Connection::open(path)
        .with_context(|| format!("opening {} (run `cs index` first?)", path.display()))
}

/// Conversation-grouped output: the conversation is the result, its best matching messages
/// nest beneath it. Mirrors the shape docs search engines settled on, and it also dissolves
/// the duplicate-hit problem — a conversation's main thread and its subagents collapse into
/// one entry instead of competing for slots.
fn render_groups(groups: &[cs_core::Group], query: &str, ms: f64, json: bool) -> Result<()> {
    if json {
        println!("{:#}", serde_json::json!({
            "query": query, "ms": (ms * 100.0).round() / 100.0,
            "count": groups.len(), "grouped": true, "results": groups,
        }));
        return Ok(());
    }
    if groups.is_empty() {
        println!("no results for {query:?}");
        println!("  if you expected one, `cs explain <conv-id> {query:?}` says why it missed");
        return Ok(());
    }
    for (i, g) in groups.iter().enumerate() {
        let mut meta = vec![g.source.clone()];
        // Rendered by cs-core, not here: this is the same string the JSON hands a GUI, so
        // the terminal and every other client cannot disagree about which day a session was.
        if let Some(date) = &g.ended_date {
            meta.push(date.clone());
        }
        meta.push(format!("{} turns", g.user_turns));
        if g.deleted_upstream {
            meta.push("deleted upstream".into());
        }
        println!(
            "{:>2}. {}",
            i + 1,
            g.title.as_deref().unwrap_or("(untitled)")
        );
        println!("    {}", meta.join(" · "));
        for h in &g.hits {
            let tag = if h.is_sidechain {
                "  [subagent]"
            } else if !h.on_head_path {
                "  [edited away]"
            } else {
                ""
            };
            println!("      {}{}", h.snippet, tag);
        }
        if g.match_count > g.hits.len() {
            println!("      +{} more match(es)", g.match_count - g.hits.len());
        }
        if let Some(cmd) = &g.resume_cmd {
            println!("    {cmd}");
        }
        println!();
    }
    println!("{} conversations · {:.1} ms", groups.len(), ms);
    Ok(())
}
