//! Conversations that exist only on a vendor's server (chat-search-a7k.22).
//!
//! The other two standing reports start from something observable. [`crate::drift`] asks whether
//! the config still describes this disk, and [`crate::staleness`] asks whether a configured
//! export has stopped happening. Both need a directory to look at.
//!
//! ChatGPT, claude.ai and Gemini on the web give them none. Those surfaces write nothing local
//! at all — ChatGPT's desktop store is encrypted behind a Keychain access group, and the other
//! two keep conversations server-side with no local copy of the text (ADR 21) — so
//! [`crate::config::candidate_sources`] cannot find them, and no improvement to detection ever
//! will. That is what makes the gap worse than an unadopted tool rather than smaller: `cs init`
//! on a second machine writes a config that captures every agent on the disk and omits ChatGPT
//! entirely, drift has nothing to detect, and staleness only watches sources already configured.
//! Nothing on the machine is left to say so.
//!
//! So this report is *stated* rather than detected. The register below is the one list in this
//! crate that cannot be derived from anything, which is exactly why it has to exist: a surface
//! left out of it is a surface nothing will ever mention again.

use crate::config::{Layout, Source};
use crate::error::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A conversation surface with no local files: the transcripts live on a vendor's server and
/// arrive here only when a human fetches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Surface {
    /// The source id to configure it under, permanent once bytes are captured (ADR 16).
    ///
    /// Naming it in the report is what lets the report end: coverage is an exact id match, so a
    /// config that adopted this surface under an id of its own choosing keeps being told to go
    /// and fetch something it already has. Matching loosely would be worse — `gemini-cli` is a
    /// live tool directory and would read as coverage for Gemini on the web, silently retiring
    /// the line for the surface that needs it most.
    pub id: &'static str,
    /// What a person calls it. A source id is a key, not a product name, and the reader of this
    /// line has to recognise the account they would be logging into.
    pub name: &'static str,
    /// The human act that puts the bytes on this disk. There is no other route, which is the
    /// whole reason for the entry.
    pub fetch: &'static str,
    /// Include globs for a `[[sources]]` block, or empty when no export of this shape has landed
    /// here yet. Empty is a real answer and not a gap to fill in later: a guessed glob captures
    /// part of an export and says nothing about the rest, which is the silent partial capture
    /// [`crate::drift::Drift::adoption_toml`] exists to prevent.
    pub include: &'static [&'static str],
}

/// Every surface known to keep conversations server-side, whether or not this machine has an
/// account on one.
///
/// Hand-maintained, and unavoidably so — [`crate::config::candidate_sources`] is at least
/// filtered by what exists on the disk, and there is nothing here to filter against. The list is
/// short because the constraint is narrow: a surface belongs here only if its transcripts reach
/// this machine by a human act and by no other route (ADR 21).
pub const SERVER_SIDE: &[Surface] = &[
    Surface {
        id: "chatgpt-export",
        name: "ChatGPT",
        // Kept to the few words that get someone there. The line it lands in is already
        // carrying a label and an id, and the archive table beside it is 88 columns wide.
        fetch: "Settings → Data controls → Export data",
        // Two shapes, because an export has had two. The sharded
        // `Conversations__<hash>-chatgpt-NNNN/conversations-NNN.json` is what the reference
        // export on this machine unpacks to; a single `conversations.json` is the older
        // whole-account file. The globs are disjoint, and the importer takes bytes rather than
        // paths, so covering both costs nothing and missing one costs the entire account.
        include: &["**/conversations-*.json", "**/conversations.json"],
    },
    Surface {
        id: "claude-ai",
        name: "Claude on claude.ai",
        fetch: "Settings → request a data export",
        // No export of this shape has been seen here, so nothing is offered. The id is still
        // named: knowing what to call it is most of the value, since it is permanent (ADR 16)
        // and is already the id `cs-tui` reserves a colour for.
        include: &[],
    },
    Surface {
        id: "google-takeout",
        name: "Gemini on the web",
        // The domain rather than a settings path, because Takeout is not reached from Gemini.
        fetch: "takeout.google.com → Gemini Apps",
        // Moved here from `cs-import`'s `google_takeout` module doc, which could hold them but
        // never print them. Why they are shaped this way — a leading `**/` because Takeout
        // unpacks to a bare `Takeout/` every time, and named files rather than a sweep because
        // 311 MB of the 330 MB reference export is NotebookLM PDFs and audio — is still written
        // out there, beside the format they belong to.
        include: &[
            "**/My Activity/*/MyActivity.json",
            "**/Conversation History/conversation_*.txt",
            "**/Chat History/*.html",
        ],
    },
];

/// Conversations `chatgpt-export` alone contributed, the one time this could be measured:
/// 2,011 of the 2,935 `cs index` held on 2026-07-30 (chat-search-a7k.22).
///
/// Frozen rather than queried, for a reason that is the whole bug in miniature. The machine that
/// most needs to hear this figure has no index yet, and an index built from a config missing
/// these surfaces reports their share as 0% however large it really is — so a live query would
/// go silent exactly where the stated number still speaks. It is also an understatement by
/// construction: the denominator counts only what was reachable, and claude.ai and Gemini on the
/// web contributed nothing to it at all.
pub const MEASURED_FROM_EXPORT: u64 = 2_011;

/// The corpus [`MEASURED_FROM_EXPORT`] was measured against.
pub const MEASURED_CORPUS: u64 = 2_935;

/// [`MEASURED_FROM_EXPORT`] as a percentage, rounded once and in one place.
///
/// Integer division would say 68 where the measurement says 69, and a figure that disagrees with
/// the bug it came from is a figure nobody can check.
pub fn measured_percent() -> u64 {
    (100.0 * MEASURED_FROM_EXPORT as f64 / MEASURED_CORPUS as f64).round() as u64
}

/// The measurement as a reader should see it, rendered here so every caller says it the same way.
pub fn measured_share() -> String {
    format!(
        "{} of {} conversations, {}%",
        grouped(MEASURED_FROM_EXPORT),
        grouped(MEASURED_CORPUS),
        measured_percent()
    )
}

/// Thousands separators. Four digits beside a date-shaped sentence read as a year, and this
/// figure is quoted in the bug and in `docs/ARCHITECTURE.md` with the comma in it.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|c| std::str::from_utf8(c).expect("ascii digits"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Where a pasted block points until someone edits it.
///
/// One directory for every export rather than one each, because that is how the reference config
/// does it and because an export is unpacked wherever the person happens to be. If it does not
/// exist, [`crate::drift`] reports the source as `missing` on the next run — a paste that was
/// never finished says so by itself rather than looking configured.
const UNPACK_INTO: &str = "~/exports";

/// The surfaces this config has no entry for.
///
/// Not `detect`, because nothing is detected: [`SERVER_SIDE`] is stated, and the only question
/// asked of the machine is which of its entries the config has already answered.
pub fn pending(configured: &[Source]) -> Vec<&'static Surface> {
    let ids: HashSet<&str> = configured.iter().map(|s| s.id.as_str()).collect();
    SERVER_SIDE.iter().filter(|s| !ids.contains(s.id)).collect()
}

/// `[[sources]]` blocks for the pending surfaces whose globs are known, ready to paste.
///
/// Built through [`Source`] and the same serializer [`crate::drift::Drift::adoption_toml`] uses,
/// so a block cannot drift into a shape `Config::load` then rejects. The one field a paste has
/// to edit is `path`, and it is the one field this crate cannot know — an export has no
/// canonical home, which is the reason the surface is in this list at all.
pub fn adoption_toml(pending: &[&Surface]) -> String {
    #[derive(Serialize)]
    struct Doc {
        sources: Vec<Source>,
    }
    let sources: Vec<Source> = pending
        .iter()
        .filter(|s| !s.include.is_empty())
        .map(|s| Source {
            id: s.id.to_string(),
            path: PathBuf::from(UNPACK_INTO),
            layout: Layout::Mirror,
            include: s.include.iter().map(|g| g.to_string()).collect(),
        })
        .collect();
    if sources.is_empty() {
        return String::new();
    }
    toml::to_string_pretty(&Doc { sources }).unwrap_or_default()
}

/// Beside `.drift.json` and `.staleness.json`, dotted for the same reason: machine-local
/// bookkeeping, not captured data, and never synced (ADR 11 syncs raw bytes only).
fn state_path(machine_dir: &Path) -> PathBuf {
    machine_dir.join(".unreachable.json")
}

/// Whether to print this report now, recording the answer.
///
/// Identity is the set of pending ids, so configuring one of three prints the remaining two
/// immediately rather than a day later, and an empty set clears the state — a `[[sources]]`
/// entry deleted after the fact is news again.
///
/// Unlike the other two reports, this one describes a condition that never resolves itself. A
/// tool directory gets adopted, an export lands, and drift and staleness fall silent on their
/// own; a surface with no account behind it stays pending forever and says so once a day. That
/// is the deliberate trade, and the cheap-looking alternative is worse: a line that stops before
/// the person who needed it read it, over a share of the corpus the report itself measures at
/// [`measured_percent`].
pub fn claim(machine_dir: &Path, pending: &[&Surface], now_ms: u64) -> Result<bool> {
    let fingerprint =
        (!pending.is_empty()).then(|| pending.iter().map(|s| s.id).collect::<Vec<_>>().join(","));
    crate::throttle::claim(&state_path(machine_dir), fingerprint.as_deref(), now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::candidate_sources;

    fn src(id: &str, path: &str) -> Source {
        Source {
            id: id.into(),
            path: PathBuf::from(path),
            layout: Layout::Mirror,
            include: vec!["**/*.json".into()],
        }
    }

    fn ids(pending: &[&Surface]) -> Vec<&'static str> {
        pending.iter().map(|s| s.id).collect()
    }

    #[test]
    fn a_config_that_names_none_of_them_is_told_about_all_of_them() {
        // What `cs init` writes on a second machine: every agent on the disk, and not one of the
        // surfaces that hold most of the conversations.
        let configured =
            [src("codex", "/home/.codex/sessions"), src("claude-code", "/home/.claude")];
        assert_eq!(ids(&pending(&configured)), ["chatgpt-export", "claude-ai", "google-takeout"]);
    }

    #[test]
    fn a_surface_that_has_been_configured_is_not_offered_again() {
        let configured = [src("chatgpt-export", "/home/exports")];
        assert_eq!(ids(&pending(&configured)), ["claude-ai", "google-takeout"]);
    }

    #[test]
    fn nothing_is_pending_once_every_surface_is_configured() {
        let configured: Vec<Source> =
            SERVER_SIDE.iter().map(|s| src(s.id, "/home/exports")).collect();
        assert!(pending(&configured).is_empty());
    }

    #[test]
    fn a_live_tool_directory_is_not_coverage_for_the_web_surface_it_shares_a_name_with() {
        // The trap that decides why coverage is an exact id match. `gemini-cli` is a directory
        // the CLI writes into; Gemini on the web is an account with no local files, and the two
        // share nothing but four letters. A looser match would retire this report on the machine
        // that most needs it and leave no trace of having done so.
        let configured = [src("gemini-cli", "/home/.gemini/tmp")];
        assert!(ids(&pending(&configured)).contains(&"google-takeout"));
    }

    #[test]
    fn no_server_side_surface_is_also_a_detectable_candidate() {
        // The coherence check between the two registers, and it has to fail loudly rather than
        // quietly: an id in both would be reported here *and* offered by `drift`, and
        // `staleness` — which derives "export-shaped" from absence in the candidate list — would
        // stop watching it the moment it was configured. One id, one register.
        let detectable = candidate_sources();
        for s in SERVER_SIDE {
            assert!(
                !detectable.iter().any(|c| c.id == s.id),
                "{} is in both registers",
                s.id
            );
        }
    }

    #[test]
    fn the_measured_share_is_the_one_the_bug_was_filed_with() {
        // 2,011 of 2,935 rounds to 69, and integer division would say 68. The figure is quoted in
        // chat-search-a7k.22 and in docs/ARCHITECTURE.md; a report that said 68, or that dropped
        // the commas the docs carry, would read as a different measurement of a different thing.
        assert_eq!(measured_percent(), 69);
        assert_eq!(measured_share(), "2,011 of 2,935 conversations, 69%");
    }

    #[test]
    fn grouping_holds_at_every_length_a_corpus_can_be() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn the_paste_block_is_a_usable_config_fragment() {
        let toml = adoption_toml(&pending(&[]));
        assert!(toml.contains(r#"id = "chatgpt-export""#), "{toml}");
        assert!(toml.contains(r#"id = "google-takeout""#), "{toml}");
        // Round-trips into the real config shape, so pasting it cannot produce a config that
        // `Config::load` then rejects.
        let parsed: crate::Config =
            toml::from_str(&format!("archive_root = \"/tmp/a\"\n{toml}")).unwrap();
        assert_eq!(parsed.sources.len(), 2);
        assert_eq!(parsed.sources[0].id, "chatgpt-export");
    }

    #[test]
    fn a_surface_whose_export_has_never_been_seen_is_named_without_a_block() {
        // `claude-ai` has no globs, because no export of that shape has landed here. It still
        // appears in the report — the id and the route are the useful part — but a guessed glob
        // would capture some of an export and stay silent about the rest.
        let claude = SERVER_SIDE.iter().find(|s| s.id == "claude-ai").unwrap();
        assert!(claude.include.is_empty());
        assert!(adoption_toml(&[claude]).is_empty());
    }

    /// The globs as [`crate::scan`] will actually apply them: against the source-relative path,
    /// through `globset`. A pattern checked by eye is a pattern that captures three quarters of
    /// an export and reports success.
    fn captures(surface: &Surface, rel_path: &str) -> bool {
        let mut b = globset::GlobSetBuilder::new();
        for g in surface.include {
            b.add(globset::Glob::new(g).expect("recommended glob does not compile"));
        }
        b.build().unwrap().is_match(rel_path)
    }

    #[test]
    fn the_recommended_chatgpt_globs_match_both_shapes_an_export_has_had() {
        // The reference export here unpacks to a sharded directory; the older whole-account file
        // is a bare `conversations.json`. Covering one shape and not the other reads identically
        // from the outside — an archive with nothing in it.
        let chatgpt = SERVER_SIDE.iter().find(|s| s.id == "chatgpt-export").unwrap();
        assert!(captures(chatgpt, "Conversations__abc-chatgpt-0001/conversations-000.json"));
        assert!(captures(chatgpt, "conversations.json"));
        assert!(!captures(chatgpt, "Conversations__abc-chatgpt-0001/chat.html"), "not the render");
    }

    #[test]
    fn the_recommended_takeout_globs_reach_the_conversations_and_not_the_bulk() {
        // 311 MB of the 330 MB reference export is NotebookLM source PDFs and audio. The globs
        // are narrow on purpose, and they lead with `**/` because Takeout unpacks to a bare
        // `Takeout/` every time — the directory a second export is renamed to is unknowable.
        let takeout = SERVER_SIDE.iter().find(|s| s.id == "google-takeout").unwrap();
        assert!(captures(takeout, "Takeout-gemini/My Activity/Gemini Apps/MyActivity.json"));
        assert!(captures(takeout, "renamed/Gemini in Workspace/Conversation History/conversation_1.txt"));
        assert!(!captures(takeout, "Takeout/NotebookLM/nb/Sources/paper.pdf"), "not the sources");
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("cs-unreachable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_same_report_does_not_print_on_every_run() {
        // 288 runs a day under launchd. This is the one report whose condition may never resolve,
        // so it is also the one with the most to lose from becoming wallpaper.
        let dir = tmpdir();
        let all = pending(&[]);
        assert!(claim(&dir, &all, 0).unwrap(), "first sighting is news");
        for i in 1..288 {
            assert!(!claim(&dir, &all, i * 300_000).unwrap(), "run {i} reported again");
        }
        assert!(claim(&dir, &all, crate::throttle::REPEAT_AFTER_MS).unwrap(), "a day on, again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn configuring_one_surface_reprints_the_rest_immediately() {
        // Somebody acting on the report is the moment it is being read. Making them wait a day to
        // find out what is left would be the worst possible time to go quiet.
        let dir = tmpdir();
        let all = pending(&[]);
        let rest = pending(&[src("chatgpt-export", "/home/exports")]);
        assert!(claim(&dir, &all, 0).unwrap());
        assert!(!claim(&dir, &all, 300_000).unwrap());
        assert!(claim(&dir, &rest, 600_000).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn configuring_every_surface_clears_the_state() {
        let dir = tmpdir();
        let all = pending(&[]);
        assert!(claim(&dir, &all, 0).unwrap());
        assert!(!claim(&dir, &[], 300_000).unwrap(), "nothing left to say");
        // Deleted from the config again: an event, not a repeat of one.
        assert!(claim(&dir, &all, 600_000).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }
}
