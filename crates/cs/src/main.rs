use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cs_archive::{machine, CaptureKind, Change, Config, Event, Fingerprint, Layout, Manifest, ManifestWriter, Op};
use std::path::{Path, PathBuf};

mod eval;
mod commands;
mod inventory;
mod tui;

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
        /// Bytes of each tool call/result to keep. 0 drops tool text entirely; it is all
        /// reproducible from the archive.
        #[arg(long, default_value = "1024")]
        tool_text_limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Search interactively, as you type.
    ///
    /// Same index and same ranking as `cs search`, in-process rather than per keystroke
    /// (ADR 14). A subcommand rather than its own binary so a separately-installed client
    /// cannot drift from the `cs` that built the index it is reading.
    Tui {
        #[arg(long)]
        db: Option<PathBuf>,
        /// Restrict to one source id.
        #[arg(long)]
        source: Option<String>,
        /// Conversations per search.
        ///
        /// Ranking cost scales with this, not with what is drawn: `search_grouped` pulls
        /// `limit * 50` candidate messages to rank. At 100 that was 5,000 scored to fill
        /// about 35 visible rows, and broad prefixes paid for it — `pro` measured 443 ms at
        /// 100 against 64 ms at 40 (chat-search-6eb.29). 50 keeps roughly a screen and a
        /// half of scroll. Partial mitigation, not the fix.
        #[arg(long, default_value = "50")]
        limit: i64,
        /// Print the resume command instead of running it.
        ///
        /// Enter launches the agent in place, which is what makes the TUI usable on its own.
        /// This is the escape hatch for a wrapper that wants the string — and is implied
        /// whenever stdin or stdout is not a terminal, so `eval "$(cs tui)"` still works.
        #[arg(long)]
        print: bool,
        /// Draw frames to text files under `--out` and exit, without entering the terminal.
        ///
        /// Not a user affordance: it is how a change to the TUI gets *shown* in a pull request
        /// rather than described. The Swift app's `--shot` writes PNGs because its frames are
        /// pixels; this writes text because a terminal frame already is text, which is what
        /// lets it run with no display at all. `docs/PULL-REQUESTS.md`.
        #[arg(long)]
        shot: bool,
        /// What `--shot` searches for. Ignored otherwise.
        #[arg(long, default_value = "borrow checker")]
        query: String,
        /// Where `--shot` writes its frames. One file per frame.
        #[arg(long, default_value = "/tmp/chat-search-tui")]
        out: PathBuf,
        /// The terminal size `--shot` draws at.
        ///
        /// Fixed rather than inherited, because a frame taken at whatever the author's window
        /// happened to be is not comparable with the one taken before the change.
        #[arg(long, default_value = "140x40")]
        size: String,
    },
    /// Search indexed conversations.
    Search {
        /// What to search for, filters included: `agent:claude,codex`, `-agent:codex`,
        /// `dir:!web-app`, `date:<3h`, `date:today`.
        ///
        /// `date:` also takes an absolute span, half-open and with either end optional:
        /// `date:2026-07-28..2026-08-02` is the 28th through the 1st, `date:2026-07-28..` is
        /// everything since, and `date:2026-07-28` is that day alone. A bound can carry a time
        /// — `date:2026-07-28T09:30..` — and the span is the one form that does not move
        /// overnight.
        ///
        /// A value holding a space or a comma goes in double quotes — `dir:"~/Mobile
        /// Documents"` — since without them whitespace ends the word and a comma ends the
        /// value. Quote the value, not the whole token, and mind your shell's quoting too.
        ///
        /// A filter value that names nothing selectable is reported rather than applied, and
        /// a half-typed one is searched as text — never an error.
        // clap renders the doc comment above as `--help`, so the reason lives down here
        // instead: `allow_hyphen_values` is what stops `-agent:codex` — one of the DSL's two
        // negation spellings — being read as a bundle of short flags and refused outright.
        // The filters have to live in the query text so they survive the move to a TUI, which
        // has one input box and no flags at all; a query that works there and fails here
        // would defeat the point of one parser (`chat-search-6eb.11`).
        #[arg(allow_hyphen_values = true)]
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
        /// Treat the last word as a prefix, for typeahead.
        #[arg(long)]
        prefix: bool,
        /// Matching messages to nest under each conversation.
        #[arg(long, default_value = "3")]
        nested: usize,
        /// One row per message instead of grouping by conversation.
        #[arg(long)]
        flat: bool,
        /// The client contract (ADR 12). docs/JSON-CONTRACT.md is every field it emits and,
        /// for the six that can be null, what the null means.
        #[arg(long)]
        json: bool,
    },
    /// The facet rail for a query: every source, what the query says about it, and the query
    /// text clicking it would produce.
    ///
    /// For a client that is not a Rust program. docs/TUI-DESIGN.md §5 requires a facet bar to
    /// be a projection of the query text rather than a selection kept beside it, and the
    /// rewriting rules that make that work live in `cs_core::query` with the grammar. A client
    /// in another process cannot call them, so this hands over the projection instead.
    Facets {
        /// The query the rail is a projection of. Same syntax as `cs search`.
        #[arg(allow_hyphen_values = true, default_value = "")]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
        /// The client contract. docs/JSON-CONTRACT.md says what it emits.
        #[arg(long)]
        json: bool,
    },
    /// When a query's answers happened: a bar per stretch of days, with the matches raised out
    /// of them.
    ///
    /// The facet rail for the one axis a rail cannot enumerate. `cs facets` hands each chip the
    /// query text clicking it produces; a scrubber's window is two instants out of a continuum,
    /// so `--drag` is that trade made the other way round — hand over two instants, get back the
    /// whole query text.
    ///
    /// It counts rather than lists, and it counts *here*, because a client holding a `--limit`
    /// page holds a biased sample of this axis: ranking is not chronological.
    Timeline {
        /// The query the distribution is of. Same syntax as `cs search`.
        // `allow_hyphen_values` for the reason `cs search` needs it: `-agent:codex` is one of
        // the DSL's two negation spellings.
        #[arg(allow_hyphen_values = true, default_value = "")]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
        /// How many bars to divide the axis into. A picture's resolution, so a surface that
        /// knows how wide it is may say; the default is what a 900 pt window wants.
        #[arg(long, default_value_t = cs_core::BUCKETS)]
        buckets: usize,
        /// Two epoch-millisecond instants, `FROM..UNTIL`, in whichever order the pointer
        /// visited them. Answers "what does this drag write into the query line" and changes
        /// nothing else about the reply.
        #[arg(long, value_name = "FROM..UNTIL")]
        drag: Option<String>,
        /// Treat the last word as a prefix, for typeahead. Pass it when the search beside this
        /// was asked that way: the two readings rank different sets, and a drawer under a list
        /// has to be describing that list.
        #[arg(long)]
        prefix: bool,
        /// The client contract. docs/JSON-CONTRACT.md says what it emits.
        #[arg(long)]
        json: bool,
    },
    /// Record that a search ended in opening this conversation, and print its resume command.
    ///
    /// The selection is the ground truth an eval set cannot invent: the query says what was
    /// wanted, and this says what answered it.
    Pick {
        conv_id: String,
        /// The search that produced the result list this was chosen from.
        // `allow_hyphen_values` for the reason `cs search` needs it: `-agent:codex` is one of
        // the DSL's two negation spellings, and without this clap reads it as a bundle of short
        // flags and refuses. A client recording a pick has no say in what the user typed, so a
        // query this cannot carry is a query whose pick is silently never recorded.
        #[arg(long, default_value = "", allow_hyphen_values = true)]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        source: Option<String>,
        /// How deep the list the choice was made from went.
        #[arg(long, default_value = "200")]
        limit: i64,
        /// Record the pick without printing the reopen line.
        #[arg(long)]
        quiet: bool,
        /// Which destination to print — `terminal` or `web`. Defaults to the source's best.
        ///
        /// Named `--in` because the question is "open it in what". A source that cannot offer
        /// the one asked for fails and says what it does offer, rather than quietly printing a
        /// different one.
        #[arg(long = "in")]
        kind: Option<String>,
    },
    /// Record that a search ended in nothing being opened.
    ///
    /// The other half of `cs pick`, for a client whose search runs in another process. A
    /// `Search` with no `Pick` after it is the abandonment signal — the ranking showed nothing
    /// worth opening — and it is the only thing this log ever learns that is not a success
    /// (docs/TUI-DESIGN.md §6). A typeahead client cannot get one out of `cs search`, which
    /// logs on the non-`--prefix` path only.
    Abandon {
        /// The search that was given up on. Same syntax as `cs search`.
        // `allow_hyphen_values` for the reason `cs pick --query` needs it: `-agent:codex` is
        // one of the DSL's two negation spellings, and a query this cannot carry is a query
        // whose abandonment is silently never recorded.
        #[arg(allow_hyphen_values = true)]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        source: Option<String>,
        /// How deep the list that was given up on went.
        #[arg(long, default_value = "200")]
        limit: i64,
    },
    /// What you have searched for, and what answered it.
    ///
    /// Needs rather than keystrokes: a query typed one character at a time is one row, and a
    /// query run three times to time it is one search. What that sets aside is printed under
    /// the table, because these counts are what chat-search-6eb.21 reads before harvesting an
    /// eval set out of them.
    Needs {
        #[arg(long, default_value = "40")]
        limit: usize,
        #[arg(long)]
        json: bool,
        /// Read a log other than the configured one.
        #[arg(long)]
        log: Option<PathBuf>,
        /// Record that a half-open span of the log was machine-driven, then show the result.
        ///
        /// Local dates or times: `2026-08-04..2026-08-05`, or
        /// `2026-08-04T09:47..2026-08-04T11:00`. A benchmark's queries cannot be told from real
        /// ones after the fact — both are ordinary text and both go unpicked — so this is how
        /// the person who ran it says which were which.
        #[arg(long, value_name = "FROM..UNTIL")]
        driven: Option<String>,
        /// What was being measured. Required by `--driven`.
        #[arg(long, value_name = "TEXT")]
        why: Option<String>,
    },
    /// Why a conversation did not come back for a query.
    Explain {
        conv_id: String,
        /// Same syntax as `cs search`, so the explanation is of the query you actually ran.
        #[arg(allow_hyphen_values = true)]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// One conversation's messages, head path only.
    ///
    /// `--json` is the client contract (ADR 12): every field a reader needs, including which
    /// messages are drawn and which matches are entitled to claim they ranked the
    /// conversation, so no client re-derives either.
    Show {
        conv_id: String,
        /// Same syntax as `cs search`. Marks the terms the ranker would have matched, so a
        /// highlight here means what it means in the results list. Omitted marks nothing.
        #[arg(allow_hyphen_values = true, default_value = "")]
        query: String,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Measure the ranking against judged queries.
    Eval {
        #[command(subcommand)]
        command: EvalCommand,
    },
    /// Capture changed files into the archive and record what was observed.
    ///
    /// Quiet: a run that observed nothing prints nothing. Problems still print.
    Archive {
        /// Limit to one source id.
        #[arg(long)]
        source: Option<String>,
        /// Report what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Print the table even on a run that observed nothing.
        #[arg(long)]
        verbose: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum EvalCommand {
    /// Write one gradeable sheet per query, for editing in whatever you edit text in.
    Sheet {
        #[arg(long, default_value = "evals/ranking.toml")]
        set: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        /// Results per query to pool from each ranking variant.
        #[arg(long, default_value_t = eval::DEPTH)]
        depth: usize,
        /// Write a single query's sheet.
        #[arg(long)]
        only: Option<String>,
        /// Rewrite sheets even where that discards grades not yet collected.
        #[arg(long)]
        force: bool,
    },
    /// Read the grade columns back out of the sheets and record them.
    Collect {
        #[arg(long, default_value = "evals/ranking.toml")]
        set: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        /// Record grades against conversation ids this index does not hold. Normally a
        /// mistyped id, so it is refused by default.
        #[arg(long)]
        allow_unknown: bool,
    },
    /// Score the current ranking against the judgements.
    Run {
        #[arg(long, default_value = "evals/ranking.toml")]
        set: PathBuf,
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long, default_value_t = eval::DEPTH)]
        depth: usize,
        /// Override the repeat-match damping, for a tuning sweep (chat-search-6eb.13).
        #[arg(long, default_value_t = cs_core::REPEAT_WEIGHT)]
        repeat_weight: f64,
        /// Override the recency decay, for a tuning sweep.
        #[arg(long, default_value_t = cs_core::DECAY)]
        decay: f64,
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
        Command::Index { db, tool_text_limit, json } => {
            commands::index(&config_path, db, tool_text_limit, json)
        }
        Command::Tui { db, source, limit, print, shot, query: q, out, size } => {
            if shot {
                tui::shot(&config_path, db, source.as_deref(), limit, &q, &out, &size)
            } else {
                tui::run(&config_path, db, source.as_deref(), limit, print)
            }
        }
        Command::Search { query: q, limit, db, source, tools, include_off_path, prefix, nested, flat, json } => {
            commands::search(&config_path, db, &q, limit, source.as_deref(), tools, include_off_path, prefix, nested, flat, json)
        }
        Command::Facets { query: q, db, json } => facets(&config_path, db, &q, json),
        Command::Timeline { query: q, db, buckets, drag, prefix, json } => {
            commands::timeline(&config_path, db, &q, buckets, drag.as_deref(), prefix, json)
        }
        Command::Pick { conv_id, query: q, db, source, limit, quiet, kind } => {
            commands::pick(&config_path, db, &conv_id, &q, source.as_deref(), limit, quiet, kind.as_deref())
        }
        Command::Abandon { query: q, db, source, limit } => {
            commands::abandon(&config_path, db, &q, source.as_deref(), limit)
        }
        Command::Needs { limit, json, log, driven, why } => {
            commands::needs(&config_path, log, limit, json, driven.as_deref(), why.as_deref())
        }
        Command::Explain { conv_id, query: q, db } => commands::explain(&config_path, db, &conv_id, &q),
        Command::Show { conv_id, query: q, db, json } => {
            commands::show(&config_path, db, &conv_id, &q, json)
        }
        Command::Eval { command } => match command {
            EvalCommand::Sheet { set, db, depth, only, force } => {
                eval::sheet(&config_path, db, &set, depth, only.as_deref(), force)
            }
            EvalCommand::Collect { set, db, allow_unknown } => {
                eval::collect(&config_path, db, &set, allow_unknown)
            }
            EvalCommand::Run { set, db, depth, repeat_weight, decay, json } => {
                eval::run(&config_path, db, &set, depth, repeat_weight, decay, json)
            }
        },
        Command::Archive { source, dry_run, verbose, json } => {
            archive(&config_path, source.as_deref(), dry_run, verbose, json)
        }
    }
}

/// Byte accounting for a capture run. Three numbers rather than one, because "how big are
/// these files" and "what did this cost me" are different questions (chat-search-a7k.6).
///
/// `apparent` is the size the captured files report. It is not a cost: an append re-captures
/// the whole file, so a transcript that grows by a few KB every five minutes bills its full
/// size again on every scan, forever.
///
/// `allocated` is the blocks the filesystem actually gave those files, which is what they
/// occupy. `cloned`/`copied` split that by how the bytes got in: a reflinked capture shares
/// its blocks with the source (ADR 20) and so adds nothing to the volume until the two
/// diverge, while a copied one is genuinely new bytes on disk.
#[derive(Default, Clone, Copy)]
struct Bytes {
    apparent: u64,
    allocated: u64,
    cloned: u64,
    copied: u64,
}

impl std::ops::AddAssign for Bytes {
    fn add_assign(&mut self, o: Self) {
        self.apparent += o.apparent;
        self.allocated += o.allocated;
        self.cloned += o.cloned;
        self.copied += o.copied;
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

/// Capture, then say as little as possible about it.
///
/// This is the launchd job: it runs every 300 s, and on the overwhelming majority of those
/// runs every file is unchanged. Printing the table anyway cost ~150 KB of log a day saying
/// "nothing happened", which is how the one line that mattered would have gone unread
/// (chat-search-a7k.5). So the table is printed only when the run recorded something, and
/// `--verbose` brings it back for debugging.
///
/// Suppression stops at the table. Errors propagate to stderr as they always did, and the
/// source-drift report (chat-search-a7k.12) and the export-staleness nag (chat-search-a7k.10)
/// each print on their own throttle — a warning quiet mode can swallow is worse than the noise
/// quiet mode removes.
fn archive(
    config_path: &PathBuf,
    only: Option<&str>,
    dry_run: bool,
    verbose: bool,
    json: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    let machine_dir = m.dir(&cfg.archive_root);
    // Mutable because this run's own events are folded back in below, before the staleness
    // check reads it.
    let mut manifest = Manifest::load(&machine_dir).context("loading manifest")?;
    let writer = (!dry_run)
        .then(|| ManifestWriter::new(&machine_dir))
        .transpose()
        .context("opening manifest for append")?;

    // Detection runs here, on every archive, and not only inside `Config::default` where
    // only `cs init` could ever reach it (chat-search-a7k.12). A dozen `stat` calls before a
    // scan that walks tens of thousands of files.
    let drift = cs_archive::drift::detect(&cfg.sources);
    // A dry run is somebody asking a question, so it always answers and records nothing —
    // it is the unthrottled way to see the current state. Scheduled runs go through the
    // throttle so an unadopted candidate cannot print 288 times a day.
    let show_drift = if dry_run {
        !drift.is_empty()
    } else {
        cs_archive::drift::claim(&machine_dir, &drift, cs_archive::manifest::now_ms())
            .context("recording the source-drift report")?
    };

    let started = std::time::Instant::now();
    let mut reports = Vec::new();
    let (mut tot_cloned, mut tot_copied) = (0u64, 0u64);
    let mut tot = Bytes::default();
    // Whether the run has anything to say. Read off the events it recorded rather than
    // recounted from the table, so "the log stayed silent" and "the manifest gained nothing"
    // can never disagree — a second rule for the same question is how a vanished file would
    // end up unreported.
    let mut recorded_anything = false;

    for source in &cfg.sources {
        if only.is_some_and(|o| o != source.id) || source.layout == Layout::Bundle {
            continue;
        }
        let scan = cs_archive::scan_source(source, &manifest)
            .with_context(|| format!("scanning {}", source.id))?;

        let t0 = std::time::Instant::now();
        let (mut cloned, mut copied) = (0u64, 0u64);
        let mut bytes = Bytes::default();
        let mut events = Vec::new();

        for c in &scan.changes {
            let op = match c.change {
                Change::New => Op::Seen,
                Change::Appended => Op::Appended,
                Change::Rewritten => Op::Rewritten,
                Change::Vanished => Op::Vanished,
                Change::Unchanged => continue, // nothing observed worth recording
            };

            if c.change.needs_capture() {
                bytes.apparent += c.fingerprint.as_ref().map_or(0, |fp| fp.size);
                bytes.allocated += c.allocated_bytes;

                if !dry_run {
                    let kind = cs_archive::capture_file(
                        &machine_dir, &source.id, &c.rel_path, &c.abs_path, c.change,
                        cs_archive::manifest::now_ms(),
                    )
                    .with_context(|| format!("capturing {}", c.abs_path.display()))?;
                    // Bill the destination's occupancy against how it got there: the source's
                    // allocation is the right proxy either way, since a clone and a copy both
                    // land the same blocks — they differ only in whether those blocks are new.
                    match kind {
                        CaptureKind::Cloned => {
                            cloned += 1;
                            bytes.cloned += c.allocated_bytes;
                        }
                        CaptureKind::Copied => {
                            copied += 1;
                            bytes.copied += c.allocated_bytes;
                        }
                    }
                }
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

        recorded_anything |= !events.is_empty();
        if let Some(w) = &writer {
            w.append(&events).context("appending manifest events")?;
        }
        // Fold this run's events into the in-memory manifest, so the staleness check below
        // sees an export that landed moments ago as fresh. Without this, `cs archive` run by
        // hand straight after unpacking one would nag about the very export it just captured
        // — the worst possible moment to be wrong, because it is the moment the user did the
        // right thing. On a dry run nothing is written but the files are on disk all the same,
        // and it is their existence, not their capture, that closes the gap.
        for e in events {
            manifest.apply(e);
        }

        tot_cloned += cloned;
        tot_copied += copied;
        tot += bytes;
        reports.push(serde_json::json!({
            "source": scan.source,
            "new": scan.count(Change::New),
            "appended": scan.count(Change::Appended),
            "rewritten": scan.count(Change::Rewritten),
            "vanished": scan.count(Change::Vanished),
            "unchanged": scan.count(Change::Unchanged),
            "cloned": cloned, "copied": copied,
            // `bytes` is kept as-is for consumers that already read it, but it is apparent
            // size, not cost. `apparent_bytes` is the same number under a name that says so;
            // prefer it, and prefer `allocated_bytes` when you want the disk figure.
            "bytes": bytes.apparent,
            "apparent_bytes": bytes.apparent,
            "allocated_bytes": bytes.allocated,
            "cloned_bytes": bytes.cloned,
            "copied_bytes": bytes.copied,
            "ms": t0.elapsed().as_millis() as u64,
        }));
    }

    // After the loop, so the manifest already carries what this run captured. Every source is
    // asked, not just the one `--source` selected: an export left to rot is exactly as lost
    // whether or not this invocation happened to be scanning it.
    let stale = cs_archive::staleness::detect(
        &cfg.sources,
        |id| manifest.newest_mtime_ms(id),
        cs_archive::manifest::now_ms(),
    );
    // Same split as the drift report: a dry run is somebody asking a question, so it answers
    // unthrottled and records nothing, and scheduled runs go through the throttle.
    let show_stale = if dry_run {
        !stale.is_empty()
    } else {
        cs_archive::staleness::claim(&machine_dir, &stale, cs_archive::manifest::now_ms())
            .context("recording the export-staleness nag")?
    };

    // Read off the config alone, because there is nothing else to read: these surfaces write no
    // local file, so the scan above could not have found them and neither could `cs init`
    // (chat-search-a7k.22). Same dry-run split as the two reports above it.
    let unreachable = cs_archive::unreachable::pending(&cfg.sources);
    let show_unreachable = if dry_run {
        !unreachable.is_empty()
    } else {
        cs_archive::unreachable::claim(&machine_dir, &unreachable, cs_archive::manifest::now_ms())
            .context("recording the unreachable-surfaces report")?
    };

    if json {
        println!("{:#}", serde_json::json!({
            "dry_run": dry_run, "archive": machine_dir, "sources": reports,
            // Deliberately not throttled: the throttle exists to keep a human's log readable,
            // and a consumer polling JSON wants the current state on every poll, not a field
            // that silently empties for 24 h after the first sighting.
            "unconfigured": drift.unconfigured,
            "missing": drift.missing,
            "stale": stale,
            "unreachable": unreachable,
            "cloned": tot_cloned, "copied": tot_copied,
            "bytes": tot.apparent,
            "apparent_bytes": tot.apparent,
            "allocated_bytes": tot.allocated,
            "cloned_bytes": tot.cloned,
            "copied_bytes": tot.copied,
            "ms": started.elapsed().as_millis() as u64,
        }));
    } else {
        // A dry run is somebody asking a question, so it answers even when the answer is
        // "nothing" — the same reason the drift report skips its throttle for one. An empty
        // reply to a command typed by hand reads as a broken binary, not as good news.
        let show_table = recorded_anything || verbose || dry_run;
        if show_table {
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
            println!("\n  {:.1} MB apparent size · {:.1} MB allocated blocks · {} ms",
                     mb(tot.apparent), mb(tot.allocated), started.elapsed().as_millis());
            if !dry_run {
                println!("  {} cloned ({:.1} MB, shares blocks with the source) · {} copied ({:.1} MB new on disk)",
                         tot_cloned, mb(tot.cloned), tot_copied, mb(tot.copied));
            }
            println!("  archive: {}", machine_dir.display());
        }
        // Each block separates itself from whatever printed above it, and prints nothing when
        // it is first: on a quiet run one of these is the entire output and must not open with
        // a stray newline. Tracking what has printed rather than testing `show_table` is what
        // keeps that true now there are two of them — the nag has to be able to stand alone,
        // below a table, or below a drift report it has never heard of.
        //
        // `--verbose` is deliberately not wired into either throttle: restoring the old table
        // is all it claims to do, and `--dry-run` is already the unthrottled view of both.
        let mut printed = show_table;
        if show_drift {
            if printed {
                println!();
            }
            print_drift(&drift, config_path);
            printed = true;
        }
        if show_stale {
            if printed {
                println!();
            }
            print_stale(&stale);
            printed = true;
        }
        if show_unreachable {
            if printed {
                println!();
            }
            print_unreachable(&unreachable, config_path);
        }
    }
    Ok(())
}

/// Exports that have stopped happening (chat-search-a7k.10).
///
/// Exempt from quiet mode for the reason the drift report is: a staleness warning that quiet
/// mode can suppress is a warning that vanishes exactly when you stop reading the logs, which
/// is the same failure shape as the Claude Code 30-day prune — silent, and only noticed once
/// the data is gone.
fn print_stale(stale: &[cs_archive::Stale]) {
    for s in stale {
        println!("  stale         {:<14} {} days since its newest archived file", s.source, s.days);
    }
    // Restated every time, because the remedy is not obvious from the line and the cost of
    // not knowing it is unrecoverable. The threshold is a week, so this prints at most once a
    // day and only while something is actually rotting.
    println!(
        "\n  Nothing accrues in a manual export between runs, so each of those days is a gap no\n  later export can fill (ADR 21). Re-export and unpack into the watched directory."
    );
}

/// The two ways a config stops describing the machine it runs on (chat-search-a7k.12).
///
/// Printed last, and separately from the table, because that is where it has to survive:
/// under quiet mode (chat-search-a7k.5) the table is gone on an idle run and these are the
/// only lines left. Nothing above them is required for them to make sense, and the blank line
/// that separates them from the table is the caller's — there is nothing to separate from
/// when they are the whole output.
fn print_drift(drift: &cs_archive::Drift, config_path: &Path) {
    for s in &drift.unconfigured {
        println!("  unconfigured  {:<14} {}", s.id, s.path.display());
    }
    for s in &drift.missing {
        println!("  missing       {:<14} {}", s.id, s.path.display());
    }

    if !drift.unconfigured.is_empty() {
        // Not adopted automatically, and the reason is worth restating every time it prints:
        // an id becomes part of every conversation id the moment the source is first
        // captured (ADR 16), and there is no rename afterwards. The block below is the whole
        // decision, so the explicit act costs one paste rather than an afternoon of guessing
        // include globs.
        println!("\n  Present but not captured. `cs` will not add these for you — a source id is\n  permanent once it is in the archive (ADR 16). Append to {}:\n", config_path.display());
        for line in drift.adoption_toml().lines() {
            // The blank line between blocks stays blank; indenting it would leave trailing
            // whitespace that a paste carries into the config.
            println!("{}", if line.is_empty() { String::new() } else { format!("      {line}") });
        }
    }
    if !drift.missing.is_empty() {
        println!("\n  Configured but gone: uninstalled, moved, or an unmounted volume. What is already\n  archived is safe, but nothing new is arriving, and this run recorded every file under\n  it as vanished.");
    }
}

/// Surfaces whose conversations never touch this disk (chat-search-a7k.22).
///
/// Exempt from quiet mode with the other two standing reports, and on a stronger argument than
/// either of them. Drift and staleness describe something that changed, so a swallowed line is
/// re-earned by the next change; this one describes a gap that has been there since `cs init`
/// and that nothing on the machine will ever raise again. Suppressing it would not delay the
/// news, it would end it.
///
/// Shared with `cs init`, which is the other moment a person is looking — and the moment the
/// config that omits them is being written. One renderer, so the two cannot come to differ
/// about what the remedy is.
fn print_unreachable(pending: &[&cs_archive::Surface], config_path: &Path) {
    for s in pending {
        println!("  unreachable   {:<14} {} — {}", s.id, s.name, s.fetch);
    }
    // The share is restated on every printing for the reason the staleness remedy is: the lines
    // above are a fact about plumbing, and this is the only thing that says why it is worth
    // acting on. It is stated about the whole category rather than about whichever surfaces are
    // still pending, because the measured one is usually the first to be configured — and the
    // stakes of the two left do not shrink when it is.
    println!(
        "\n  These keep every conversation on a vendor's machine and write nothing here, so no\n  \
         detection can find them and no improvement to detection ever will (ADR 21) — the\n  \
         config is the only place they can be named. Not a rounding error either: the one\n  \
         of them ever measured is the ChatGPT export, at {}\n  \
         on the machine this was built for — and the others contributed nothing to that\n  \
         count, so it is a floor and not a share. Once one is configured, the `stale` line\n  \
         is what says it has stopped arriving.",
        cs_archive::unreachable::measured_share(),
    );

    let toml = cs_archive::unreachable::adoption_toml(pending);
    if !toml.is_empty() {
        // `path` is the one field this cannot fill in: an export has no canonical home, which
        // is the whole reason detection fails on these. Left unedited it points at a directory
        // that does not exist, and the next run says so as `missing` — a half-finished paste
        // reports itself rather than passing for configured.
        println!("\n  Fetch one and unpack it anywhere. Point `path` at whichever directory you unpack\n  exports into — one serves all of them, and until it exists `cs archive` reports it\n  as `missing`. The id is permanent once bytes are captured (ADR 16). Append to\n  {}:\n", config_path.display());
        for line in toml.lines() {
            // The blank line between blocks stays blank; indenting it would leave trailing
            // whitespace that a paste carries into the config.
            println!("{}", if line.is_empty() { String::new() } else { format!("      {line}") });
        }
    }

    let unknown: Vec<&str> =
        pending.iter().filter(|s| s.include.is_empty()).map(|s| s.id).collect();
    if !unknown.is_empty() {
        // Named rather than quietly omitted. An id in the list above with no block beside it
        // reads as an oversight, and the reason it is not one is worth the two lines: a guessed
        // glob captures part of an export and says nothing about the rest, which is the failure
        // that looks most like success.
        println!(
            "\n  No block for {}: no export of that shape has landed here yet, and a guessed\n  include glob captures part of one and stays silent about the rest.",
            unknown.join(", ")
        );
    }
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
            // `bytes_to_capture` keeps its existing meaning — apparent size — for consumers
            // that already read it. `allocated_to_capture` is what those files occupy, which
            // is the figure that answers "what will this cost me".
            "bytes_to_capture": scan.changes.iter()
                .filter(|c| c.change.needs_capture())
                .filter_map(|c| c.fingerprint.as_ref().map(|f| f.size))
                .sum::<u64>(),
            "allocated_to_capture": scan.changes.iter()
                .filter(|c| c.change.needs_capture())
                .map(|c| c.allocated_bytes)
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
        let sum = |k: &str| -> u64 { reports.iter().filter_map(|r| r.get(k)?.as_u64()).sum() };
        println!("\n  {:.1} MB apparent size · {:.1} MB allocated blocks would be captured · {} ms total",
                 mb(sum("bytes_to_capture")), mb(sum("allocated_to_capture")),
                 started.elapsed().as_millis());
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

    // The list above is everything detection can offer, and on a fresh machine it is a minority
    // of the corpus (chat-search-a7k.22). Said here as well as in `cs archive` because this is
    // the moment the incomplete config is written, and a person who walks away from a finished
    // `cs init` believing it found everything has no reason to look again.
    let unreachable = cs_archive::unreachable::pending(&cfg.sources);
    if !unreachable.is_empty() {
        println!();
        print_unreachable(&unreachable, config_path);
    }
    Ok(())
}

fn status(config_path: &PathBuf, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {} (run `cs init` first?)", config_path.display()))?;
    let m = machine::load_or_create(&cfg.archive_root, cfg.machine_alias.as_deref())?;
    // Asked rather than discovered by failing a search: a client that wants to draw "building
    // one now" before anyone types has nowhere else to look (chat-search-me9.28).
    let db = cfg.default_db();
    let index_state = cs_core::IndexState::of(&db);

    // One detection for the whole command: `watched` needs the presence of each directory and
    // the table below needs the paths, and re-stat'ing every candidate for the second answer
    // would be two derivations of one fact.
    let drift = cs_archive::drift::detect(&cfg.sources);
    let watched = inventory::watched(&cfg, &drift);
    let inventoried = inventory::census(&db, &watched);

    if json {
        let sources: Vec<_> = inventoried
            .iter()
            .map(|s| {
                let known = inventory::source_by_id(&cfg, &drift, &s.id);
                serde_json::json!({
                    "id": s.id,
                    "coverage": s.coverage.as_str(),
                    "conversations": s.conversations,
                    "path": known.map(|k| &k.path),
                    "layout": known.map(|k| format!("{:?}", k.layout).to_lowercase()),
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
                "index": { "path": db, "state": index_state.as_str() },
                "sources": sources,
            })
        );
    } else {
        println!("config       {}", config_path.display());
        println!("archive root {}", cfg.archive_root.display());
        println!("machine      {} ({})", m.alias, m.id);
        println!("machine dir  {}", m.dir(&cfg.archive_root).display());
        println!("index        {} ({})", db.display(), index_state.as_str());
        // Every source the machine knows about, not just the configured ones: a location
        // detected here and claimed by nothing is exactly what `cs status` should be able to
        // show, and a configured source reading 0 is the broken-importer signal.
        println!("sources");
        for s in &inventoried {
            let known = inventory::source_by_id(&cfg, &drift, &s.id);
            println!(
                "  {:<12} {:<14} {:>7}  {:<7} {}",
                s.coverage.as_str(),
                s.id,
                s.conversations,
                known.map_or_else(|| "-".to_string(), |k| format!("{:?}", k.layout)),
                known.map_or_else(String::new, |k| k.path.display().to_string()),
            );
        }
    }
    Ok(())
}

/// The facet rail for one query.
///
/// **This never refuses.** `cs search` cannot answer without a readable index and says so with
/// a nonzero exit, but the rail can: on a first run the true rail is every configured source
/// at zero, which is the state a client most needs to draw and the one an error would hide.
/// `index_state` beside the counts is what says whether they are provisional.
///
/// The projection itself is `cs_core::facets`, because it is made of the grammar's rewriting
/// rules and this crate is not where the grammar lives. All that happens here is the half
/// `cs-core` is not allowed to know: which sources the config names, and which of their
/// directories are on this disk (docs/TUI-DESIGN.md §1).
fn facets(config_path: &PathBuf, db_path: Option<PathBuf>, text: &str, json: bool) -> Result<()> {
    let cfg = Config::load(config_path)
        .with_context(|| format!("reading {} (run `cs init` first?)", config_path.display()))?;
    let db = db_path.unwrap_or_else(|| cfg.default_db());
    let watched = inventory::watched(&cfg, &cs_archive::drift::detect(&cfg.sources));
    // Typeahead, because a rail is a typeahead affordance — and it makes no difference to
    // anything below anyway. The two readings differ only in whether the final term expands,
    // which reaches `match_expr` and nothing the projection touches.
    let query = cs_core::Query::typeahead(text);
    // One clock for the whole reply, so that no two spans can be counted either side of a
    // midnight and no chip can be labelled from a different day than the one it counted.
    let (sources, dirs, dates) =
        inventory::rails(&db, &watched, DIR_CHIPS, cs_core::now_ms());
    let source_rail = cs_core::facets::sources(&query, &sources);
    let dir_rail = cs_core::facets::dirs(&query, &dirs);
    let date_rail = cs_core::facets::dates(&query, &dates);

    if json {
        println!(
            "{:#}",
            serde_json::json!({
                "v": 1,
                // The query as parsed, so a client can tell which of several replies it holds
                // — the same field, for the same reason, as the search envelope's.
                "query": query.raw(),
                "index_state": cs_core::IndexState::of(&db).as_str(),
                "sources": source_rail,
                "dirs": dir_rail,
                "dates": date_rail,
            })
        );
    } else {
        // Drawn in the order a client draws them: recency first, because `ended_at` answers for
        // every conversation and `cwd` for a third of them (`poc/ui`'s sidebar orders by coverage
        // rather than by how interesting the facet is).
        section("when", date_rail.keyword, &format!("{} spans", date_rail.values.len()));
        row(chip_mark(date_rail.all.selected), "any time", "", &date_rail.all.query);
        for c in &date_rail.values {
            row(mark(c.state), c.label, &c.conversations.to_string(), &c.query);
        }

        section("sources", source_rail.keyword, "config ∪ index");
        row(chip_mark(source_rail.all.selected), "all sources", "", &source_rail.all.query);
        for c in &source_rail.values {
            let count = format!("{} {}", c.conversations, c.coverage);
            row(mark(c.state), &c.value, &count, &c.query);
        }

        section(
            "directories",
            dir_rail.keyword,
            &format!("{} of {} indexed · {} record none", dir_rail.values.len(), dir_rail.indexed, dir_rail.undirected),
        );
        row(chip_mark(dir_rail.all.selected), "anywhere", "", &dir_rail.all.query);
        for c in &dir_rail.values {
            row(mark(c.state), &c.value, &c.conversations.to_string(), &c.query);
        }
    }
    Ok(())
}

/// How many directories the rail carries.
///
/// A rail rather than a list. `chat-search-6eb.26` measured a large share of this corpus's
/// directories to be per-conversation scratch dirs, so the tail is worth counting and not worth
/// drawing — and `indexed` beside the chips says how much of it was left out.
const DIR_CHIPS: usize = 12;

fn section(label: &str, keyword: &str, meta: &str) {
    println!("\n{label}  {keyword} · {meta}");
}

/// One chip. The click is last because a path can be wider than any column, and the column that
/// then runs on is the one nobody is reading down.
fn row(mark: &str, value: &str, count: &str, click: &str) {
    println!("  {mark:<5} {value:<52} {count:>13}  {click:?}");
}

fn mark(state: cs_core::ChipState) -> &'static str {
    match state {
        cs_core::ChipState::Include => "[on]",
        cs_core::ChipState::Exclude => "[not]",
        cs_core::ChipState::Off => "",
    }
}

fn chip_mark(selected: bool) -> &'static str {
    if selected {
        "[on]"
    } else {
        ""
    }
}

