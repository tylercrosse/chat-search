//! `cs index`, `cs search` and `cs explain` — everything downstream of the archive.

use anyhow::{Context, Result};
use cs_archive::{machine, ArchiveReader, Config};
use cs_core::model::Conversation;
use std::path::{Path, PathBuf};

/// Default index location. Sits beside the archive but is disposable — deleting it costs a
/// rebuild, never data (ADR 1).
pub fn default_db(cfg: &Config) -> PathBuf {
    cfg.archive_root.join("index.db")
}

/// Which importer handles a source. Keyed on the source id, which is permanent (ADR 16).
fn import_source(source_id: &str, logical_path: &str, bytes: &[u8]) -> Vec<Conversation> {
    match source_id {
        "codex" => cs_import::codex::import(logical_path, bytes).into_iter().collect(),
        "claude-code" => cs_import::claude_code::import(logical_path, bytes).into_iter().collect(),
        "chatgpt-export" => cs_import::chatgpt_export::import_all(bytes),
        _ => Vec::new(),
    }
}

pub fn index(
    config_path: &Path,
    db_path: Option<PathBuf>,
    chatgpt_export: Option<PathBuf>,
    tool_text_limit: usize,
    json: bool,
) -> Result<()> {
    let opts = cs_core::IndexOptions { tool_text_limit };
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    let reader = ArchiveReader::new(m.dir(&cfg.archive_root));
    let db_path = db_path.unwrap_or_else(|| default_db(&cfg));

    let started = std::time::Instant::now();
    let mut conn = cs_core::open_fresh(db_path.to_str().context("db path is not utf-8")?)
        .context("opening index")?;

    let mut reports = Vec::new();
    let mut totals = cs_core::IndexStats::default();

    for source in &cfg.sources {
        let files = reader.files(&source.id)?;
        if files.is_empty() {
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

    // The ChatGPT export is not archived yet, so it is indexed straight from the export
    // directory. Temporary: once export ingest lands it becomes an ordinary source.
    if let Some(dir) = chatgpt_export {
        let t0 = std::time::Instant::now();
        let mut convs = Vec::new();
        let mut files = 0usize;
        for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !name.starts_with("conversations") || path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            files += 1;
            convs.extend(cs_import::chatgpt_export::import_all(&std::fs::read(&path)?));
        }
        let stats = cs_core::write_conversations_with(&mut conn, convs.iter(), opts)?;
        reports.push(source_report("chatgpt-export", files, 0, &stats, t0));
        accumulate(&mut totals, &stats);
    }

    let ms = started.elapsed().as_millis() as u64;
    if json {
        println!("{:#}", serde_json::json!({
            "db": db_path, "sources": reports,
            "conversations": totals.conversations, "messages": totals.messages,
            "prose": totals.prose, "duplicates": totals.duplicates, "ms": ms,
        }));
    } else {
        println!("  {:<16} {:>6} {:>8} {:>10} {:>8} {:>7}",
                 "source", "files", "convs", "messages", "prose", "ms");
        for r in &reports {
            let g = |k: &str| r[k].as_u64().unwrap_or(0);
            println!("  {:<16} {:>6} {:>8} {:>10} {:>8} {:>7}",
                     r["source"].as_str().unwrap_or("?"),
                     g("files"), g("conversations"), g("messages"), g("prose"), g("ms"));
        }
        println!("\n  {} conversations · {} messages · {} prose · {} ms",
                 totals.conversations, totals.messages, totals.prose, ms);
        println!("  index: {}", db_path.display());
        if ms > 60_000 {
            println!("\n  warning: rebuild exceeded 60s — see ADR 1 'revisit when'");
        }
    }
    Ok(())
}

fn source_report(
    id: &str, files: usize, failed: u64, s: &cs_core::IndexStats, t0: std::time::Instant,
) -> serde_json::Value {
    serde_json::json!({
        "source": id, "files": files, "unreadable": failed,
        "conversations": s.conversations, "messages": s.messages, "prose": s.prose,
        "duplicates": s.duplicates, "text_bytes": s.text_bytes,
        "ms": t0.elapsed().as_millis() as u64,
    })
}

fn accumulate(total: &mut cs_core::IndexStats, s: &cs_core::IndexStats) {
    total.conversations += s.conversations;
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
    let db_path = db_path.unwrap_or_else(|| default_db(&cfg));
    let t0 = std::time::Instant::now();
    let conn = rusqlite_open(&db_path)?;
    let q = cs_core::Query {
        text,
        limit,
        field: if tools { cs_core::Field::Tools } else { cs_core::Field::Prose },
        source,
        include_off_path,
        prefix,
        now_ms: now_ms(),
    };

    if !flat {
        let groups = cs_core::search_grouped(&conn, &q, nested)?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
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

pub fn explain(config_path: &Path, db_path: Option<PathBuf>, conv_id: &str, query: &str) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| default_db(&cfg));
    let conn = rusqlite_open(&db_path)?;
    let e = cs_core::explain(&conn, conv_id, query)?;
    println!("{:#}", serde_json::to_value(&e)?);
    Ok(())
}

fn rusqlite_open(path: &Path) -> Result<rusqlite::Connection> {
    rusqlite::Connection::open(path)
        .with_context(|| format!("opening {} (run `cs index` first?)", path.display()))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
        if let Some(ts) = g.ended_at {
            meta.push(ymd(ts));
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

/// Date only, from epoch millis — enough for a result line, and avoids a date dependency.
fn ymd(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}
