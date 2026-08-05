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
        // Four formats behind one id; the importer dispatches on the shape of the path.
        "google-takeout" => cs_import::google_takeout::import_all(logical_path, bytes),
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
    // Built beside the live index, never over it: until `commit` renames it into place every
    // query is still answered, in full, out of the previous build (chat-search-me9.28).
    let mut build = cs_core::IndexBuild::begin(&db_path).context("starting the build")?;

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
        let stats = cs_core::write_conversations_with(build.conn(), convs.iter(), opts)
            .with_context(|| format!("indexing {}", source.id))?;
        reports.push(source_report(&source.id, files.len(), failed, &stats, t0));
        accumulate(&mut totals, &stats);
    }

    // Before the summary, and inside the timing: what the numbers below describe is the index
    // a client can now query, not a file that was written and might yet fail to arrive. The
    // swap reads the new index back before returning, so the summary is downstream of a
    // committed importer version rather than of nothing having gone wrong (chat-search-6eb.35).
    build.commit().context("swapping the new index into place")?;
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
    let index = open_index(&db_path, json)?;
    // `--source` desugars into the query's own filters rather than living beside them: one
    // home for a filter, whether it arrived as a flag or as `agent:` in the text.
    let parsed = if prefix { cs_core::Query::typeahead(text) } else { cs_core::Query::exact(text) }
        .with_source(source);
    let opts = cs_core::SearchOptions {
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

    if flat {
        let answer = cs_core::FlatAnswer::of(&index, &parsed, &opts)?;
        if !json {
            notes(&answer.unapplied_filters, answer.index_state);
        }
        return render_flat(&answer, json);
    }

    let mut answer = cs_core::answer(&index, &parsed, &opts)?;
    // A one-shot search has no later moment to spend the second pass in, so it spends it here
    // and reports a whole number. `--prefix` is typeahead over argv — one process per
    // keystroke — and is the one caller that should leave the floor standing, because the
    // total of a query somebody is still typing is a number nobody is reading.
    //
    // A failure here fails the search rather than falling back to the floor. `settle` leaves a
    // true answer behind and a long-running client may well go on drawing it, but the counting
    // pass is the ranking query minus the scoring: if it cannot run, the rows that just came
    // back out of the same index are not trustworthy either, and printing them under a number
    // labelled `settled: false` would read as the ordinary typeahead case.
    if !prefix {
        answer.settle(&index).context("counting what the query selects")?;
    }
    if !json {
        notes(&answer.unapplied_filters, answer.index_state);
    }
    // Typeahead is excluded: `--prefix` fires once per keystroke, so logging it would
    // bury the handful of real queries under every prefix of each of them. A client
    // doing typeahead records the finished query with `cs pick` instead.
    if !prefix {
        log_search(&cfg, &parsed, text, source, &answer);
    }
    render_answer(&answer, json)
}

/// What a terminal is told beside the results, which a `--json` client reads as two fields.
///
/// Both are taken off the answer rather than recomputed from the query and the reader, so the
/// sentence a person reads and the key a program branches on are one statement made twice
/// rather than two that have to be kept in step.
fn notes(unapplied_filters: &[String], index_state: &str) {
    // A filter token whose value selects nothing has to say so. Returning unfiltered results
    // for a query that names a filter is a worse answer than returning none, because it looks
    // like it worked (chat-search-me9.15 is the discoverability half of the same gap).
    if !unapplied_filters.is_empty() {
        eprintln!(
            "note: {} — not a value this can select on, so it is not filtering",
            unapplied_filters.join(", ")
        );
    }
    // A note and not a warning: what came back is the previous build's answer in full, not a
    // partial one, because the new index is swapped in whole (chat-search-me9.28).
    if index_state == cs_core::IndexState::Rebuilding.as_str() {
        eprintln!("note: a rebuild is running — these results are the previous index's");
    }
}

/// Record a search. Never fails the search it is describing — see `querylog::append`.
///
/// `ms` is read off the answer rather than measured again. It used to be rounded here, in a
/// third copy of `(ms * 100.0).round() / 100.0`, so the number a need was recorded under and
/// the number the same search printed could differ in the last place — and the log is what a
/// later tuning run compares against.
///
/// `text` rather than the parsed query, because `--source` desugars into the query's own
/// filters and the log already carries `source` beside `q`. Folding on the desugared text would
/// split one need in two depending on how the filter was spelled at the shell.
fn log_search(
    cfg: &Config,
    parsed: &cs_core::Query,
    text: &str,
    source: Option<&str>,
    answer: &cs_core::Answer,
) {
    if !cfg.recording_queries() || parsed.mode() == cs_core::Mode::Empty {
        return;
    }
    let shown = cs_core::querylog::truncate_shown(
        answer.results.iter().map(|g| g.conv_id.clone()).collect(),
    );
    let _ = cs_core::querylog::append(&cfg.query_log(), &cs_core::querylog::Event::Search {
        ts: cs_core::now_ms(),
        q: text.to_string(),
        source: source.map(String::from),
        shown,
        n: answer.count,
        ms: answer.ms,
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
/// tuning run needs to compare against. It asks through `answer` like every other client, so
/// the ranking a pick is placed in is the one a search would have shown it in.
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
    // No `--json` here: what this prints on success is a shell line, so an error body would
    // be a second language on the same stream.
    let index = open_index(&db_path, false)?;
    let conn = &index.conn;

    let parsed = cs_core::Query::exact(text).with_source(source);
    let (mut shown, n) = if parsed.mode() == cs_core::Mode::Empty {
        // Picked off the no-query recent list. Worth recording — it says the conversation
        // was wanted without anything being typed — but there is no ranking to place it in.
        (Vec::new(), 0)
    } else {
        let q = cs_core::SearchOptions { limit, ..cs_core::SearchOptions::new(cs_core::now_ms()) };
        let ids: Vec<String> = cs_core::answer(&index, &parsed, &q)?
            .results
            .into_iter()
            .map(|g| g.conv_id)
            .collect();
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
    let index = open_index(&db_path, true)?;
    let conn = &index.conn;
    let e = cs_core::explain(conn, conv_id, query, cs_core::now_ms())?;
    println!("{:#}", serde_json::to_value(&e)?);
    Ok(())
}

/// One conversation's messages, for a reader.
///
/// The whole point of `--json`: it is the only way a client that is not Rust can draw a
/// conversation, and every rule it needs — head path, which messages are drawn, which matches
/// may claim to have ranked it, and which records are one sitting — is answered by
/// `cs_core::blocks` rather than by the client. A missing conversation is a real failure and
/// exits nonzero, because a client cannot tell an empty conversation from a wrong id if both
/// print `[]`.
pub fn show(
    config_path: &Path,
    db_path: Option<PathBuf>,
    conv_id: &str,
    query: &str,
    json: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let index = open_index(&db_path, json)?;
    let conn = &index.conn;
    let terms = cs_core::Query::exact(query).marking_terms();
    let t = cs_core::Transcript::read(conn, conv_id, &terms)?;
    if t.count == 0 {
        anyhow::bail!("no conversation {conv_id:?} in {} (or none of it is on the head path)", db_path.display());
    }
    if json {
        println!("{:#}", serde_json::to_value(&t)?);
        return Ok(());
    }
    // Deliberately thin. The readable form is a table of contents, not a second renderer —
    // `cs-tui` already owns the one that wraps, folds and marks, and a second one here would
    // be the duplication `blocks` exists to prevent.
    if let Some(sitting) = &t.sitting {
        // Said before the counts, because they are the whole sitting's and a reader who typed
        // one id deserves to know why the number is not the one on the record they named.
        println!(
            "{} records read as one sitting — a reconstruction, not something the export \
             recorded; {} minutes of silence ends one",
            sitting.members.len(),
            sitting.gap_ms / 60_000
        );
    }
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

/// Open the index for reading, or say why not in a form a client can branch on.
///
/// The `--json` audience gets an error *body* on stdout beside the nonzero exit. ADR 14 makes
/// the exit code part of the interface, but the code alone cannot say which situation it was,
/// and what arrived beside it was one English sentence — so the first non-Rust client to read
/// this contract classified index health by substring-matching another program's prose
/// (chat-search-me9.22). `code` is the contract; the message stays free to be reworded.
///
/// The body is a `Refusal`, which is core's shape rather than a fifth envelope assembled at the
/// printer: a refusal and the reply it stands in for are two halves of one contract, and this
/// was the half that lived furthest from it.
fn open_index(db_path: &Path, json: bool) -> Result<cs_core::Reader> {
    match cs_core::open_for_read(db_path) {
        Ok(reader) => Ok(reader),
        Err(why) => {
            if json {
                println!("{:#}", serde_json::to_value(cs_core::Refusal::from(&why))?);
            }
            Err(anyhow::Error::new(why))
        }
    }
}

/// Conversation-grouped output: the conversation is the result, its best matching messages
/// nest beneath it. Mirrors the shape docs search engines settled on, and it also dissolves
/// the duplicate-hit problem — a conversation's main thread and its subagents collapse into
/// one entry instead of competing for slots.
///
/// Both surfaces read the same `Answer`. Serializing it *is* the wire, the way serializing a
/// `Transcript` already is for `cs show --json`, so the terminal cannot come to describe a
/// different search than the JSON does — which is what four hand-built envelopes made possible.
fn render_answer(answer: &cs_core::Answer, json: bool) -> Result<()> {
    if json {
        println!("{:#}", serde_json::to_value(answer)?);
        return Ok(());
    }
    let query = &answer.query;
    if answer.results.is_empty() {
        println!("no results for {query:?}");
        println!("  if you expected one, `cs explain <conv-id> {query:?}` says why it missed");
        return Ok(());
    }
    for (i, g) in answer.results.iter().enumerate() {
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
        for m in &g.matches {
            let tag = if m.is_sidechain {
                "  [subagent]"
            } else if !m.on_head_path {
                "  [edited away]"
            } else {
                ""
            };
            println!("      {}{}", m.snippet, tag);
        }
        if g.match_count > g.matches.len() {
            println!("      +{} more match(es)", g.match_count - g.matches.len());
        }
        if let Some(d) = g.destinations.first() {
            println!("    {}", d.shell_line(g.cwd.as_deref()));
        }
        println!();
    }
    // What `--limit` hides and the rows cannot recover: a hundred results out of a hundred and
    // a hundred out of two thousand are the same hundred lines. Printed only once settled,
    // because an unsettled total is a floor and about half the truth on this corpus — a number
    // to be ranged, not shown. `--prefix` is the only caller that leaves one standing.
    match (answer.settled && answer.total > answer.count, answer.ms) {
        (true, ms) => println!("{} of {} conversations · {} ms", answer.count, answer.total, ms),
        (false, ms) => println!("{} conversations · {} ms", answer.count, ms),
    }
    Ok(())
}

/// `--flat`: matching messages, ungrouped, each naming the conversation it came out of.
///
/// A second envelope rather than a mode of the first — see `FlatAnswer`, which says why. The
/// terminal listing is correspondingly its own: one line per message, where the grouped one
/// nests messages under a conversation heading.
fn render_flat(answer: &cs_core::FlatAnswer, json: bool) -> Result<()> {
    if json {
        println!("{:#}", serde_json::to_value(answer)?);
        return Ok(());
    }
    let query = &answer.query;
    if answer.hits.is_empty() {
        println!("no results for {query:?}");
        println!("  if you expected one, `cs explain <conv-id> {query:?}` says why it missed");
        return Ok(());
    }
    for (i, h) in answer.hits.iter().enumerate() {
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
    // No total beside it, unlike the grouped listing: the number worth settling counts
    // conversations, and this list is not about conversations.
    println!("{} results · {} ms", answer.count, answer.ms);
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
