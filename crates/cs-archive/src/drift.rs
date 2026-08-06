//! Has the config stopped describing this machine? (chat-search-a7k.12)
//!
//! [`crate::config::detect_sources`] used to run in exactly one place — `Config::default`,
//! reached only by `cs init`, which refuses to run a second time. So the watched set was
//! frozen at whatever was installed the day the config was written. Adopt an agent
//! afterwards and it is never captured; uninstall or move one and capture stops. Both are
//! silent, and both are unrecoverable: this project has already lost transcripts to the
//! Claude Code 30-day prune, which deletes on a schedule regardless of whether anything was
//! watching. The only signal today is noticing months later that the conversations are not
//! searchable.
//!
//! So detection re-runs on every `cs archive`. Nothing is adopted automatically: a source id
//! is permanent once it enters the archive (ADR 16), so choosing one stays a config edit.

use crate::config::{candidate_sources, Source};
use crate::error::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The gap between what the config watches and what is on this disk.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Known locations that exist here but match no `[[sources]]` entry — an agent adopted
    /// after `cs init`, whose conversations are accruing uncaptured right now.
    pub unconfigured: Vec<Source>,
    /// Configured sources whose directory is gone: uninstalled, moved, or an external volume
    /// that is not mounted. What was already archived is safe — the bead's own note records
    /// `chatgpt-export` sitting complete in the archive with 2,011 conversations while the
    /// config had lost the entry — but nothing new is arriving.
    pub missing: Vec<Source>,
}

impl Drift {
    pub fn is_empty(&self) -> bool {
        self.unconfigured.is_empty() && self.missing.is_empty()
    }

    /// Identity of *what* is being reported, deliberately not of when. The report is
    /// throttled on this string, so it has to change exactly when the printed lines would
    /// change and not otherwise.
    fn fingerprint(&self) -> String {
        let ids = |v: &[Source]| v.iter().map(|s| s.id.clone()).collect::<Vec<_>>().join(",");
        format!("u:{}|m:{}", ids(&self.unconfigured), ids(&self.missing))
    }

    /// `[[sources]]` blocks for the unconfigured candidates, ready to paste.
    ///
    /// Adoption stays a config edit by design (ADR 16), but a permanent decision should not
    /// also be a research task: the include globs here are the ones this crate already knows
    /// are right for that layout, and getting them wrong is silent partial capture.
    pub fn adoption_toml(&self) -> String {
        #[derive(Serialize)]
        struct Doc<'a> {
            sources: &'a [Source],
        }
        toml::to_string_pretty(&Doc { sources: &self.unconfigured }).unwrap_or_default()
    }
}

/// Re-detect and diff against the configured sources.
///
/// One `stat` per candidate plus one per configured source — about a dozen syscalls, run
/// immediately before a scan that walks tens of thousands of files. Nothing here is worth
/// caching for the 300 s launchd cadence.
pub fn detect(configured: &[Source]) -> Drift {
    diff(&candidate_sources(), configured, |p| p.is_dir())
}

/// The diff itself, over supplied inputs.
///
/// Split from [`detect`] for the same reason `expand_tilde_with` is split from
/// `expand_tilde`: a test that asked the real filesystem would be asserting on whichever
/// agents happen to be installed on the machine running it, so it would pass here and fail
/// in CI and on anyone else's laptop.
pub fn diff(candidates: &[Source], configured: &[Source], exists: impl Fn(&Path) -> bool) -> Drift {
    let ids: HashSet<&str> = configured.iter().map(|s| s.id.as_str()).collect();
    let paths: HashSet<&Path> = configured.iter().map(|s| s.path.as_path()).collect();

    Drift {
        unconfigured: candidates
            .iter()
            // Either match counts as configured. A config that kept the id but repointed the
            // path has made a deliberate choice; one that kept the path under a different id
            // is already capturing the bytes, and reporting it would invite a second id for
            // the same location — which ADR 16 makes permanent and unmergeable.
            .filter(|c| !ids.contains(c.id.as_str()) && !paths.contains(c.path.as_path()))
            .filter(|c| exists(&c.path))
            .cloned()
            .collect(),
        missing: configured.iter().filter(|s| !exists(&s.path)).cloned().collect(),
    }
}

/// Beside `.machine.json` and dotted for the same reason: machine-local bookkeeping, not
/// captured data, and never synced (ADR 11 syncs raw bytes only).
fn state_path(machine_dir: &Path) -> PathBuf {
    machine_dir.join(".drift.json")
}

/// Whether to print this report now, recording the answer.
///
/// Any *change* to the set prints immediately — a candidate appearing, one being adopted, a
/// source directory vanishing are all events. An unchanged set repeats once per
/// [`crate::throttle::REPEAT_AFTER_MS`]. An empty set clears the state, so a candidate that
/// comes back after being resolved is an event again rather than waiting out a stale window.
pub fn claim(machine_dir: &Path, drift: &Drift, now_ms: u64) -> Result<bool> {
    let fingerprint = (!drift.is_empty()).then(|| drift.fingerprint());
    crate::throttle::claim(&state_path(machine_dir), fingerprint.as_deref(), now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Layout;

    fn src(id: &str, path: &str) -> Source {
        Source {
            id: id.into(),
            path: PathBuf::from(path),
            layout: Layout::Mirror,
            include: vec!["**/*.jsonl".into()],
        }
    }

    /// Everything under `/live` is on disk; everything else is not.
    fn on_disk(p: &Path) -> bool {
        p.starts_with("/live")
    }

    #[test]
    fn a_configured_candidate_is_not_a_candidate() {
        let candidates = [src("codex", "/live/codex")];
        let configured = [src("codex", "/live/codex")];
        assert_eq!(diff(&candidates, &configured, on_disk), Drift::default());
    }

    #[test]
    fn a_repointed_or_renamed_source_still_counts_as_configured() {
        // Both halves matter: matching on id alone would re-offer a location the user moved,
        // and matching on path alone would re-offer one they renamed. Either would end in two
        // permanent ids for one directory (ADR 16).
        let candidates = [src("codex", "/live/codex")];
        assert!(diff(&candidates, &[src("codex", "/live/elsewhere")], on_disk).unconfigured.is_empty());
        assert!(diff(&candidates, &[src("codex-work", "/live/codex")], on_disk).unconfigured.is_empty());
    }

    #[test]
    fn a_present_but_unconfigured_candidate_is_reported() {
        // The live case: `~/.copilot/session-state` exists, `cs init` ran before it did.
        let candidates = [src("codex", "/live/codex"), src("copilot-cli", "/live/copilot")];
        let drift = diff(&candidates, &[src("codex", "/live/codex")], on_disk);
        assert_eq!(drift.unconfigured, vec![src("copilot-cli", "/live/copilot")]);
        assert!(drift.missing.is_empty());
    }

    #[test]
    fn a_candidate_that_is_not_installed_is_not_reported() {
        // Otherwise every run would nag about every agent the user has chosen not to run.
        let candidates = [src("opencode", "/gone/opencode")];
        assert!(diff(&candidates, &[], on_disk).is_empty());
    }

    #[test]
    fn a_configured_source_whose_directory_vanished_is_reported() {
        let configured = [src("codex", "/live/codex"), src("gemini-cli", "/gone/gemini")];
        let drift = diff(&[], &configured, on_disk);
        assert_eq!(drift.missing, vec![src("gemini-cli", "/gone/gemini")]);
        assert!(drift.unconfigured.is_empty());
    }

    #[test]
    fn nothing_to_report_when_there_are_no_candidates() {
        assert!(diff(&[], &[], on_disk).is_empty());
        assert!(diff(&[], &[src("codex", "/live/codex")], on_disk).is_empty());
    }

    #[test]
    fn both_halves_are_reported_from_one_pass() {
        let candidates = [src("copilot-cli", "/live/copilot")];
        let configured = [src("codex", "/live/codex"), src("gemini-cli", "/gone/gemini")];
        let drift = diff(&candidates, &configured, on_disk);
        assert_eq!(drift.unconfigured.len(), 1);
        assert_eq!(drift.missing.len(), 1);
    }

    #[test]
    fn the_paste_block_is_a_usable_config_fragment() {
        let drift = diff(&[src("copilot-cli", "/live/copilot")], &[], on_disk);
        let toml = drift.adoption_toml();
        assert!(toml.contains("[[sources]]"), "{toml}");
        assert!(toml.contains(r#"id = "copilot-cli""#), "{toml}");
        // Round-trips into the real config shape, so pasting it cannot produce a config that
        // `Config::load` then rejects.
        let parsed: crate::Config =
            toml::from_str(&format!("archive_root = \"/tmp/a\"\n{toml}")).unwrap();
        assert_eq!(parsed.sources, drift.unconfigured);
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs-drift-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_same_report_does_not_print_on_every_run() {
        let dir = tmpdir();
        let drift = diff(&[src("copilot-cli", "/live/copilot")], &[], on_disk);
        assert!(claim(&dir, &drift, 0).unwrap(), "first sighting is news");
        // 288 runs at 300 s is one launchd day, and the 288th lands exactly on the window.
        for i in 1..288 {
            assert!(!claim(&dir, &drift, i * 300_000).unwrap(), "run {i} printed again");
        }
        assert!(claim(&dir, &drift, 288 * 300_000).unwrap(), "a day on, say it again");
        assert_eq!(288 * 300_000, crate::throttle::REPEAT_AFTER_MS);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_changed_report_prints_immediately() {
        let dir = tmpdir();
        let one = diff(&[src("copilot-cli", "/live/copilot")], &[], on_disk);
        let two = diff(
            &[src("copilot-cli", "/live/copilot"), src("codex", "/live/codex")],
            &[],
            on_disk,
        );
        assert!(claim(&dir, &one, 0).unwrap());
        assert!(!claim(&dir, &one, 300_000).unwrap());
        assert!(claim(&dir, &two, 600_000).unwrap(), "a new candidate must not wait a day");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolving_the_drift_resets_the_throttle() {
        let dir = tmpdir();
        let drift = diff(&[src("copilot-cli", "/live/copilot")], &[], on_disk);
        assert!(claim(&dir, &drift, 0).unwrap());
        assert!(!claim(&dir, &Drift::default(), 300_000).unwrap(), "nothing to say");
        // Adopted then removed from the config again: an event, not a repeat.
        assert!(claim(&dir, &drift, 600_000).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_throttle_file_reports_rather_than_swallows() {
        let dir = tmpdir();
        std::fs::write(state_path(&dir), "{ not json").unwrap();
        let drift = diff(&[src("copilot-cli", "/live/copilot")], &[], on_disk);
        assert!(claim(&dir, &drift, 0).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }
}
