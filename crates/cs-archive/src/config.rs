use crate::error::{Error, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How a source's files are laid out in the archive (ADR 18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// One archived file per source file. Correct when the source already writes
    /// one file per thread — Codex, Claude Code, Gemini.
    #[default]
    Mirror,
    /// Many small fragments packed into one append-only JSONL per conversation.
    /// For sources like OpenCode with 170k files. Not implemented yet.
    Bundle,
}

/// One watched location (ADR 16). `id` is permanent: it becomes part of every
/// conversation id, so it must never change once the archive holds data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    pub path: PathBuf,
    #[serde(default)]
    pub layout: Layout,
    /// Glob patterns, relative to `path`. Empty means "every file".
    #[serde(default)]
    pub include: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub archive_root: PathBuf,
    /// Overrides the auto-derived machine slug (ADR 17). Cosmetic only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_alias: Option<String>,
    /// Record what was searched for and what was opened, into [`Config::query_log`].
    ///
    /// On by default: the log never leaves the machine, and it is the only source of real
    /// information needs this project has. An eval set of invented queries measures invented
    /// needs, so switching this off costs the ranking its ground truth.
    #[serde(default = "yes")]
    pub log_queries: bool,
    #[serde(default)]
    pub sources: Vec<Source>,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            archive_root: expand_tilde("~/.chat-archive"),
            machine_alias: None,
            log_queries: true,
            sources: detect_sources(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).at(path)?;
        let mut cfg: Config = toml::from_str(&text)
            .map_err(|source| Error::ConfigParse { path: path.to_path_buf(), source })?;
        cfg.archive_root = expand_path(&cfg.archive_root);
        for s in &mut cfg.sources {
            s.path = expand_path(&s.path);
        }
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?).at(path)
    }

    /// Source ids end up embedded in conversation ids forever, so reject anything
    /// that would be awkward there before it reaches the archive.
    fn validate(&self) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        for s in &self.sources {
            let ok = !s.id.is_empty()
                && s.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
            if !ok {
                return Err(Error::BadSourceId(s.id.clone()));
            }
            if !seen.insert(&s.id) {
                return Err(Error::DuplicateSourceId(s.id.clone()));
            }
        }
        Ok(())
    }

    pub fn default_path() -> PathBuf {
        expand_tilde("~/.config/chat-search/config.toml")
    }

    /// Where the search index lives by default. Sits beside the archive but is disposable —
    /// deleting it costs a rebuild, never data (ADR 1).
    ///
    /// Path resolution lives here rather than in cs-core because it is a question about the
    /// config, and cs-core knows nothing about configs: pointing it at one would run the
    /// dependency backwards and drag toml and the globs into the search crate. Every client
    /// already loads a `Config` to find the archive, so this costs them nothing.
    pub fn default_db(&self) -> PathBuf {
        self.archive_root.join("index.db")
    }

    /// Where searches and selections are recorded.
    ///
    /// Beside `index.db` but the opposite kind of file: the index is a pure function of the
    /// archive and can be deleted at will, while this cannot be reconstructed from anything
    /// and should be backed up. Kept outside `raw/` so it never looks like captured source
    /// data, and it is the first thing here that will belong in `library.db` once that
    /// exists (chat-search-6eb.14).
    pub fn query_log(&self) -> PathBuf {
        self.archive_root.join("queries.jsonl")
    }

    /// Whether this run should record what it searched for.
    ///
    /// The config setting, unless `CS_LOG_QUERIES` overrides it. The override exists because
    /// benchmarks share the log with real searching, and a driven run pollutes it in a way no
    /// later rule can undo: a query typed to measure latency looks exactly like one typed to
    /// find something, right down to going unpicked. `CS_LOG_QUERIES=0` wraps a whole
    /// measurement session, subprocesses included, which is what `--config` pointing at a
    /// scratch file does not.
    ///
    /// A convenience rather than the mechanism, though. Forgetting it stays recoverable —
    /// `cs_core::querylog::Event::Driven` says afterwards what should not have been recorded.
    pub fn recording_queries(&self) -> bool {
        recording_queries_with(self.log_queries, std::env::var("CS_LOG_QUERIES").ok().as_deref())
    }
}

/// [`Config::recording_queries`] against an explicit environment.
///
/// Split out for the same reason [`expand_tilde_with`] is: the rule is worth testing, and
/// setting a process-global variable to test it would race every other test in the binary.
///
/// Off wins ties. An unrecognised value reads as "somebody meant to turn this off and
/// mistyped it", which loses a few data points; the other reading loses a benchmark into the
/// log, and that is the failure this whole flag exists to prevent.
fn recording_queries_with(configured: bool, env: Option<&str>) -> bool {
    match env {
        None => configured,
        Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
    }
}

/// Every location this project knows how to watch, in the order they should be archived,
/// whether or not one exists on this machine.
///
/// Split out from [`detect_sources`] because the *unfiltered* list is what [`crate::drift`]
/// diffs the config against on every `cs archive`. Filtering by existence at `cs init` and
/// never looking again is precisely the bug in chat-search-a7k.12: a tool adopted after init
/// was never archived and nothing said so.
pub fn candidate_sources() -> Vec<Source> {
    let candidates = [
        // Codex zstd-compresses rollouts older than ~7 days in a background worker, so
        // the glob must survive `.jsonl` becoming `.jsonl.zst` or everything older than a
        // week stops being captured. The archiver stores either verbatim; decompression is
        // the importer's problem, not capture's.
        ("codex", "~/.codex/sessions", Layout::Mirror, vec!["**/rollout-*.jsonl", "**/rollout-*.jsonl.zst"]),
        ("claude-code", "~/.claude/projects", Layout::Mirror, vec!["**/*.jsonl"]),
        // `~/.gemini/tmp/<projectHash>/` holds three JSON shapes and only one is a
        // conversation. Sweeping all of them archived 175.5 MiB of `checkpoints/` — tool-write
        // snapshots taken before every `write_file`, six of them ~29 MiB apiece — to reach
        // 324 KiB of chats, which is 7.3% of this whole archive for 0.07% of the index
        // (chat-search-aig, measured 2026-08-01). `logs.json` is small and left out for the
        // other reason: it is a prompt log the importer already declines as redundant with the
        // chats beside it (chat-search-n58.22 is where that would be revisited).
        ("gemini-cli", "~/.gemini/tmp", Layout::Mirror, vec!["**/chats/*.json"]),
        // Claude Desktop's Cowork and Chat tabs. One session is two files — `local_<uuid>.json`
        // for the title and model, `local_<uuid>/audit.jsonl` for the messages — so both
        // patterns are needed or half of every conversation goes missing. The `local_*.json`
        // glob also reaches plugin manifests nested inside a session's working directory; the
        // importer declines those by shape.
        (
            "claude-desktop",
            "~/Library/Application Support/Claude/local-agent-mode-sessions",
            Layout::Mirror,
            vec!["**/local_*.json", "**/audit.jsonl"],
        ),
        // GitHub Copilot CLI. Unlike the sources above it writes a small directory tree per
        // session — `workspace.yaml`, `checkpoints/*.md`, `research/`, `files/` — rather than
        // one transcript file, so the include list has to name three extensions to catch a
        // whole session. 9 files / 44 KB on this machine on 2026-07-30; the reason it is here
        // is not that volume but that it accumulated entirely unwatched after `cs init`.
        ("copilot-cli", "~/.copilot/session-state", Layout::Mirror, vec!["**/*.yaml", "**/*.md", "**/*.json"]),
        // Bundle layout is not implemented yet; listed so the id is reserved.
        ("opencode", "~/.local/share/opencode/storage", Layout::Bundle, vec!["**/*.json"]),
    ];
    candidates
        .into_iter()
        .map(|(id, path, layout, include)| Source {
            id: id.to_string(),
            path: expand_tilde(path),
            layout,
            include: include.into_iter().map(String::from).collect(),
        })
        .collect()
}

/// The candidates that exist here, so a fresh config from `cs init` is immediately usable.
pub fn detect_sources() -> Vec<Source> {
    candidate_sources().into_iter().filter(|s| s.path.is_dir()).collect()
}

fn expand_path(p: &Path) -> PathBuf {
    expand_tilde(&p.to_string_lossy())
}

/// `~` is not expanded by the shell when a path comes from a config file.
pub fn expand_tilde(s: &str) -> PathBuf {
    expand_tilde_with(s, std::env::var_os("HOME").map(PathBuf::from))
}

/// Split out so it can be tested without mutating process-global `HOME`, which would
/// race against every other test in the binary.
fn expand_tilde_with(s: &str, home: Option<PathBuf>) -> PathBuf {
    match (s.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_can_switch_query_logging_off_for_a_driven_run() {
        // The lever a benchmark reaches for, so its queries never become information needs.
        for off in ["0", "false", "no", "off", "OFF", " 0 "] {
            assert!(!recording_queries_with(true, Some(off)), "{off:?}");
        }
        for on in ["1", "true", "yes", "on", "ON"] {
            assert!(recording_queries_with(false, Some(on)), "{on:?}");
        }
        assert!(recording_queries_with(true, None), "unset leaves the config in charge");
        assert!(!recording_queries_with(false, None));
    }

    #[test]
    fn a_mistyped_override_stops_logging_rather_than_starting_it() {
        // Off wins ties deliberately. Reading `fasle` as "on" loses a whole benchmark into the
        // log, which is the failure the flag exists to prevent; reading it as "off" loses a
        // handful of real searches, which is recoverable by searching again.
        assert!(!recording_queries_with(true, Some("fasle")));
        assert!(!recording_queries_with(true, Some("")));
    }

    #[test]
    fn the_gemini_glob_reaches_the_chats_and_not_the_checkpoints_beside_them() {
        // Measured on the live archive: 34 files captured, of which 15 chats were 324 KiB and
        // 15 checkpoints were 175.5 MiB (chat-search-aig). `gemini.rs` declines both other
        // shapes by their contents, but capture runs long before any importer does, so
        // declining to parse a file is not the same as declining to pay for it.
        let gemini = candidate_sources().into_iter().find(|s| s.id == "gemini-cli").unwrap();
        let m = crate::scan::build_matcher(&gemini.include).unwrap();

        assert!(m.is_match("2df4271300f9be6046dd0db74eff4d3d/chats/session-2026-01-14T08-28-ab7e9dcf.json"));
        // Not every project directory is a hash: Gemini names one after the workspace when it
        // can, and a glob anchored on hex would capture some sessions and not others.
        assert!(m.is_match("personal-site/chats/session-2025-10-19T21-38-fb65e7fe.json"));

        assert!(
            !m.is_match("de716be395009fe9d077d68154f2a399a/checkpoints/2026-01-31T08-53-53_544Z-tests.py-write_file.json"),
            "the 29 MiB tool-write snapshots are the whole point of narrowing this"
        );
        assert!(!m.is_match("personal-site/logs.json"), "a prompt log, not a conversation");
    }

    #[test]
    fn rejects_ids_that_would_pollute_conversation_ids() {
        let bad = |id: &str| Config {
            archive_root: PathBuf::from("/tmp/a"),
            machine_alias: None,
            log_queries: true,
            sources: vec![Source {
                id: id.into(),
                path: PathBuf::from("/tmp/s"),
                layout: Layout::Mirror,
                include: vec![],
            }],
        }
        .validate();

        assert!(bad("codex").is_ok());
        assert!(bad("claude-code").is_ok());
        assert!(bad("Codex").is_err(), "uppercase would break id comparisons");
        assert!(bad("codex:cli").is_err(), "colon is the id separator");
        assert!(bad("").is_err());
    }

    #[test]
    fn rejects_duplicate_ids() {
        let src = |id: &str| Source {
            id: id.into(),
            path: PathBuf::from("/tmp/s"),
            layout: Layout::Mirror,
            include: vec![],
        };
        let cfg = Config {
            archive_root: PathBuf::from("/tmp/a"),
            machine_alias: None,
            log_queries: true,
            sources: vec![src("codex"), src("codex")],
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn the_index_resolves_against_the_expanded_archive_root() {
        // Resolution happens after `load` expanded `~`, so a client that only ever sees the
        // loaded config gets an openable path rather than a literal tilde.
        let cfg = Config {
            archive_root: expand_tilde_with("~/.chat-archive", Some(PathBuf::from("/Users/test"))),
            machine_alias: None,
            log_queries: true,
            sources: vec![],
        };
        assert_eq!(cfg.default_db(), PathBuf::from("/Users/test/.chat-archive/index.db"));
    }

    #[test]
    fn tilde_expands_only_at_the_front() {
        let home = Some(PathBuf::from("/Users/test"));
        assert_eq!(expand_tilde_with("~/.codex", home.clone()), PathBuf::from("/Users/test/.codex"));
        assert_eq!(expand_tilde_with("/abs/~/x", home.clone()), PathBuf::from("/abs/~/x"));
        assert_eq!(expand_tilde_with("~notauser/x", home), PathBuf::from("~notauser/x"));
        // no HOME: leave the path alone rather than silently rooting it at /
        assert_eq!(expand_tilde_with("~/.codex", None), PathBuf::from("~/.codex"));
    }
}
