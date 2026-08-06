//! Wiring for `cs tui`.
//!
//! Everything config-shaped lives here so `cs-tui` does not have to know it (docs/TUI-DESIGN.md
//! §1): this resolves the index path and the query log, and hands the TUI a sink. The TUI
//! resolves neither, which is what keeps it off `cs-archive`.

use anyhow::{Context, Result};
use cs_archive::Config;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

pub fn run(
    config_path: &Path,
    db_path: Option<PathBuf>,
    source: Option<&str>,
    limit: i64,
    print: bool,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());

    // Captured by the sink rather than read per event: the TUI can emit while the terminal
    // is still in raw mode, and re-reading the config there would be a filesystem hit inside
    // a keystroke path.
    let log_path = cfg.query_log();
    let log_queries = cfg.recording_queries();
    let mut sink = |event: cs_core::querylog::Event| {
        // Same rule as everywhere else this is called: losing a log line costs a data point,
        // failing the action costs the thing that was asked for.
        if log_queries {
            let _ = cs_core::querylog::append(&log_path, &event);
        }
    };

    // The facet bar's other half. The TUI can count what the index holds; it cannot know that
    // a source holding nothing is configured and broken rather than a tool nobody uses, and
    // that is a question about the config, which stops here (§1).
    let watched = crate::inventory::watched(&cfg, &cs_archive::drift::detect(&cfg.sources));

    // Unlike `cs pick`, the TUI does not recompute the result list to find the rank — it
    // built the list, so it knows where the selection sat and emits the event itself.
    let opts = cs_tui::Opts {
        query: String::new(),
        source: source.map(String::from),
        limit,
        watched,
    };

    match cs_tui::run(db_path, &mut sink, opts)? {
        cs_tui::Exit::Quit => Ok(()),
        cs_tui::Exit::Open { destinations, cwd, .. } => {
            // §6's sticky default: the table's first entry is what Enter takes when nobody is
            // asked. The picker that chooses between several is chat-search-me9.1's.
            let Some(dest) = destinations.into_iter().next() else {
                // Some sources have no reopen path at all — Gemini CLI writes no resumable
                // session. Saying so on stderr beats the TUI closing on Enter and appearing to
                // do nothing, which is indistinguishable from a bug.
                eprintln!("no way to reopen this conversation from here");
                return Ok(());
            };
            let cwd = cwd.filter(|d| !d.is_empty());
            // Handing over the terminal is only possible when we hold one. Under
            // `eval "$(cs tui)"` stdout is a pipe, and the agent would render into it; that
            // is the case the printing contract exists for, so it keeps printing.
            if print || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                println!("{}", dest.shell_line(cwd.as_deref()));
                return Ok(());
            }
            launch(&dest, cwd.as_deref())
        }
    }
}

/// Draw the TUI's frames to text files and exit, for `--shot`.
///
/// Deliberately not routed through [`run`]: there is no sink, so no scripted run can reach
/// `queries.jsonl`. The Swift app states the same rule out loud under `--shot`, and for the same
/// reason — a benchmark's queries are not needs anybody had (`6eb.21`).
pub fn shot(
    config_path: &Path,
    db_path: Option<PathBuf>,
    source: Option<&str>,
    limit: i64,
    query: &str,
    out: &Path,
    size: &str,
) -> Result<()> {
    let (width, height) = parse_size(size)
        .with_context(|| format!("--size wants WxH, as in 140x40; got {size:?}"))?;

    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());
    let watched = crate::inventory::watched(&cfg, &cs_archive::drift::detect(&cfg.sources));

    let opts = cs_tui::Opts {
        query: query.to_string(),
        source: source.map(String::from),
        limit,
        watched,
    };

    let frames = cs_tui::shot::frames(db_path, opts, width, height)?;
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;

    println!("{query:?} at {width}x{height}, nothing written to the query log");
    for frame in &frames {
        // All three, always. They answer different questions and any one alone misleads: the
        // text diffs but drops colour, the `.ans` is exact but renders as mojibake in a PR body,
        // and the SVG renders but resolves named colours against a palette it had to choose.
        for (ext, body) in [("txt", &frame.text), ("ans", &frame.ansi), ("svg", &frame.svg)] {
            let path = out.join(format!("tui-{}.{ext}", frame.name));
            std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        }
        println!(
            "  {}/tui-{}.{{txt,ans,svg}} ({} lines, {} KB of SVG)",
            out.display(),
            frame.name,
            frame.text.lines().count(),
            frame.svg.len() / 1024
        );
    }
    println!("`cat` an .ans in a terminal for the true colours; the .svg states the palette it chose.");
    Ok(())
}

/// `WxH` into a pair. Malformed input is an error rather than a default, unlike the Swift app's
/// `--size`: there it would silently change what a measurement was taken at, here it would
/// silently make two frames incomparable, and a shot nobody can trust is worse than no shot.
fn parse_size(s: &str) -> Option<(u16, u16)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Replace this process with the agent, so it inherits the terminal directly.
///
/// **Nothing here may redirect a standard descriptor.** A crossterm TUI — Codex is one —
/// fails to build its event source when fd 0 is anything other than the terminal it was
/// started on, and a freshly-opened `/dev/tty` counts as *other*: registering that fd with
/// kqueue fails on macOS, which surfaces as "reader source not set" and, in Claude Code, as
/// a session that draws but never accepts a keystroke. The obvious-looking fix — `< /dev/tty`
/// on the exec, which the fzf script this replaced did — is measurably the cause rather than
/// the cure; plain inheritance is what works.
///
/// No shell is involved either. `$SHELL -lc` re-sources the whole rc chain first, and
/// anything in there that touches the terminal lands between us and the agent.
fn launch(dest: &cs_core::Destination, cwd: Option<&str>) -> Result<()> {
    let argv = dest.argv();
    let Some((program, args)) = argv.split_first() else {
        // Unreachable through `destinations`, which returns no entry rather than an empty one.
        // Reported rather than returned quietly, so a future table entry that produces one is
        // a message instead of an Enter that does nothing.
        eprintln!("this conversation resolved to an empty command");
        return Ok(());
    };

    let mut command = std::process::Command::new(program);
    command.args(args);
    // The directory becomes an argument rather than a `cd` composed into a shell line, so
    // nothing in it can be read as syntax.
    if let Some(dir) = cwd {
        // Probed first: `exec` reports a missing directory as a failure to run the *program*,
        // which sends you looking for a broken install instead of a directory that has since
        // been deleted. Codex writes per-conversation scratch dirs, so this is ordinary.
        if !std::path::Path::new(dir).is_dir() {
            anyhow::bail!("{dir} no longer exists, so {program} cannot be resumed there");
        }
        command.current_dir(dir);
    }

    // Dim, on stderr, so it is visible without polluting the stdout contract. NO_COLOR is
    // honoured here as everywhere else — this line is outside the TUI, not outside the rule.
    let line = argv.join(" ");
    if std::env::var_os("NO_COLOR").is_some() {
        eprintln!("{line}");
    } else {
        eprintln!("\x1b[2m{line}\x1b[0m");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Only returns on failure; on success this process no longer exists, which is what
        // leaves no orphaned parent between the shell and the agent.
        let err = command.exec();
        Err(anyhow::Error::from(err)).with_context(|| format!("running {program}"))
    }
    #[cfg(not(unix))]
    {
        // No exec on Windows, so the parent stays alive and waits rather than exiting and
        // handing the terminal back while the agent is still drawing on it.
        let status = command.status().with_context(|| format!("running {program}"))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}
