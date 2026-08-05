//! What the config and this disk say about the sources, in the shape `cs_core::inventory`
//! joins against (chat-search-a7k.29).
//!
//! This is the only crate that can see both halves, so the conversion lives here once and
//! every client is handed the result: `cs status` renders it, and `cs tui` passes it in
//! because the TUI deliberately cannot reach `cs-archive` (docs/TUI-DESIGN.md §1).

use cs_archive::{Config, Drift, Source};
use cs_core::Watched;

/// Every source the machine knows about — configured or merely detected — and whether its
/// directory is here.
///
/// Takes the [`Drift`] rather than computing it, because a caller that also wants the paths
/// (`cs status` prints them) would otherwise re-stat every candidate to get at the same
/// answer. One detection per command.
pub fn watched(cfg: &Config, drift: &Drift) -> Vec<Watched> {
    let gone: Vec<&str> = drift.missing.iter().map(|s| s.id.as_str()).collect();
    let configured = cfg.sources.iter().map(|s| Watched {
        id: s.id.clone(),
        configured: true,
        present: !gone.contains(&s.id.as_str()),
    });
    // Detected and unclaimed: its conversations are accruing uncaptured right now
    // (chat-search-a7k.12), which is a thing a client should be able to draw rather than a
    // line that scrolls past during `cs archive`.
    let candidates = drift.unconfigured.iter().map(|s| Watched {
        id: s.id.clone(),
        configured: false,
        present: true,
    });
    configured.chain(candidates).collect()
}

/// The `[[sources]]` entry or detected candidate behind an id, for the path and layout that
/// only the config side knows. `None` for a source that only the index remembers.
pub fn source_by_id<'a>(cfg: &'a Config, drift: &'a Drift, id: &str) -> Option<&'a Source> {
    cfg.sources.iter().chain(drift.unconfigured.iter()).find(|s| s.id == id)
}
