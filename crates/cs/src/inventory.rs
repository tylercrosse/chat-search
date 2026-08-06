//! What the config and this disk say about the sources, in the shape `cs_core::inventory`
//! joins against (chat-search-a7k.29).
//!
//! This is the only crate that can see both halves, so the conversion lives here once and
//! every client is handed the result: `cs status` renders it, `cs facets` projects a rail out
//! of it, and `cs tui` passes it in because the TUI deliberately cannot reach `cs-archive`
//! (docs/TUI-DESIGN.md §1).

use cs_archive::{Config, Drift, Source};
use cs_core::Watched;
use std::path::Path;

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

/// One read against the index, or `None` and a word about why.
///
/// **An unreadable index costs the counts and nothing else.** A `cs status` that failed
/// because there is no index yet would be useless on the run where it is needed most, and the
/// same is true of a facet rail: on a first run the honest rail is every configured source at
/// zero, not an empty pane. Two of the four reasons — nothing there, and a build in progress
/// — are already named by the `index_state` both commands report beside this, so a zero under
/// either is unambiguous. The other two are not, and say so on stderr rather than letting a
/// real corpus report itself as empty.
fn counted<T>(
    db: &Path,
    read: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
) -> Option<T> {
    match cs_core::open_for_read(db) {
        Ok(reader) => match read(&reader.conn) {
            Ok(counts) => return Some(counts),
            Err(e) => eprintln!("counting conversations: {e} — the counts read 0"),
        },
        Err(e) => {
            let named = matches!(
                e,
                cs_core::Unreadable::NoIndex { .. } | cs_core::Unreadable::Building { .. }
            );
            if !named {
                eprintln!("{e} — the counts read 0");
            }
        }
    }
    None
}

/// The census: what is watched, with the index counts folded in if the index can be read.
pub fn census(db: &Path, watched: &[Watched]) -> Vec<cs_core::SourceCoverage> {
    counted(db, |conn| cs_core::inventory::of(conn, watched))
        .unwrap_or_else(|| cs_core::inventory::join(watched, &[]))
}

/// Every census a facet rail needs, through one connection.
///
/// Three reads rather than three opens: `cs facets` runs once per keystroke in the macOS app, and
/// the rail is already a second process on that path (`chat-search-me9.8.5`).
///
/// The other two have no config half to fall back on, so an unreadable index leaves them empty —
/// which `cs_core::facets` reads as no directories and four spans at zero, rather than as a
/// corpus with no history.
pub fn rails(
    db: &Path,
    watched: &[Watched],
    dirs: usize,
    now_ms: i64,
) -> (Vec<cs_core::SourceCoverage>, cs_core::DirCensus, Vec<cs_core::DateCoverage>) {
    counted(db, |conn| {
        Ok((
            cs_core::inventory::of(conn, watched)?,
            cs_core::inventory::dirs(conn, dirs)?,
            cs_core::inventory::dates(conn, now_ms)?,
        ))
    })
    .unwrap_or_else(|| {
        (cs_core::inventory::join(watched, &[]), cs_core::DirCensus::default(), Vec::new())
    })
}
