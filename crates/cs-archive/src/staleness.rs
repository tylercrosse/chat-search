//! Has a manual export stopped happening? (chat-search-a7k.10)
//!
//! Most sources are directories a running tool writes into: leave them alone and they still
//! accrue. Two are not. `chatgpt-export` exists because ChatGPT's local store is AES-encrypted
//! behind a Keychain access group, and `google-takeout` because Gemini's web history has no API
//! at all — so for both, the only route in is a human asking the vendor for a zip and unpacking
//! it into a watched directory (ADR 21).
//!
//! Nothing arrives between those acts, and the gap they leave is unrecoverable: no later export
//! backfills a conversation the vendor has since dropped. The archive on this machine is the
//! worked example — the ChatGPT export ended 2026-07-10 and nothing said so for three weeks.
//!
//! So the age of the newest bytes held per export-shaped source is measured on every archive
//! run, and reported once it passes a week. ADR 21 also leans on this from the other side: when
//! a claude.ai fetcher eventually lands, a silently dead one has to surface as a *stale* source
//! rather than as an empty one, and it will, because it is export-shaped by the rule below.

use crate::config::{candidate_sources, Source};
use crate::error::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// When an export stops being fresh enough to trust.
///
/// A judgement call rather than a measurement: short enough that a missed month cannot
/// accumulate unnoticed, and the line is cheap enough that a false nag costs nothing.
pub const STALE_AFTER_DAYS: u64 = 7;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// One export-shaped source whose newest archived bytes are older than [`STALE_AFTER_DAYS`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stale {
    pub source: String,
    pub days: u64,
}

/// The configured sources that no running tool writes into.
///
/// Derived rather than listed, so a third export source is covered without editing this check.
/// [`candidate_sources`] is this project's register of locations a tool owns — ARCHITECTURE.md
/// puts it plainly: "Every id in the archiver's candidate list is a directory some running
/// agent writes to ... An export is not written by anything — it is mailed, downloaded and
/// unpacked wherever you happen to put it." Absence from that list *is* export-shapedness, and
/// it is why `chatgpt-export` and `google-takeout` are not in it.
///
/// Matched on id *or* path, for the reason [`crate::drift::diff`] matches on both: a config
/// that repointed `codex` elsewhere, or that watches `~/.codex/sessions` under a second id, is
/// still watching a live tool and must not be told to go re-export it.
fn export_shaped<'a>(candidates: &[Source], configured: &'a [Source]) -> Vec<&'a Source> {
    let ids: HashSet<&str> = candidates.iter().map(|s| s.id.as_str()).collect();
    let paths: HashSet<&Path> = candidates.iter().map(|s| s.path.as_path()).collect();
    configured
        .iter()
        .filter(|s| !ids.contains(s.id.as_str()) && !paths.contains(s.path.as_path()))
        .collect()
}

/// Which export-shaped sources have gone stale, worst first.
pub fn detect(
    configured: &[Source],
    newest_ms: impl Fn(&str) -> Option<u64>,
    now_ms: u64,
) -> Vec<Stale> {
    check(&candidate_sources(), configured, newest_ms, now_ms)
}

/// The check itself, over supplied inputs.
///
/// Split from [`detect`] for the same reason `drift::diff` is split from `drift::detect`: a
/// test that asked the real candidate list and the real clock would be asserting on today's
/// date and on which agents happen to be installed, so it would pass here and fail tomorrow.
pub fn check(
    candidates: &[Source],
    configured: &[Source],
    newest_ms: impl Fn(&str) -> Option<u64>,
    now_ms: u64,
) -> Vec<Stale> {
    let mut stale: Vec<Stale> = export_shaped(candidates, configured)
        .into_iter()
        .filter_map(|s| {
            // A source the archive holds nothing for has no age to report. That is not the
            // same as fresh, and it is deliberately not silence either: `cs index` already
            // prints "nothing archived — run `cs archive`" for it. Inventing a day count for
            // bytes that were never captured is the one thing this check must not do.
            let newest = newest_ms(&s.id)?;
            let days = now_ms.saturating_sub(newest) / DAY_MS;
            (days > STALE_AFTER_DAYS).then(|| Stale { source: s.id.clone(), days })
        })
        .collect();
    // Worst first, then by id so the order — and therefore the throttle fingerprint — does not
    // depend on how the config happened to be written.
    stale.sort_by(|a, b| b.days.cmp(&a.days).then_with(|| a.source.cmp(&b.source)));
    stale
}

/// Beside `.drift.json` and dotted for the same reason: machine-local bookkeeping, not captured
/// data, and never synced (ADR 11 syncs raw bytes only).
fn state_path(machine_dir: &Path) -> PathBuf {
    machine_dir.join(".staleness.json")
}

/// Whether to print this nag now, recording the answer.
///
/// Identity is the set of stale source ids and not their day counts. Folding the counts in
/// would make every source's midnight an event and reset the window with it, which on two
/// export sources ticking over at different times would print far more than once a day for no
/// new information. The counts still stay current: they advance by one per day and the window
/// is a day, so each printing carries the number it was true for.
pub fn claim(machine_dir: &Path, stale: &[Stale], now_ms: u64) -> Result<bool> {
    let fingerprint = (!stale.is_empty())
        .then(|| stale.iter().map(|s| s.source.as_str()).collect::<Vec<_>>().join(","));
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

    /// The live tool locations this project knows, standing in for `candidate_sources()`.
    fn candidates() -> Vec<Source> {
        vec![src("codex", "/home/.codex/sessions"), src("gemini-cli", "/home/.gemini/tmp")]
    }

    const NOW: u64 = 1_000 * DAY_MS;

    fn days_ago(n: u64) -> u64 {
        NOW - n * DAY_MS
    }

    /// Every source reports the same age, so only the classification is under test.
    fn aged(days: u64) -> impl Fn(&str) -> Option<u64> {
        move |_| Some(days_ago(days))
    }

    #[test]
    fn an_export_that_stopped_is_reported_with_its_age() {
        let configured = [src("chatgpt-export", "/home/exports")];
        assert_eq!(
            check(&candidates(), &configured, aged(25), NOW),
            vec![Stale { source: "chatgpt-export".into(), days: 25 }]
        );
    }

    #[test]
    fn a_watched_tool_directory_is_never_nagged_to_re_export() {
        // The case that decides the whole design. `gemini-cli` on this machine last wrote in
        // February — 161 days — because Gemini's CLI simply is not used. Nagging about it would
        // be telling the user to export a source that has no export, every day, forever, which
        // is how the one line that mattered would stop being read.
        let configured = [src("codex", "/home/.codex/sessions"), src("gemini-cli", "/home/.gemini/tmp")];
        assert_eq!(check(&candidates(), &configured, aged(161), NOW), vec![]);
    }

    #[test]
    fn a_fresh_export_is_silent() {
        let configured = [src("chatgpt-export", "/home/exports")];
        assert_eq!(check(&candidates(), &configured, aged(0), NOW), vec![]);
    }

    #[test]
    fn the_threshold_is_more_than_seven_days_not_seven() {
        let configured = [src("chatgpt-export", "/home/exports")];
        assert_eq!(STALE_AFTER_DAYS, 7);
        assert!(check(&candidates(), &configured, aged(7), NOW).is_empty(), "a week is still fine");
        assert_eq!(check(&candidates(), &configured, aged(8), NOW).len(), 1);
    }

    #[test]
    fn a_repointed_or_renamed_tool_source_is_still_a_tool_source() {
        // Both halves matter for the same reason they do in `drift::diff`. Matching on id alone
        // would nag about a `codex` the user moved to an external volume; matching on path alone
        // would nag about `~/.codex/sessions` watched under a second id for a work account.
        // Neither is an export, and neither has anything to re-run.
        let moved = [src("codex", "/volumes/big/codex")];
        assert!(check(&candidates(), &moved, aged(30), NOW).is_empty(), "repointed by id");
        let renamed = [src("codex-work", "/home/.codex/sessions")];
        assert!(check(&candidates(), &renamed, aged(30), NOW).is_empty(), "renamed by path");
    }

    #[test]
    fn a_source_the_archive_holds_nothing_for_has_no_age_to_report() {
        // Configured today, never exported into. There is no measurement to make, and the check
        // must not manufacture one — "nothing archived" is `cs index`'s line, not this one.
        let configured = [src("google-takeout", "/home/exports")];
        assert_eq!(check(&candidates(), &configured, |_| None, NOW), vec![]);
    }

    #[test]
    fn a_third_export_source_is_covered_without_editing_this_check() {
        // The whole point of deriving rather than listing: the claude.ai fetcher ADR 21 leaves
        // for later needs no change here to be watched, and a dead one surfaces as stale rather
        // than as empty.
        let configured = [
            src("chatgpt-export", "/home/exports"),
            src("google-takeout", "/home/exports"),
            src("claude-ai", "/home/fetched"),
        ];
        let stale = check(&candidates(), &configured, aged(9), NOW);
        let ids: Vec<&str> = stale.iter().map(|s| s.source.as_str()).collect();
        assert_eq!(ids, vec!["chatgpt-export", "claude-ai", "google-takeout"]);
    }

    #[test]
    fn the_worst_offender_is_named_first() {
        let configured =
            [src("chatgpt-export", "/home/exports"), src("google-takeout", "/home/takeout")];
        let ages = |id: &str| Some(days_ago(if id == "google-takeout" { 40 } else { 10 }));
        let stale = check(&candidates(), &configured, ages, NOW);
        assert_eq!(stale[0], Stale { source: "google-takeout".into(), days: 40 });
        assert_eq!(stale[1], Stale { source: "chatgpt-export".into(), days: 10 });
    }

    #[test]
    fn the_real_candidate_list_classifies_the_two_export_sources_it_ships_with() {
        // `check` is where the logic is tested; this pins the one input that is a fact about
        // this project rather than about the algorithm. If `chatgpt-export` were ever added to
        // `candidate_sources()`, the nag would silently stop and every test above would pass.
        let configured = [
            src("chatgpt-export", "/home/exports"),
            src("google-takeout", "/home/exports"),
            src("codex", "/home/.codex/sessions"),
        ];
        let ids: Vec<String> =
            detect(&configured, aged(30), NOW).into_iter().map(|s| s.source).collect();
        assert_eq!(ids, vec!["chatgpt-export", "google-takeout"]);
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs-stale-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_same_nag_does_not_print_on_every_run() {
        let dir = tmpdir();
        let stale = vec![Stale { source: "chatgpt-export".into(), days: 25 }];
        assert!(claim(&dir, &stale, 0).unwrap(), "first sighting is news");
        for i in 1..288 {
            assert!(!claim(&dir, &stale, i * 300_000).unwrap(), "run {i} nagged again");
        }
        assert!(claim(&dir, &stale, crate::throttle::REPEAT_AFTER_MS).unwrap(), "a day on, again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_second_source_going_stale_does_not_wait_a_day_to_be_mentioned() {
        let dir = tmpdir();
        let one = vec![Stale { source: "chatgpt-export".into(), days: 25 }];
        let two = vec![
            Stale { source: "chatgpt-export".into(), days: 25 },
            Stale { source: "google-takeout".into(), days: 8 },
        ];
        assert!(claim(&dir, &one, 0).unwrap());
        assert!(!claim(&dir, &one, 300_000).unwrap());
        assert!(claim(&dir, &two, 600_000).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exporting_resets_the_throttle_so_falling_behind_again_is_news() {
        let dir = tmpdir();
        let stale = vec![Stale { source: "chatgpt-export".into(), days: 25 }];
        assert!(claim(&dir, &stale, 0).unwrap());
        assert!(!claim(&dir, &[], 300_000).unwrap(), "exported: nothing to say");
        assert!(claim(&dir, &stale, 600_000).unwrap(), "a week later it is news again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_day_count_advancing_does_not_by_itself_start_a_new_window() {
        // Identity is the set of ids, not the counts. Two export sources crossing midnight at
        // different times must not print twice for one day's worth of news.
        let dir = tmpdir();
        assert!(claim(&dir, &[Stale { source: "chatgpt-export".into(), days: 25 }], 0).unwrap());
        assert!(
            !claim(&dir, &[Stale { source: "chatgpt-export".into(), days: 26 }], 300_000).unwrap()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
