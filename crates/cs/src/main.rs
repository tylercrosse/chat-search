use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cs_archive::{machine, CaptureKind, Change, Config, Event, Fingerprint, Layout, Manifest, ManifestWriter, Op};
use std::path::PathBuf;

mod query;

#[derive(Parser)]
#[command(name = "cs", about = "Search your AI conversations across every tool", version)]
struct Cli {
    /// Config file (default: ~/.config/chat-search/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a starter config with the sources found on this machine.
    Init {
        /// Override the auto-derived machine slug.
        #[arg(long)]
        machine_alias: Option<String>,
        /// Overwrite an existing config.
        #[arg(long)]
        force: bool,
    },
    /// Show the resolved config and machine identity.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Compare sources against the manifest and report what changed. Reads only.
    Scan {
        /// Limit to one source id.
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Rebuild the search index from the archive. Always a full rebuild — the index is a
    /// pure function of the archive, so there is no migration path (ADR 1).
    Index {
        #[arg(long)]
        db: Option<PathBuf>,
        /// Index a ChatGPT export directly, until export ingest lands.
        #[arg(long)]
        chatgpt_export: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Search indexed conversations.
    Search {
        query: String,
        #[arg(long, default_value = "10")]
        limit: i64,
        #[arg(long)]
        db: Option<PathBuf>,
        /// Restrict to one source id.
        #[arg(long)]
        source: Option<String>,
        /// Search tool calls and output instead of prose.
        #[arg(long)]
        tools: bool,
        /// Include messages on branches that were edited away.
        #[arg(long)]
        include_off_path: bool,
        #[arg(long)]
        json: bool,
    },
    /// Why a conversation did not come back for a query.
    Explain {
        conv_id: String,
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Capture changed files into the archive and record what was observed.
    Archive {
        /// Limit to one source id.
        #[arg(long)]
        source: Option<String>,
        /// Report what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(Config::default_path);

    match cli.command {
        Command::Init { machine_alias, force } => init(&config_path, machine_alias, force),
        Command::Status { json } => status(&config_path, json),
        Command::Scan { source, json } => scan(&config_path, source.as_deref(), json),
        Command::Index { db, chatgpt_export, json } => {
            query::index(&config_path, db, chatgpt_export, json)
        }
        Command::Search { query: q, limit, db, source, tools, include_off_path, json } => {
            query::search(&config_path, db, &q, limit, source.as_deref(), tools, include_off_path, json)
        }
        Command::Explain { conv_id, query: q, db } => query::explain(&config_path, db, &conv_id, &q),
        Command::Archive { source, dry_run, json } => {
            archive(&config_path, source.as_deref(), dry_run, json)
        }
    }
}

fn archive(config_path: &PathBuf, only: Option<&str>, dry_run: bool, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    let machine_dir = m.dir(&cfg.archive_root);
    let manifest = Manifest::load(&machine_dir).context("loading manifest")?;
    let writer = (!dry_run)
        .then(|| ManifestWriter::new(&machine_dir))
        .transpose()
        .context("opening manifest for append")?;

    let started = std::time::Instant::now();
    let mut reports = Vec::new();
    let (mut tot_cloned, mut tot_copied, mut tot_bytes) = (0u64, 0u64, 0u64);

    for source in &cfg.sources {
        if only.is_some_and(|o| o != source.id) || source.layout == Layout::Bundle {
            continue;
        }
        let scan = cs_archive::scan_source(source, &manifest)
            .with_context(|| format!("scanning {}", source.id))?;

        let t0 = std::time::Instant::now();
        let (mut cloned, mut copied, mut bytes) = (0u64, 0u64, 0u64);
        let mut events = Vec::new();

        for c in &scan.changes {
            let op = match c.change {
                Change::New => Op::Seen,
                Change::Appended => Op::Appended,
                Change::Rewritten => Op::Rewritten,
                Change::Vanished => Op::Vanished,
                Change::Unchanged => continue, // nothing observed worth recording
            };

            if c.change.needs_capture() && !dry_run {
                let kind = cs_archive::capture_file(
                    &machine_dir, &source.id, &c.rel_path, &c.abs_path, c.change,
                    cs_archive::manifest::now_ms(),
                )
                .with_context(|| format!("capturing {}", c.abs_path.display()))?;
                match kind {
                    CaptureKind::Cloned => cloned += 1,
                    CaptureKind::Copied => copied += 1,
                }
            }
            if let Some(fp) = &c.fingerprint {
                bytes += if c.change.needs_capture() { fp.size } else { 0 };
            }

            events.push(Event {
                ts: cs_archive::manifest::now_ms(),
                op,
                source: source.id.clone(),
                path: c.rel_path.clone(),
                // A vanished file has nothing left to fingerprint; carry the last known one
                // so the event still records what was lost.
                fingerprint: c.fingerprint.clone().unwrap_or_else(|| {
                    manifest
                        .get(&source.id, &c.rel_path)
                        .map(|e| e.fingerprint.clone())
                        .unwrap_or(Fingerprint {
                            size: 0, mtime_ms: 0, prefix_hash: String::new(), prefix_len: 0,
                        })
                }),
            });
        }

        if let Some(w) = &writer {
            w.append(&events).context("appending manifest events")?;
        }

        tot_cloned += cloned;
        tot_copied += copied;
        tot_bytes += bytes;
        reports.push(serde_json::json!({
            "source": scan.source,
            "new": scan.count(Change::New),
            "appended": scan.count(Change::Appended),
            "rewritten": scan.count(Change::Rewritten),
            "vanished": scan.count(Change::Vanished),
            "unchanged": scan.count(Change::Unchanged),
            "cloned": cloned, "copied": copied, "bytes": bytes,
            "ms": t0.elapsed().as_millis() as u64,
        }));
    }

    if json {
        println!("{:#}", serde_json::json!({
            "dry_run": dry_run, "archive": machine_dir, "sources": reports,
            "cloned": tot_cloned, "copied": tot_copied, "bytes": tot_bytes,
            "ms": started.elapsed().as_millis() as u64,
        }));
    } else {
        if dry_run {
            println!("dry run — nothing written\n");
        }
        println!("  {:<13} {:>5} {:>9} {:>10} {:>9} {:>10} {:>8} {:>7} {:>7}",
                 "source", "new", "appended", "rewritten", "vanished", "unchanged",
                 "cloned", "copied", "ms");
        for r in &reports {
            let g = |k: &str| r[k].as_u64().unwrap_or(0);
            println!("  {:<13} {:>5} {:>9} {:>10} {:>9} {:>10} {:>8} {:>7} {:>7}",
                     r["source"].as_str().unwrap_or("?"),
                     g("new"), g("appended"), g("rewritten"), g("vanished"),
                     g("unchanged"), g("cloned"), g("copied"), g("ms"));
        }
        println!("\n  {:.1} MB captured · {} cloned · {} copied · {} ms",
                 tot_bytes as f64 / 1_048_576.0, tot_cloned, tot_copied,
                 started.elapsed().as_millis());
        println!("  archive: {}", machine_dir.display());
    }
    Ok(())
}

fn scan(config_path: &PathBuf, only: Option<&str>, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    let manifest = Manifest::load(&m.dir(&cfg.archive_root)).context("loading manifest")?;

    let started = std::time::Instant::now();
    let mut reports = Vec::new();

    for source in &cfg.sources {
        if only.is_some_and(|o| o != source.id) {
            continue;
        }
        if source.layout == Layout::Bundle {
            reports.push(serde_json::json!({
                "source": source.id, "skipped": "bundle layout not implemented yet",
            }));
            continue;
        }
        let t0 = std::time::Instant::now();
        let scan = cs_archive::scan_source(source, &manifest)
            .with_context(|| format!("scanning {}", source.id))?;
        reports.push(serde_json::json!({
            "source": scan.source,
            "files": scan.changes.len(),
            "new": scan.count(Change::New),
            "appended": scan.count(Change::Appended),
            "rewritten": scan.count(Change::Rewritten),
            "vanished": scan.count(Change::Vanished),
            "unchanged": scan.count(Change::Unchanged),
            "bytes_to_capture": scan.changes.iter()
                .filter(|c| c.change.needs_capture())
                .filter_map(|c| c.fingerprint.as_ref().map(|f| f.size))
                .sum::<u64>(),
            "ms": t0.elapsed().as_millis() as u64,
        }));
    }

    if json {
        println!("{:#}", serde_json::json!({
            "manifest_entries": manifest.len(),
            "sources": reports,
            "ms": started.elapsed().as_millis() as u64,
        }));
    } else {
        println!("manifest holds {} known files\n", manifest.len());
        println!("  {:<13} {:>6} {:>6} {:>9} {:>10} {:>9} {:>10} {:>7}",
                 "source", "files", "new", "appended", "rewritten", "vanished", "unchanged", "ms");
        for r in &reports {
            if let Some(skip) = r.get("skipped") {
                println!("  {:<13} — {}", r["source"].as_str().unwrap_or("?"), skip.as_str().unwrap_or(""));
                continue;
            }
            let g = |k: &str| r[k].as_u64().unwrap_or(0);
            println!("  {:<13} {:>6} {:>6} {:>9} {:>10} {:>9} {:>10} {:>7}",
                     r["source"].as_str().unwrap_or("?"),
                     g("files"), g("new"), g("appended"), g("rewritten"),
                     g("vanished"), g("unchanged"), g("ms"));
        }
        let bytes: u64 = reports.iter().filter_map(|r| r.get("bytes_to_capture")?.as_u64()).sum();
        println!("\n  {:.1} MB would be captured · {} ms total",
                 bytes as f64 / 1_048_576.0, started.elapsed().as_millis());
    }
    Ok(())
}

fn init(config_path: &PathBuf, machine_alias: Option<String>, force: bool) -> Result<()> {
    if config_path.exists() && !force {
        anyhow::bail!("{} already exists (use --force to overwrite)", config_path.display());
    }
    let mut cfg = Config::default();
    cfg.machine_alias = machine_alias;
    cfg.save(config_path).with_context(|| format!("writing {}", config_path.display()))?;

    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())
        .context("establishing machine identity")?;

    println!("config       {}", config_path.display());
    println!("archive root {}", cfg.archive_root.display());
    println!("machine      {} ({})", m.alias, m.id);
    println!("sources      {}", cfg.sources.len());
    for s in &cfg.sources {
        println!("  {:<12} {:?}  {}", s.id, s.layout, s.path.display());
    }
    Ok(())
}

fn status(config_path: &PathBuf, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {} (run `cs init` first?)", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;

    if json {
        let sources: Vec<_> = cfg
            .sources
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "path": s.path,
                    "layout": format!("{:?}", s.layout).to_lowercase(),
                    "exists": s.path.is_dir(),
                })
            })
            .collect();
        println!(
            "{:#}",
            serde_json::json!({
                "config": config_path,
                "archive_root": cfg.archive_root,
                "machine": { "id": m.id, "alias": m.alias },
                "machine_dir": m.dir(&cfg.archive_root),
                "sources": sources,
            })
        );
    } else {
        println!("config       {}", config_path.display());
        println!("archive root {}", cfg.archive_root.display());
        println!("machine      {} ({})", m.alias, m.id);
        println!("machine dir  {}", m.dir(&cfg.archive_root).display());
        println!("sources");
        for s in &cfg.sources {
            let mark = if s.path.is_dir() { "ok     " } else { "MISSING" };
            println!("  {mark} {:<12} {:?}  {}", s.id, s.layout, s.path.display());
        }
    }
    Ok(())
}
