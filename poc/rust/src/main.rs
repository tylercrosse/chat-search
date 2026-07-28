mod db;
mod importers;
mod search;
mod types;
mod walk;

use importers::{discover, import_claude_code, import_codex};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == &format!("--{name}"))?;
    args.get(i + 1).filter(|v| !v.starts_with("--")).cloned()
}
fn has(name: &str) -> bool {
    std::env::args().any(|a| a == format!("--{name}"))
}
fn default_db() -> String {
    format!("{}/.chat-search/poc-rs.db", std::env::var("HOME").unwrap_or_default())
}
fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

fn cmd_index() -> Result<(), Box<dyn std::error::Error>> {
    let db_path = arg("db").unwrap_or_else(default_db);
    let limit: usize = arg("limit").and_then(|v| v.parse().ok()).unwrap_or(0);
    let t0 = Instant::now();

    let files = discover();
    let picked = if limit > 0 { &files[..limit.min(files.len())] } else { &files[..] };

    let mut conn = db::open(&db_path, true)?;
    conn.execute_batch(
        "DELETE FROM conversation; DELETE FROM message;
         DELETE FROM fts_prose; DELETE FROM fts_tools;",
    )?;

    let (mut convs, mut msgs, mut prose, mut failed, mut dupes) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut bytes = 0u64;
    let mut by_source: BTreeMap<&str, u64> = BTreeMap::new();

    let tx = conn.transaction()?;
    {
        // Field-level merge, not first-write-wins: a conversation can be spread over
        // several files (a session plus its subagent sidechains, or a resumed rollout)
        // and only some of them carry the title, cwd or model.
        let mut ins_conv = tx.prepare(
            "INSERT INTO conversation
             (id, source, native_id, title, cwd, git_branch, model, started_at, ended_at,
              msg_count, prose_count, forked_from, resume_cmd)
             VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,NULL,0,0,?8,?9)
             ON CONFLICT(id) DO UPDATE SET
               title       = COALESCE(conversation.title, excluded.title),
               cwd         = COALESCE(conversation.cwd, excluded.cwd),
               git_branch  = COALESCE(conversation.git_branch, excluded.git_branch),
               model       = COALESCE(conversation.model, excluded.model),
               forked_from = COALESCE(conversation.forked_from, excluded.forked_from)",
        )?;
        // OR IGNORE, not OR REPLACE: several files can carry the same sessionId (a session
        // and its subagent sidechains). Replacing would orphan the FTS rows keyed on the
        // old rowid. Ignoring keeps message and index in lockstep.
        let mut ins_msg = tx.prepare(
            "INSERT OR IGNORE INTO message
             (id, conv_id, parent_id, seq, role, kind, ts, is_sidechain, text)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        )?;
        let mut ins_prose = tx.prepare("INSERT INTO fts_prose(rowid, text) VALUES (?1,?2)")?;
        let mut ins_tools = tx.prepare("INSERT INTO fts_tools(rowid, text) VALUES (?1,?2)")?;

        for f in picked {
            let c = match f.source {
                "codex" => import_codex(&f.path),
                _ => import_claude_code(&f.path),
            };
            let Some(c) = c else {
                failed += 1;
                continue;
            };

            let conv_id = format!("{}:{}", c.source, c.native_id);
            ins_conv.execute(rusqlite::params![
                conv_id, c.source, c.native_id, c.title, c.cwd, c.git_branch, c.model,
                c.forked_from, c.resume_cmd
            ])?;

            for m in &c.messages {
                let changed = ins_msg.execute(rusqlite::params![
                    format!("{conv_id}:{}", m.native_id),
                    conv_id,
                    m.parent_id.as_ref().map(|p| format!("{conv_id}:{p}")),
                    m.seq,
                    m.role,
                    m.kind.as_str(),
                    m.ts,
                    if m.is_sidechain { 1 } else { 0 },
                    m.text,
                ])?;
                if changed == 0 {
                    dupes += 1;
                    continue; // already indexed
                }
                let rowid = tx.last_insert_rowid();
                if m.kind == types::Kind::Prose {
                    ins_prose.execute(rusqlite::params![rowid, m.text])?;
                    prose += 1;
                } else if m.kind.is_tool() {
                    ins_tools.execute(rusqlite::params![rowid, m.text])?;
                }
                bytes += m.text.len() as u64;
                msgs += 1;
            }
            convs += 1;
            *by_source.entry(c.source).or_insert(0) += 1;
        }
    }

    // counts and time bounds rolled up by aggregate, so multiple files feeding one
    // conversation are combined correctly
    tx.execute_batch(
        "UPDATE conversation SET
           msg_count   = (SELECT COUNT(*) FROM message WHERE conv_id = conversation.id),
           prose_count = (SELECT COUNT(*) FROM message WHERE conv_id = conversation.id AND kind='prose'),
           started_at  = (SELECT MIN(ts)  FROM message WHERE conv_id = conversation.id),
           ended_at    = (SELECT MAX(ts)  FROM message WHERE conv_id = conversation.id)",
    )?;
    tx.commit()?;
    conn.pragma_update(None, "wal_checkpoint", "TRUNCATE").ok();

    let by: Map<String, Value> =
        by_source.into_iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    println!(
        "{:#}",
        json!({
            "impl": "rust", "db": db_path,
            "files_seen": files.len(), "files_indexed": picked.len(),
            "failed": failed, "conversations": convs, "messages": msgs, "prose": prose,
            "dupes": dupes, "text_bytes": bytes, "by_source": by,
            "ms": t0.elapsed().as_millis() as u64,
        })
    );
    Ok(())
}

fn cmd_search() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let q = match args.get(2) {
        Some(q) if !q.starts_with("--") => q.clone(),
        _ => {
            eprintln!("usage: search <query> [--db PATH] [--limit N] [--tools]");
            std::process::exit(2);
        }
    };
    let db_path = arg("db").unwrap_or_else(default_db);
    let limit: i64 = arg("limit").and_then(|v| v.parse().ok()).unwrap_or(10);
    let t0 = Instant::now();
    let conn = db::open(&db_path, false)?;
    let results = search::search(&conn, &q, limit, has("tools"), now_ms())?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!(
        "{:#}",
        json!({ "impl": "rust", "query": q, "ms": (ms * 100.0).round() / 100.0, "results": results })
    );
    Ok(())
}

fn cmd_stats() -> Result<(), Box<dyn std::error::Error>> {
    let conn = db::open(&arg("db").unwrap_or_else(default_db), false)?;
    let count = |sql: &str| -> rusqlite::Result<i64> { conn.query_row(sql, [], |r| r.get(0)) };
    let group = |sql: &str| -> rusqlite::Result<Map<String, Value>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, json!(r.get::<_, i64>(1)?)))
        })?;
        rows.collect()
    };
    println!(
        "{:#}",
        json!({
            "impl": "rust",
            "conversations": count("SELECT COUNT(*) FROM conversation")?,
            "messages": count("SELECT COUNT(*) FROM message")?,
            "by_source": group("SELECT source, COUNT(*) FROM conversation GROUP BY source")?,
            "by_kind": group("SELECT kind, COUNT(*) FROM message GROUP BY kind")?,
        })
    );
    Ok(())
}

fn main() {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    let r = match cmd.as_str() {
        "index" => cmd_index(),
        "search" => cmd_search(),
        "stats" => cmd_stats(),
        _ => {
            eprintln!("usage: <index|search|stats> [...]");
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
