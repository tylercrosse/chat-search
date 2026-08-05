//! Which sources exist, and how much of each one the index holds (chat-search-a7k.29).
//!
//! The index can only answer *which sources produced rows*, and that is not the question a
//! client asks. A configured source holding nothing and a source nobody ever configured are
//! the same absence in `conversation`, and they are opposite problems: the first is an
//! importer that threw or an archive run that never happened, the second is a tool the user
//! does not use. Drawn from the index alone, the broken one is invisible — you search, get
//! nothing, and conclude you used a different agent (chat-search-me9.14).
//!
//! `cs_archive::drift` already knows the missing half. It reports it as a printed line during
//! `cs archive`, which no other crate can read, so both clients that need it — the TUI's
//! facet bar and `cs status` — would each have to join it against the index themselves. That
//! is the shape of the local-date bug: one rule, derived in three places, wrong in two.
//!
//! So the join lives here, once, and returns a value.
//!
//! **This crate still does not read config.** The caller supplies [`Watched`] — what the
//! config names and what is on this disk — and gets the index counts folded in. Pointing
//! `cs-core` at a config would run the dependency backwards and drag `toml` and the globs
//! into the search crate (`config.rs:99`, docs/TUI-DESIGN.md §1), and `cs-tui` could not use
//! the result at all: it depends on `cs-core` and nothing else, so it has no way to reach a
//! type that lives in `cs-archive`. Facts in, one answer out, is the only arrangement that
//! serves both clients without either of them learning where the config lives.

use rusqlite::Connection;
use std::collections::BTreeMap;

/// What the config and this disk say about one source, before the index is consulted.
///
/// Assembled by whoever can see both — `cs` builds it from `Config::sources` and
/// `cs_archive::drift::detect`. A detected-but-unconfigured location is one of these with
/// `configured: false`, which is how a candidate accruing uncaptured conversations reaches a
/// client at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    /// Permanent, and part of every conversation id it produced (ADR 16). The join key.
    pub id: String,
    /// A `[[sources]]` entry names it, so `cs archive` captures it.
    pub configured: bool,
    /// Its directory is on this disk right now.
    pub present: bool,
}

/// Where a source stands with the config and this disk: the four cells of
/// configured × present, named.
///
/// Named rather than left as two booleans for the same reason [`crate::IndexState`] is —
/// clients branch on the name, and a state derived at each call site is a state derived
/// differently at each call site. The count is deliberately not part of it: "configured and
/// holding nothing" is [`Coverage::Live`] with zero conversations, and collapsing those two
/// facts into one would make an empty source indistinguishable from an unconfigured one all
/// over again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// Configured and its directory is here. Capture is set up; whether it has *worked* is
    /// the conversation count beside this, not this.
    Live,
    /// Configured, but the directory is gone — uninstalled, moved, or an external volume
    /// that is not mounted. What was already archived is safe; nothing new is arriving.
    Missing,
    /// Its directory is here and no `[[sources]]` entry claims it. Conversations are accruing
    /// uncaptured right now, which is what chat-search-a7k.12 exists to say out loud.
    Unconfigured,
    /// Neither configured nor on this disk, yet the index holds rows for it. A source that
    /// was captured once and has since fallen out of the config — the bead's own example is
    /// `chatgpt-export` sitting complete in the archive with 2,011 conversations while the
    /// config had lost the entry. Searchable, and never growing again.
    Retired,
}

impl Coverage {
    /// The four cells, in the only order they can be read: config first, then disk.
    pub fn of(configured: bool, present: bool) -> Self {
        match (configured, present) {
            (true, true) => Coverage::Live,
            (true, false) => Coverage::Missing,
            (false, true) => Coverage::Unconfigured,
            (false, false) => Coverage::Retired,
        }
    }

    /// The name this state travels under. Stable; the prose above it is not.
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::Live => "live",
            Coverage::Missing => "missing",
            Coverage::Unconfigured => "unconfigured",
            Coverage::Retired => "retired",
        }
    }

    /// Whether a `[[sources]]` entry names it.
    pub fn configured(self) -> bool {
        matches!(self, Coverage::Live | Coverage::Missing)
    }

    /// Whether its directory is on this disk.
    pub fn present(self) -> bool {
        matches!(self, Coverage::Live | Coverage::Unconfigured)
    }
}

/// One source, and everything a client needs to draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCoverage {
    pub id: String,
    pub coverage: Coverage,
    /// Conversations the index holds for it. Zero is an answer rather than an absence — read
    /// beside [`SourceCoverage::coverage`] it separates a broken importer from a tool that
    /// was never watched.
    pub conversations: i64,
}

/// The full inventory: everything watched, unioned with everything the index holds.
///
/// A read failure is returned rather than swallowed. Callers differ on what to do with it —
/// the facet bar drops the census and keeps searching, `cs status` says so — and a function
/// that decided for them would be making that call in the wrong place.
pub fn of(conn: &Connection, watched: &[Watched]) -> rusqlite::Result<Vec<SourceCoverage>> {
    Ok(join(watched, &counts(conn)?))
}

/// Conversations per source, as the index has them. Sources with no rows are simply absent —
/// which is the entire reason [`join`] exists.
fn counts(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare("SELECT source, COUNT(*) FROM conversation GROUP BY source")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// The join itself, over supplied inputs.
///
/// Split from [`of`] for the same reason `drift::diff` is split from `drift::detect`: the
/// interesting cases are a source in one input and not the other, and building an index to
/// reach them would test SQLite rather than the rule.
///
/// Ordered by id, so the facet bar never reshuffles between runs and two clients listing the
/// same corpus list it the same way.
pub fn join(watched: &[Watched], indexed: &[(String, i64)]) -> Vec<SourceCoverage> {
    let mut out: BTreeMap<&str, SourceCoverage> = BTreeMap::new();
    for w in watched {
        // First entry wins. `Config::validate` rejects a duplicate id before it can reach
        // here, and a second one would be a second permanent namespace for one directory
        // (ADR 16) rather than something to merge.
        out.entry(&w.id).or_insert_with(|| SourceCoverage {
            id: w.id.clone(),
            coverage: Coverage::of(w.configured, w.present),
            conversations: 0,
        });
    }
    for (id, n) in indexed {
        out.entry(id)
            .or_insert_with(|| SourceCoverage {
                id: id.clone(),
                coverage: Coverage::Retired,
                conversations: 0,
            })
            .conversations = *n;
    }
    out.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Conversation, Kind, Message, Role, Titles};

    fn watched(id: &str, configured: bool, present: bool) -> Watched {
        Watched { id: id.into(), configured, present }
    }

    #[test]
    fn a_configured_source_with_no_rows_is_not_the_same_as_one_nobody_configured() {
        // The whole bead in one assertion: both hold zero conversations, and a client that
        // could only see the index would draw them identically — or, worse, draw neither.
        let inventory = join(
            &[watched("claude-code", true, true), watched("copilot-cli", false, true)],
            &[],
        );
        assert_eq!(inventory[0].coverage, Coverage::Live);
        assert_eq!(inventory[1].coverage, Coverage::Unconfigured);
        assert!(inventory.iter().all(|s| s.conversations == 0));
    }

    #[test]
    fn a_source_the_index_holds_but_nothing_watches_is_still_in_the_inventory() {
        // `chatgpt-export`: 2,011 conversations archived, then the config lost the entry.
        // Dropping it would hide a searchable third of the corpus from the facet bar.
        let inventory = join(&[watched("codex", true, true)], &[("chatgpt-export".into(), 2011)]);
        assert_eq!(inventory.len(), 2);
        assert_eq!(inventory[0].id, "chatgpt-export");
        assert_eq!(inventory[0].coverage, Coverage::Retired);
        assert_eq!(inventory[0].conversations, 2011);
    }

    #[test]
    fn a_configured_source_whose_directory_vanished_keeps_its_rows() {
        let inventory = join(&[watched("gemini-cli", true, false)], &[("gemini-cli".into(), 112)]);
        assert_eq!(inventory[0].coverage, Coverage::Missing);
        assert_eq!(inventory[0].conversations, 112);
    }

    #[test]
    fn the_inventory_is_ordered_by_id_whichever_side_each_source_came_from() {
        let inventory = join(
            &[watched("codex", true, true), watched("claude-code", true, true)],
            &[("gemini-cli".into(), 1), ("codex".into(), 2)],
        );
        let ids: Vec<_> = inventory.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["claude-code", "codex", "gemini-cli"]);
    }

    #[test]
    fn every_state_can_say_what_it_is_made_of() {
        for (configured, present) in [(true, true), (true, false), (false, true), (false, false)] {
            let c = Coverage::of(configured, present);
            assert_eq!((c.configured(), c.present()), (configured, present), "{}", c.as_str());
        }
    }

    #[test]
    fn the_counts_come_from_the_index_and_the_states_from_the_caller() {
        let mut conn = crate::index::open(":memory:").unwrap();
        crate::index::write_conversations(&mut conn, &[conv("codex", "a"), conv("codex", "b")])
            .unwrap();

        let inventory = of(
            &conn,
            &[watched("codex", true, true), watched("claude-code", true, true)],
        )
        .unwrap();
        assert_eq!(inventory.len(), 2);
        assert_eq!((inventory[1].id.as_str(), inventory[1].conversations), ("codex", 2));
        // The one the index cannot see at all, which is the point.
        assert_eq!((inventory[0].id.as_str(), inventory[0].conversations), ("claude-code", 0));
        assert_eq!(inventory[0].coverage, Coverage::Live);
    }

    fn conv(source: &str, native_id: &str) -> Conversation {
        Conversation {
            source: source.into(),
            native_id: native_id.into(),
            titles: Titles::default(),
            cwd: None,
            git_branch: None,
            model: None,
            surface: None,
            forked_from_native_id: None,
            head_native_id: None,
            messages: vec![Message {
                native_id: "m1".into(),
                parent_native_id: None,
                thread_key: "t".into(),
                is_sidechain: false,
                is_error: false,
                seq: 0,
                role: Role::User,
                kind: Kind::Prose,
                ts: Some(1_700_000_000_000),
                text: "hello".into(),
            }],
        }
    }
}
