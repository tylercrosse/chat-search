//! Wiring for `cs tui`.
//!
//! Everything config-shaped lives here so `cs-tui` does not have to know it (docs/TUI-DESIGN.md
//! §1): this resolves the index path and the query log, and hands the TUI a sink. The TUI
//! resolves neither, which is what keeps it off `cs-archive`.

use anyhow::Result;
use cs_archive::Config;
use std::path::{Path, PathBuf};

pub fn run(
    config_path: &Path,
    db_path: Option<PathBuf>,
    source: Option<&str>,
    limit: i64,
) -> Result<()> {
    let cfg = Config::load(config_path)?;
    let db_path = db_path.unwrap_or_else(|| cfg.default_db());

    // Captured by the sink rather than read per event: the TUI can emit while the terminal
    // is still in raw mode, and re-reading the config there would be a filesystem hit inside
    // a keystroke path.
    let log_path = cfg.query_log();
    let log_queries = cfg.log_queries;
    let mut sink = |event: cs_core::querylog::Event| {
        // Same rule as everywhere else this is called: losing a log line costs a data point,
        // failing the action costs the thing that was asked for.
        if log_queries {
            let _ = cs_core::querylog::append(&log_path, &event);
        }
    };

    // Unlike `cs pick`, the TUI does not recompute the result list to find the rank — it
    // built the list, so it knows where the selection sat and emits the event itself.
    let opts = cs_tui::Opts {
        query: String::new(),
        source: source.map(String::from),
        limit,
    };

    match cs_tui::run(db_path, &mut sink, opts)? {
        cs_tui::Exit::Quit => Ok(()),
        // Printed rather than executed, so a shell wrapper decides whether to `eval` it.
        // Executing here would strand the user in a subprocess of a TUI they just left.
        cs_tui::Exit::Open { resume_cmd, cwd, .. } => {
            if let Some(cmd) = resume_cmd {
                match cwd.filter(|d| !d.is_empty()) {
                    Some(dir) => println!("cd {dir} && {cmd}"),
                    None => println!("{cmd}"),
                }
            }
            Ok(())
        }
    }
}
