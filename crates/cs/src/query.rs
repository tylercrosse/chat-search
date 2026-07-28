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
    json: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    let reader = ArchiveReader::new(m.dir(&cfg.archive_root));
    let db_path = db_path.unwrap_or_else(|| default_db(&cfg));

    let started = std::time::Instant::now();
    let mut conn = cs_core::open(db_path.to_str().context("db path is not utf-8")?)
        .context("opening index")?;
    cs_core::reset(&conn).context("clearing index")?;

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
        let stats = cs_core::write_conversations(&mut conn, convs.iter())
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
        let stats = cs_core::write_conversations(&mut conn, convs.iter())?;
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
    json: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| default_db(&cfg));
    let t0 = std::time::Instant::now();
    let conn = rusqlite_open(&db_path)?;
    let hits = cs_core::search(
        &conn,
        &cs_core::Query {
            text,
            limit,
            field: if tools { cs_core::Field::Tools } else { cs_core::Field::Prose },
            source,
            include_off_path,
            now_ms: now_ms(),
        },
    )?;
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
