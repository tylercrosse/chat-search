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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default)]
    pub sources: Vec<Source>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            archive_root: expand_tilde("~/.chat-archive"),
            machine_alias: None,
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
}

/// Known source locations, in the order they should be archived. Only those that
/// actually exist on this machine are emitted, so a fresh config is immediately usable.
pub fn detect_sources() -> Vec<Source> {
    let candidates = [
        // Codex zstd-compresses rollouts older than ~7 days in a background worker, so
        // the glob must survive `.jsonl` becoming `.jsonl.zst` or everything older than a
        // week stops being captured. The archiver stores either verbatim; decompression is
        // the importer's problem, not capture's.
        ("codex", "~/.codex/sessions", Layout::Mirror, vec!["**/rollout-*.jsonl", "**/rollout-*.jsonl.zst"]),
        ("claude-code", "~/.claude/projects", Layout::Mirror, vec!["**/*.jsonl"]),
        ("gemini-cli", "~/.gemini/tmp", Layout::Mirror, vec!["**/*.json"]),
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
        .filter(|s| s.path.is_dir())
        .collect()
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
    fn rejects_ids_that_would_pollute_conversation_ids() {
        let bad = |id: &str| Config {
            archive_root: PathBuf::from("/tmp/a"),
            machine_alias: None,
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
