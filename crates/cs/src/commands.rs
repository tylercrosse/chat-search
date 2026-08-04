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
        "claude-desktop" => {
            cs_import::claude_desktop::import(logical_path, bytes).into_iter().collect()
        }
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
    // `--source` desugars into the query's own filters rather than living beside them: one
    // home for a filter, whether it arrived as a flag or as `agent:` in the text.
    let parsed = if prefix { cs_core::Query::typeahead(text) } else { cs_core::Query::exact(text) }
        .with_source(source);
    let q = cs_core::SearchOptions {
        limit,
        field: if tools { cs_core::Field::Tools } else { cs_core::Field::Prose },
        include_off_path,
        nested,
        // The shape rides on the JSON and nothing else, because nothing else draws it: the
        // terminal listing prints a title, a date and a snippet. It is not free — see
        // `SearchOptions::shape` for what it costs — so the surface that cannot use it does
        // not pay for it.
        shape: json,
        ..cs_core::SearchOptions::new(cs_core::now_ms())
    };

    // A filter token whose value selects nothing has to say so. Returning unfiltered results
    // for a query that names a filter is a worse answer than returning none, because it looks
    // like it worked (chat-search-me9.15 is the discoverability half of the same gap).
    let rejected = parsed.rejected();
    if !rejected.is_empty() && !json {
        eprintln!(
            "note: {} — not a value this can select on, so it is not filtering",
            rejected.join(", ")
        );
    }

    if !flat {
        let groups = cs_core::search_grouped(&conn, &parsed, &q)?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        // Typeahead is excluded: `--prefix` fires once per keystroke, so logging it would
        // bury the handful of real queries under every prefix of each of them. A client
        // doing typeahead records the finished query with `cs pick` instead.
        if !prefix {
            log_search(&cfg, text, source, &groups, ms);
        }
        return render_groups(&groups, text, &rejected, ms, json);
    }

    let hits = cs_core::search(&conn, &parsed, &q)?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;

    if json {
        // Field names here are a contract a GUI consumes verbatim — additive changes only.
        println!("{:#}", serde_json::json!({
            "query": text, "ms": (ms * 100.0).round() / 100.0, "count": hits.len(),
            // `unapplied_filters` keeps its name: it is a published field (ADR 12) and still
            // means what it says. What narrowed is which filters land in it — since
            // `chat-search-6eb.11` only a value nothing can select on does.
            "results": hits, "unapplied_filters": rejected,
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
            if let Some(d) = h.destinations.first() {
                println!("    {}", d.shell_line(None));
            }
            println!();
        }
        println!("{} results · {:.1} ms", hits.len(), ms);
    }
    Ok(())
}

/// Record a search. Never fails the search it is describing — see `querylog::append`.
fn log_search(cfg: &Config, text: &str, source: Option<&str>, groups: &[cs_core::Group], ms: f64) {
    if !cfg.recording_queries() || cs_core::Query::exact(text).mode() == cs_core::Mode::Empty {
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
    kind: Option<&str>,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let conn = rusqlite_open(&db_path)?;

    let parsed = cs_core::Query::exact(text).with_source(source);
    let (mut shown, n) = if parsed.mode() == cs_core::Mode::Empty {
        // Picked off the no-query recent list. Worth recording — it says the conversation
        // was wanted without anything being typed — but there is no ranking to place it in.
        (Vec::new(), 0)
    } else {
        let q = cs_core::SearchOptions { limit, ..cs_core::SearchOptions::new(cs_core::now_ms()) };
        let groups = cs_core::search_grouped(&conn, &parsed, &q)?;
        let ids: Vec<String> = groups.iter().map(|g| g.conv_id.clone()).collect();
        let n = ids.len();
        (ids, n)
    };
    let rank = shown.iter().position(|c| c == conv_id).map(|i| i + 1);
    shown = cs_core::querylog::truncate_shown(shown);

    if cfg.recording_queries() {
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

    // Printing the reopen line is what makes this the natural place to select from: a client
    // pipes through `cs pick`, gets a line it can `eval`, and the selection is recorded on the
    // way past. This is the shape chat-search-me9.3 argued for — the point that knows which
    // conversation was chosen is already the point being asked how to open it.
    if !quiet {
        let ids: Option<(String, String)> = conn
            .query_row(
                "SELECT source, native_id FROM conversation WHERE id = ?1",
                rusqlite::params![conv_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let Some((source, native_id)) = ids else {
            anyhow::bail!("no conversation {conv_id} in the index");
        };
        let all = cs_core::destinations(&source, &native_id);
        let chosen = match kind {
            // Named explicitly, so an absent one is an error rather than a silent fallback to
            // a different application than the one asked for.
            Some(want) => {
                let found = all.iter().find(|d| d.label().eq_ignore_ascii_case(want));
                if found.is_none() {
                    let offered: Vec<&str> = all.iter().map(|d| d.label()).collect();
                    anyhow::bail!(
                        "{source} cannot open in {want}; it offers {}",
                        if offered.is_empty() { "nothing".into() } else { offered.join(", ") }
                    );
                }
                found
            }
            None => all.first(),
        };
        match chosen {
            Some(d) => println!("{}", d.shell_line(None)),
            // Distinct from an error: the pick was still recorded, there is simply no way back
            // in for this source. Silence on stdout keeps `eval "$(cs pick …)"` a no-op.
            None => eprintln!("{source} conversations cannot be reopened from here"),
        }
    }
    Ok(())
}

/// What has been searched for, and what answered it.
///
/// The counts printed under the table are not decoration. The fold sets aside far more than it
/// keeps — keystrokes on the way to a query, benchmark spans, picks made with nothing typed —
/// and chat-search-6eb.21 reads the distinct-need count here as its trigger for harvesting an
/// eval set. A number that does not say what it excluded cannot be read honestly.
pub fn needs(
    config_path: &Path,
    log: Option<PathBuf>,
    limit: usize,
    json: bool,
    driven: Option<&str>,
    why: Option<&str>,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let path = log.unwrap_or_else(|| cfg.query_log());
    if let Some(span) = driven {
        declare_driven(&path, span, why)?;
    }
    let (events, skipped) = cs_core::querylog::load(&path)?;
    let folded = cs_core::querylog::fold(&events);

    if json {
        println!("{:#}", serde_json::json!({
            "log": path, "events": events.len(), "unreadable": skipped,
            "keystrokes": folded.keystrokes, "driven": folded.driven, "spans": folded.spans,
            "browsed": folded.browsed,
            // Over every need, not the `limit` shown: a truncated list cannot be summed back
            // into the totals, and these two are what 6eb.21's trigger is read from.
            "judgements": folded.judgements(), "answered": folded.answered(),
            "needs": folded.needs.iter().take(limit).collect::<Vec<_>>(),
        }));
        return Ok(());
    }
    if events.is_empty() {
        println!("nothing logged yet — {}", path.display());
        println!("  `cs search` records what it was asked for; `cs pick` records what you opened");
        return Ok(());
    }

    println!("  {:<34} {:>8} {:>7}  {}", "query", "searches", "picks", "opened");
    for n in folded.needs.iter().take(limit) {
        let picks: usize = n.picked.iter().map(|(_, c)| c).sum();
        let top = n.picked.first().map(|(c, _)| c.as_str()).unwrap_or("—");
        println!("  {:<34} {:>8} {:>7}  {}", trunc(&n.q, 34), n.searches, picks, trunc(top, 40));
    }
    // Judgements and the queries behind them are printed as a pair because that is the shape
    // of chat-search-6eb.21's trigger — "roughly 50-100 picks across 20+ distinct queries" —
    // and either number alone reads as further along than the set actually is.
    println!(
        "\n  {} need(s) · {} event(s) · {} judgement(s) across {} answered quer(y/ies)",
        folded.needs.len(),
        events.len(),
        folded.judgements(),
        folded.answered()
    );

    // Each of these is a claim about what the log means, so each is named rather than summed
    // into one "folded away" figure that nobody could check.
    let mut aside = Vec::new();
    if folded.keystrokes > 0 {
        aside.push(format!("{} on the way to another query", folded.keystrokes));
    }
    if folded.driven > 0 {
        aside.push(format!("{} in {} driven span(s)", folded.driven, folded.spans));
    }
    if folded.browsed > 0 {
        aside.push(format!("{} browsed with nothing typed", folded.browsed));
    }
    if !aside.is_empty() {
        println!("  set aside: {}", aside.join(" · "));
    }
    if skipped > 0 {
        println!("  warning: {skipped} unreadable line(s) in {}", path.display());
    }
    Ok(())
}

/// Record that a span of the log was machine-driven, so the fold stops reading it as needs.
///
/// Authored rather than detected, and deliberately so: see `querylog::Event::Driven`. The
/// reason is mandatory because an exclusion nobody explained is the silent carry this exists
/// to prevent, and the pick count is reported because picks are the only relevance judgements
/// the log ever produces — excluding one should be a decision, not a side effect.
fn declare_driven(path: &Path, span: &str, why: Option<&str>) -> Result<()> {
    let why = why.context(
        "--driven needs --why: an exclusion with no reason is a silent one a week from now",
    )?;
    let (from, until) = driven_span(span, cs_core::now_ms())?;

    let (events, _) = cs_core::querylog::load(path)?;
    let inside: Vec<&cs_core::querylog::Event> = events
        .iter()
        .filter(|e| e.query().is_some() && e.ts() >= from && e.ts() < until)
        .collect();
    let picks = inside
        .iter()
        .filter(|e| matches!(e, cs_core::querylog::Event::Pick { .. }))
        .count();

    cs_core::querylog::append(path, &cs_core::querylog::Event::Driven {
        ts: cs_core::now_ms(),
        from,
        until,
        why: why.to_string(),
    })?;
    println!("  driven: {why}");
    println!("  covers {} event(s) in {}", inside.len(), path.display());
    if picks > 0 {
        println!("  warning: {picks} of them are picks — the only judgements this log has");
    }
    println!("  written as one line; delete it to take the exclusion back\n");
    Ok(())
}

/// `FROM..UNTIL` as two local wall clocks, half-open.
///
/// The last check is the one worth having. A span that ends in the future is not a statement
/// about traffic that happened, it is a standing order to discard whatever gets typed next —
/// and it would do that silently, since nothing about a search says it was meant to survive.
/// Rounding an afternoon of benchmarking up to `..tomorrow` is the natural way to write one.
fn driven_span(span: &str, now_ms: i64) -> Result<(i64, i64)> {
    let bound = |text: &str| {
        cs_core::time::local_instant(text).with_context(|| {
            format!("{text:?} is not a local date or time — try 2026-08-04 or 2026-08-04T10:30")
        })
    };
    let (from, until) = span
        .split_once("..")
        .context("--driven takes a half-open span, as in 2026-08-04..2026-08-05")?;
    let (from, until) = (bound(from)?, bound(until)?);
    if until <= from {
        anyhow::bail!("a driven span has to end after it starts");
    }
    if until > now_ms {
        anyhow::bail!("a driven span cannot end in the future — it would exclude searches nobody has made yet");
    }
    Ok((from, until))
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
    let e = cs_core::explain(&conn, conv_id, query, cs_core::now_ms())?;
    println!("{:#}", serde_json::to_value(&e)?);
    Ok(())
}

/// One conversation's messages, for a reader.
///
/// The whole point of `--json`: it is the only way a client that is not Rust can draw a
/// conversation, and every rule it needs — head path, which messages are drawn, which matches
/// may claim to have ranked it — is answered by `cs_core::blocks` rather than by the client.
/// A missing conversation is a real failure and exits nonzero, because a client cannot tell an
/// empty conversation from a wrong id if both print `[]`.
pub fn show(
    config_path: &Path,
    db_path: Option<PathBuf>,
    conv_id: &str,
    query: &str,
    json: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let conn = rusqlite_open(&db_path)?;
    let terms = cs_core::Query::exact(query).marking_terms();
    let blocks = cs_core::blocks::load(&conn, conv_id, &terms)?;
    if blocks.is_empty() {
        anyhow::bail!("no conversation {conv_id:?} in {} (or none of it is on the head path)", db_path.display());
    }
    let t = cs_core::Transcript::of(conv_id, &terms, blocks);
    if json {
        println!("{:#}", serde_json::to_value(&t)?);
        return Ok(());
    }
    // Deliberately thin. The readable form is a table of contents, not a second renderer —
    // `cs-tui` already owns the one that wraps, folds and marks, and a second one here would
    // be the duplication `blocks` exists to prevent.
    println!("{} — {} messages, {} drawn, {} thread(s)", t.conv_id, t.count, t.drawn, t.threads);
    for m in &t.messages {
        if !m.drawn {
            continue;
        }
        let first = m.block.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let head: String = first.chars().take(96).collect();
        let mark = if m.block.marks.is_empty() { ' ' } else { '*' };
        println!("{mark} {:>5}  {:<11} {:<9} {head}", m.block.seq, m.block.kind, m.block.role);
    }
    Ok(())
}

fn rusqlite_open(path: &Path) -> Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)
        .with_context(|| format!("opening {} (run `cs index` first?)", path.display()))?;
    // A schema change otherwise surfaces as `no such column` from inside a query, which
    // describes SQLite rather than the situation. The index is a pure function of the
    // archive (ADR 1), so the remedy never varies.
    cs_core::ensure_current(&conn).map_err(anyhow::Error::msg)?;
    Ok(conn)
}

/// Conversation-grouped output: the conversation is the result, its best matching messages
/// nest beneath it. Mirrors the shape docs search engines settled on, and it also dissolves
/// the duplicate-hit problem — a conversation's main thread and its subagents collapse into
/// one entry instead of competing for slots.
fn render_groups(
    groups: &[cs_core::Group],
    query: &str,
    rejected: &[String],
    ms: f64,
    json: bool,
) -> Result<()> {
    if json {
        println!("{:#}", serde_json::json!({
            "query": query, "ms": (ms * 100.0).round() / 100.0,
            "count": groups.len(), "grouped": true, "results": groups,
            "unapplied_filters": rejected,
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
        if let Some(d) = g.destinations.first() {
            println!("    {}", d.shell_line(g.cwd.as_deref()));
        }
        println!();
    }
    println!("{} conversations · {:.1} ms", groups.len(), ms);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-08-04, some hours after any bound the tests below name.
    const NOW: i64 = 1_785_880_000_000;

    #[test]
    fn a_driven_span_is_two_local_wall_clocks_around_a_double_dot() {
        let (from, until) = driven_span("2026-08-04T08:00..2026-08-04T12:00", NOW).unwrap();
        assert!(from < until);
        assert_eq!(until - from, 4 * 3_600_000, "four hours, whatever zone it is read in");
        // A bare date is the midnight opening its day, so a whole day is two dates.
        let (from, until) = driven_span("2026-08-03..2026-08-04", NOW).unwrap();
        assert!(until - from >= 23 * 3_600_000, "a civil day, short one across spring forward");
    }

    #[test]
    fn a_driven_span_that_ends_in_the_future_is_refused() {
        // The failure worth a guard. `..2026-08-05` is the natural way to round up an
        // afternoon of benchmarking, and it would go on discarding real searches for a day
        // without saying anything — nothing about a search says it was meant to survive.
        let err = driven_span("2026-08-04..2026-08-05", NOW).unwrap_err().to_string();
        assert!(err.contains("future"), "{err}");
    }

    #[test]
    fn a_span_that_is_not_a_span_says_so_rather_than_covering_nothing() {
        // Each of these would otherwise write a line that silently excludes zero events, or
        // everything, and a wrong exclusion is invisible in a fold that only prints totals.
        for bad in [
            "2026-08-04",
            "2026-08-04..",
            "..2026-08-04",
            "2026-08-04..2026-08-03",
            "2026-08-04..2026-08-04",
            "yesterday..today",
            "1785855600000..1785870000000",
        ] {
            assert!(driven_span(bad, NOW).is_err(), "{bad:?}");
        }
    }
}
