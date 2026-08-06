//! What overwriting the config would delete (chat-search-a7k.30).
//!
//! `cs init --force` built a `Config::default()` and saved it over whatever was already there.
//! Default means [`crate::config::detect_sources`], which is `path.is_dir()` over a list of live
//! agent directories — so every `[[sources]]` block a person wrote by hand went, and the ones
//! that matter most are the ones no detection can ever put back. An export is unpacked wherever
//! somebody happened to unpack it (ADR 21) and the config is the only record of where. Nothing
//! in the archive is at risk, because a source id is permanent and re-adding the block resumes
//! capture (ADR 16) — but nobody re-adds a block they were never told had gone.
//!
//! So the config that would be written is checked against the file it would replace, and
//! anything the write cannot reproduce is named. The command then refuses rather than merging
//! the two, which is the decision this module exists to record:
//!
//! - Merging keeps only the fields this code knows about. Comments, key order and any setting
//!   added to `Config` after the merge was written go without a word — the same silent deletion
//!   in a smaller size. Refusing preserves the file whole, including the parts nothing here has
//!   ever heard of.
//! - It is what the crate already does everywhere else a permanent decision is involved.
//!   [`crate::drift`] prints a block to paste rather than adopting a source itself, and
//!   [`crate::unreachable`] does the same, both because an id is permanent once bytes are
//!   captured. The config is authored, and `cs` does not author it for you.
//!
//! The cost is that `--force` now declines on any config somebody has edited, and the escape
//! hatch is moving the file aside by hand. That is one command, and it is the one command that
//! keeps a copy.

use crate::config::{candidate_sources, Config, Source};
use crate::unreachable::SERVER_SIDE;
use std::path::PathBuf;

/// One thing in the existing config that the config `cs init` would write does not carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loss {
    /// A `[[sources]]` entry with no counterpart at all under that id.
    Source(Source, Unreproducible),
    /// Same id, different location, layout or globs. The path is the usual one: an external
    /// volume, or a tool's directory moved somewhere the candidate list does not look.
    Repointed { existing: Source, detected: Source },
    /// The archive would be looked for somewhere else. Everything captured stays where it is,
    /// and the tool stops seeing it — the loudest of these and the least likely.
    ArchiveRoot(PathBuf),
    /// The alias this machine chose for itself (ADR 17). The weakest of them: an archive that
    /// already exists keeps the alias in its directory name and `machine::load_or_create` reads
    /// it back from there, so this is only lost for real if the archive is later rebuilt.
    MachineAlias(String),
    /// `log_queries`, carrying the value the file states. Only ever a loss in one direction —
    /// a fresh config records searches, so overwriting one that switched it off starts writing
    /// to `queries.jsonl` again behind somebody who deliberately stopped it.
    QueryLog(bool),
    /// Comment lines, which a `Config` has never seen: the file is written by
    /// `toml::to_string_pretty`, which serialises values and nothing else.
    ///
    /// The smallest loss here and the one most likely to be the point. The reference config on
    /// the machine this was built for carries twenty lines explaining why the Takeout globs lead
    /// with `**/` and why the ChatGPT block points at a directory rather than at an export —
    /// reasons that took a survey to establish and that nothing else records.
    Comments(usize),
}

/// Why detection has no entry for a source, which is most of how bad dropping it would be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unreproducible {
    /// A surface that keeps every conversation on a vendor's server (ADR 21). Nothing detects
    /// it now and no improvement to detection ever will, so the block is the only record that
    /// an export was fetched at all, and the only record of where it was unpacked.
    ServerSide,
    /// A candidate this build knows, whose directory is not on the disk right now: a tool
    /// uninstalled, or an external volume that is not mounted. Detection would offer it again
    /// the day the directory returns, and delete it today without waiting to find out which.
    Absent,
    /// An id in neither register — a source added by hand for something this build ships no
    /// candidate for. Unreproducible by definition: nothing here knows the location or the
    /// globs, and there is nowhere to look them up.
    Unknown,
}

/// Everything in `existing` that writing `fresh` over it would remove or change.
///
/// Empty means the write is reversible by re-running detection, which is the only case `cs init`
/// lets through.
///
/// `existing_text` is the file `existing` was parsed from, and it is a second parameter rather
/// than an oversight: a `Config` is values, and the prose beside them is authored too. Passing
/// the empty string asks only about the values.
pub fn losses(existing: &Config, existing_text: &str, fresh: &Config) -> Vec<Loss> {
    // Destructured rather than read field by field, so a setting added to `Config` later cannot
    // slip past unexamined: this stops compiling until somebody has decided whether losing the
    // new field is worth refusing over. The whole bug was a field nobody thought about at the
    // moment the file was overwritten.
    let Config { archive_root, machine_alias, log_queries, sources } = existing;
    let mut losses = Vec::new();

    if archive_root != &fresh.archive_root {
        losses.push(Loss::ArchiveRoot(archive_root.clone()));
    }
    // Only a loss when the fresh config has nothing to put in its place. `--machine-alias` on
    // the command line is somebody restating it, and restating it is the escape hatch rather
    // than an oversight.
    if let (Some(alias), None) = (machine_alias, &fresh.machine_alias) {
        losses.push(Loss::MachineAlias(alias.clone()));
    }
    if log_queries != &fresh.log_queries {
        losses.push(Loss::QueryLog(*log_queries));
    }

    // In the file's own order, because that is the order somebody reading the report will be
    // reading their config in.
    for s in sources {
        match fresh.sources.iter().find(|f| f.id == s.id) {
            None => losses.push(Loss::Source(s.clone(), why_unreproducible(&s.id))),
            // Every field compared, not just the path. An include list edited to reach a
            // second file shape is as deliberate as a path, and as silently reverted.
            Some(f) if f != s => {
                losses.push(Loss::Repointed { existing: s.clone(), detected: f.clone() })
            }
            Some(_) => {}
        }
    }

    // Last, because it is the smallest of them and the report reads worst-first.
    let comments = comment_lines(existing_text);
    if comments > 0 {
        losses.push(Loss::Comments(comments));
    }
    losses
}

/// Lines that are wholly a comment.
///
/// Counted at the start of a trimmed line rather than anywhere in it, which undercounts: a `#`
/// after a value is a comment too, and telling one from a `#` inside a glob needs the parser
/// this deliberately does not have. Undercounting is the safe direction — the number decides
/// how loudly to say "there is prose here", and only zero decides anything at all.
fn comment_lines(text: &str) -> usize {
    text.lines().filter(|l| l.trim_start().starts_with('#')).count()
}

/// Which register an id belongs to, given that detection did not produce it.
///
/// The two registers are disjoint by test (`unreachable::no_server_side_surface_is_also_a_
/// detectable_candidate`), so the order of these arms is a convenience rather than a
/// tie-breaker.
fn why_unreproducible(id: &str) -> Unreproducible {
    if SERVER_SIDE.iter().any(|s| s.id == id) {
        Unreproducible::ServerSide
    } else if candidate_sources().iter().any(|c| c.id == id) {
        Unreproducible::Absent
    } else {
        Unreproducible::Unknown
    }
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

    /// The config `cs init` would write on a machine where detection found `codex`.
    fn fresh(sources: Vec<Source>) -> Config {
        Config {
            archive_root: PathBuf::from("/home/.chat-archive"),
            machine_alias: None,
            log_queries: true,
            sources,
        }
    }

    fn existing(f: impl FnOnce(&mut Config)) -> Config {
        let mut cfg = fresh(vec![src("codex", "/home/.codex/sessions")]);
        f(&mut cfg);
        cfg
    }

    #[test]
    fn a_config_detection_reproduces_exactly_loses_nothing() {
        // The case `--force` still exists for. Both configs name `codex` the same way, so the
        // write is the same file plus whatever detection has since found.
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert!(losses(&existing(|_| {}), "", &detected).is_empty());
    }

    #[test]
    fn a_config_that_names_less_than_detection_found_loses_nothing() {
        // A source appearing is what `--force` is for: `gemini-cli` installed after the config
        // was written is added, and adding is not a loss.
        let sparse = fresh(vec![]);
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert!(losses(&sparse, "", &detected).is_empty());
    }

    #[test]
    fn a_hand_added_export_is_named_as_the_thing_detection_cannot_put_back() {
        // The bug. `chatgpt-export` is 2,011 of the 2,935 conversations this was measured
        // against, it is not in the candidate list and cannot be, and the path it points at is
        // wherever somebody unpacked a download.
        let cfg = existing(|c| c.sources.push(src("chatgpt-export", "/home/exports")));
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(
            losses(&cfg, "", &detected),
            [Loss::Source(src("chatgpt-export", "/home/exports"), Unreproducible::ServerSide)]
        );
    }

    #[test]
    fn a_candidate_whose_directory_is_not_here_today_is_still_a_loss() {
        // An unmounted volume and an uninstalled tool are the same absence, and the difference
        // between them only shows up later. Deleting the block settles it the wrong way now.
        let cfg = existing(|c| c.sources.push(src("gemini-cli", "/home/.gemini/tmp")));
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(
            losses(&cfg, "", &detected),
            [Loss::Source(src("gemini-cli", "/home/.gemini/tmp"), Unreproducible::Absent)]
        );
    }

    #[test]
    fn a_source_this_build_ships_no_candidate_for_is_the_least_recoverable_of_all() {
        let cfg = existing(|c| c.sources.push(src("some-other-agent", "/home/.other")));
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(
            losses(&cfg, "", &detected),
            [Loss::Source(src("some-other-agent", "/home/.other"), Unreproducible::Unknown)]
        );
    }

    #[test]
    fn a_repointed_source_is_a_loss_because_the_path_was_a_decision() {
        // Same id, so nothing is dropped — but the archive on the external volume stops being
        // read the moment the path reverts to where the candidate list expects it.
        let cfg = existing(|c| c.sources[0].path = PathBuf::from("/Volumes/ext/.codex/sessions"));
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(
            losses(&cfg, "", &detected),
            [Loss::Repointed {
                existing: src("codex", "/Volumes/ext/.codex/sessions"),
                detected: src("codex", "/home/.codex/sessions"),
            }]
        );
    }

    #[test]
    fn an_edited_include_list_counts_as_much_as_an_edited_path() {
        // Widening a glob is how somebody reaches a file shape this build does not know about.
        // Reverting it is silent partial capture, which is the failure that looks like success.
        let cfg = existing(|c| c.sources[0].include.push("**/*.jsonl.zst".into()));
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert!(matches!(losses(&cfg, "", &detected)[..], [Loss::Repointed { .. }]));
    }

    #[test]
    fn the_archive_root_is_a_loss_because_a_fresh_config_looks_somewhere_else() {
        let cfg = existing(|c| c.archive_root = PathBuf::from("/Volumes/ext/chat-archive"));
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(
            losses(&cfg, "", &detected),
            [Loss::ArchiveRoot(PathBuf::from("/Volumes/ext/chat-archive"))]
        );
    }

    #[test]
    fn an_alias_survives_when_it_is_restated_on_the_command_line() {
        // `--machine-alias` is the documented way to set it, so passing the same one again is
        // somebody saying "keep this" in the only vocabulary `cs init` has.
        let cfg = existing(|c| c.machine_alias = Some("laptop".into()));
        let mut detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(losses(&cfg, "", &detected), [Loss::MachineAlias("laptop".into())]);

        detected.machine_alias = Some("laptop".into());
        assert!(losses(&cfg, "", &detected).is_empty());
        // A different alias is a rename somebody typed, not something being taken from them.
        detected.machine_alias = Some("desktop".into());
        assert!(losses(&cfg, "", &detected).is_empty());
    }

    #[test]
    fn query_logging_is_not_switched_back_on_behind_somebody() {
        // The default is on and the config is the only place it is ever off, so an overwrite
        // is the one event that can turn it back on without anybody asking for it.
        let cfg = existing(|c| c.log_queries = false);
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(losses(&cfg, "", &detected), [Loss::QueryLog(false)]);
    }

    #[test]
    fn prose_beside_the_values_is_a_loss_of_its_own() {
        // A config `cs init` could otherwise rewrite safely, annotated. The values survive the
        // round trip through `toml::to_string_pretty` and the reasons do not, and the reasons
        // are the half that cost somebody an afternoon.
        let cfg = existing(|_| {});
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        let text = "# unpacked here because the volume is bigger\narchive_root = \"/home/.chat-archive\"\n";
        assert_eq!(losses(&cfg, text, &detected), [Loss::Comments(1)]);
        assert!(losses(&cfg, "archive_root = \"/home/.chat-archive\"\n", &detected).is_empty());
    }

    #[test]
    fn a_hash_inside_a_value_is_not_mistaken_for_prose() {
        // Undercounting is the safe direction, and this is the case that decides it: a glob or a
        // path may hold a `#`, and refusing over one would make the refusal mean nothing.
        let cfg = existing(|_| {});
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert!(losses(&cfg, "  include = [\"**/#drafts/*.json\"]\n", &detected).is_empty());
        assert_eq!(comment_lines("   # indented prose is still prose\n"), 1);
    }

    #[test]
    fn every_loss_in_one_config_is_reported_in_one_pass() {
        // A report that named the first problem and stopped would be answered by fixing that
        // one and running again, which is how somebody ends up overwriting on the second try.
        let cfg = existing(|c| {
            c.machine_alias = Some("laptop".into());
            c.log_queries = false;
            c.sources.push(src("chatgpt-export", "/home/exports"));
        });
        let detected = fresh(vec![src("codex", "/home/.codex/sessions")]);
        assert_eq!(losses(&cfg, "", &detected).len(), 3);
    }
}
