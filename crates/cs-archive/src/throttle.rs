//! How often a standing condition is allowed to repeat itself.
//!
//! `cs archive` runs every 300 s under launchd, so a line that describes a *state* rather than
//! an event — a candidate left unadopted (chat-search-a7k.12), an export left un-run
//! (chat-search-a7k.10) — would print 288 times a day if it printed whenever it was true. That
//! is how a warning becomes wallpaper, and once quiet mode (chat-search-a7k.5) silences the
//! "nothing changed" table those lines *are* the log.
//!
//! One window and one state-file format for every such report. Extracted from [`crate::drift`]
//! when the staleness nag became the second caller, because two copies of "how long is quiet"
//! is the shape of bug the one-rule-in-one-place convention exists to prevent.

use crate::error::{Error, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// How long an unchanged report stays quiet.
///
/// A day is short enough that the nag survives a skimmed log and long enough that it never
/// competes with real output.
pub const REPEAT_AFTER_MS: u64 = 24 * 60 * 60 * 1000;

/// What was last printed, so the same thing is not printed 288 times a day.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Reported {
    fingerprint: String,
    at_ms: u64,
}

/// Whether to print this report now, recording the answer in `state`.
///
/// The fingerprint identifies *what* would be printed and deliberately not *when*, so it has to
/// change exactly when the printed lines would change and not otherwise. Any change to it
/// prints immediately — a candidate appearing, a source going stale, one being resolved are all
/// events. An unchanged fingerprint repeats once per [`REPEAT_AFTER_MS`].
///
/// `None` means there is nothing to report, which clears the state: a condition that comes back
/// after being resolved is an event again rather than waiting out a stale window.
pub fn claim(state: &Path, fingerprint: Option<&str>, now_ms: u64) -> Result<bool> {
    let Some(fingerprint) = fingerprint else {
        return match std::fs::remove_file(state) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).at(state),
            _ => Ok(false),
        };
    };

    // A state file that failed to parse must not fail an archive run, and must not suppress
    // either: treat it as "never reported" and let the run rewrite it. The cost of being wrong
    // is one extra line.
    let last: Option<Reported> =
        std::fs::read_to_string(state).ok().and_then(|t| serde_json::from_str(&t).ok());

    let due = last.is_none_or(|r| {
        r.fingerprint != fingerprint || now_ms.saturating_sub(r.at_ms) >= REPEAT_AFTER_MS
    });
    if due {
        let body =
            serde_json::to_vec(&Reported { fingerprint: fingerprint.to_string(), at_ms: now_ms })
                .map_err(|source| Error::Json { path: state.to_path_buf(), source })?;
        std::fs::write(state, body).at(state)?;
    }
    Ok(due)
}
