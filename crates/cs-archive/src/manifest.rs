use crate::error::{Error, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// What the archiver observed about one file (ADR 19). Append-only: the current state of
/// the archive is a fold over these, never a mutable record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// First time this path was captured.
    Seen,
    /// Grew, with its prefix intact — the ordinary case for an append-only transcript.
    Appended,
    /// Prefix changed or the file shrank: compacted or rewritten in place.
    Rewritten,
    /// Present before, absent now. The archived copy is kept; only the absence is recorded.
    Vanished,
}

/// A file's identity for change detection. The prefix hash covers `prefix_len` bytes, which
/// is `min(64 KiB, size)` — storing the length matters because a file smaller than 64 KiB
/// that grows would otherwise hash a *different* span and look rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime_ms: u64,
    pub prefix_hash: String,
    pub prefix_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts: u64,
    pub op: Op,
    pub source: String,
    /// Path relative to the source root, always with `/` separators.
    pub path: String,
    #[serde(flatten)]
    pub fingerprint: Fingerprint,
}

/// The folded current state: the most recent event per `(source, path)`.
#[derive(Debug, Default)]
pub struct Manifest {
    latest: HashMap<(String, String), Event>,
}

impl Manifest {
    pub fn get(&self, source: &str, path: &str) -> Option<&Event> {
        self.latest.get(&(source.to_string(), path.to_string()))
    }

    pub fn len(&self) -> usize {
        self.latest.len()
    }

    pub fn is_empty(&self) -> bool {
        self.latest.is_empty()
    }

    /// Every path recorded for a source that is not currently marked `Vanished`.
    pub fn live_paths(&self, source: &str) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .latest
            .values()
            .filter(|e| e.source == source && e.op != Op::Vanished)
            .map(|e| e.path.as_str())
            .collect();
        v.sort_unstable();
        v
    }

    /// When the newest bytes this archive holds for a source were written, over paths that
    /// have not vanished. `None` when nothing has ever been captured for it.
    ///
    /// The file's mtime and not the event's `ts`: `ts` is when the archiver *looked*, which is
    /// today for a month-old export first captured today. Staleness is a question about the
    /// bytes, so it has to be answered from the bytes (chat-search-a7k.10).
    ///
    /// Deliberately not "the newest conversation" — that would mean parsing, which this crate
    /// must never do (ADR 16). For a source whose files arrive by being unpacked the two
    /// differ by the hours between the vendor building the export and it landing here, which
    /// is far inside the week the caller measures against.
    pub fn newest_mtime_ms(&self, source: &str) -> Option<u64> {
        self.latest
            .values()
            .filter(|e| e.source == source && e.op != Op::Vanished)
            .map(|e| e.fingerprint.mtime_ms)
            .max()
    }

    pub fn apply(&mut self, event: Event) {
        self.latest.insert((event.source.clone(), event.path.clone()), event);
    }

    /// Fold every `manifest/*.jsonl` under a machine directory, in filename order.
    ///
    /// Files are named by period (`2026-07.jsonl`), so lexicographic order is chronological.
    /// A truncated final line — from a crash mid-append — is skipped rather than fatal;
    /// the next scan re-observes that file and writes a fresh event.
    pub fn load(machine_dir: &Path) -> Result<Self> {
        let dir = machine_dir.join("manifest");
        let mut manifest = Manifest::default();
        if !dir.is_dir() {
            return Ok(manifest);
        }

        let mut logs: Vec<_> = std::fs::read_dir(&dir)
            .at(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .collect();
        logs.sort();

        for log in logs {
            let text = std::fs::read_to_string(&log).at(&log)?;
            for (n, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Event>(line) {
                    Ok(event) => manifest.apply(event),
                    // Only tolerate damage on the very last line; anything earlier means
                    // the log is corrupt in a way that would silently lose history.
                    Err(source) if n + 1 == text.lines().count() => {
                        let _ = source;
                    }
                    Err(source) => return Err(Error::Json { path: log.clone(), source }),
                }
            }
        }
        Ok(manifest)
    }
}

/// Append-only writer for the manifest (ADR 19). Never rewrites; the folded state is
/// always recomputed from the log.
pub struct ManifestWriter {
    dir: std::path::PathBuf,
}

impl ManifestWriter {
    pub fn new(machine_dir: &Path) -> Result<Self> {
        let dir = machine_dir.join("manifest");
        std::fs::create_dir_all(&dir).at(&dir)?;
        Ok(Self { dir })
    }

    /// Events are grouped by period first, so a batch that straddles a month boundary
    /// still lands in the right logs and filename order stays chronological.
    pub fn append(&self, events: &[Event]) -> Result<()> {
        use std::io::Write;
        let mut by_period: HashMap<String, Vec<&Event>> = HashMap::new();
        for e in events {
            by_period.entry(period(e.ts)).or_default().push(e);
        }
        for (period, batch) in by_period {
            let path = self.dir.join(format!("{period}.jsonl"));
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .at(&path)?;
            let mut w = std::io::BufWriter::new(file);
            for e in batch {
                let line = serde_json::to_string(e)
                    .map_err(|source| Error::Json { path: path.clone(), source })?;
                writeln!(w, "{line}").at(&path)?;
            }
            w.flush().at(&path)?;
        }
        Ok(())
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `yyyy-mm` for a millisecond timestamp, used to name manifest logs.
pub fn period(ts_ms: u64) -> String {
    let (y, m, _) = civil_from_days((ts_ms / 86_400_000) as i64);
    format!("{y:04}-{m:02}")
}

/// Days since the Unix epoch to a civil (year, month, day), by Howard Hinnant's
/// `civil_from_days`. Ten lines and exact, which beats taking a date dependency for the
/// one place the archiver needs a calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(source: &str, path: &str, op: Op, size: u64) -> Event {
        Event {
            ts: size, // monotonic enough for tests
            op,
            source: source.into(),
            path: path.into(),
            fingerprint: Fingerprint {
                size,
                mtime_ms: size,
                prefix_hash: format!("h{size}"),
                prefix_len: size.min(65536),
            },
        }
    }

    #[test]
    fn fold_keeps_the_latest_event_per_path() {
        let mut m = Manifest::default();
        m.apply(ev("codex", "a.jsonl", Op::Seen, 10));
        m.apply(ev("codex", "a.jsonl", Op::Appended, 20));
        assert_eq!(m.len(), 1);
        assert_eq!(m.get("codex", "a.jsonl").unwrap().fingerprint.size, 20);
    }

    #[test]
    fn same_path_in_two_sources_is_two_entries() {
        let mut m = Manifest::default();
        m.apply(ev("codex", "a.jsonl", Op::Seen, 1));
        m.apply(ev("claude-code", "a.jsonl", Op::Seen, 2));
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("codex", "a.jsonl").unwrap().fingerprint.size, 1);
    }

    #[test]
    fn vanished_paths_are_excluded_from_live() {
        let mut m = Manifest::default();
        m.apply(ev("codex", "a.jsonl", Op::Seen, 1));
        m.apply(ev("codex", "b.jsonl", Op::Seen, 2));
        m.apply(ev("codex", "b.jsonl", Op::Vanished, 2));
        assert_eq!(m.live_paths("codex"), vec!["a.jsonl"]);
    }

    #[test]
    fn roundtrips_through_jsonl_with_flattened_fingerprint() {
        let e = ev("codex", "a.jsonl", Op::Appended, 42);
        let line = serde_json::to_string(&e).unwrap();
        assert!(line.contains(r#""op":"appended""#));
        assert!(line.contains(r#""size":42"#), "fingerprint should be flattened, not nested");
        let back: Event = serde_json::from_str(&line).unwrap();
        assert_eq!(back.fingerprint, e.fingerprint);
    }

    #[test]
    fn load_folds_logs_in_filename_order() {
        let tmp = std::env::temp_dir().join(format!("cs-manifest-{}", uuid::Uuid::new_v4()));
        let dir = tmp.join("manifest");
        std::fs::create_dir_all(&dir).unwrap();
        let line = |e: &Event| serde_json::to_string(e).unwrap();
        std::fs::write(dir.join("2026-06.jsonl"), line(&ev("codex", "a.jsonl", Op::Seen, 10))).unwrap();
        std::fs::write(dir.join("2026-07.jsonl"), line(&ev("codex", "a.jsonl", Op::Appended, 99))).unwrap();

        let m = Manifest::load(&tmp).unwrap();
        assert_eq!(m.get("codex", "a.jsonl").unwrap().fingerprint.size, 99);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn civil_dates_are_exact_including_leap_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(365), (1971, 1, 1)); // 1970 is not a leap year
        assert_eq!(civil_from_days(11016), (2000, 2, 29)); // leap day
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
    }

    #[test]
    fn period_names_logs_so_filename_order_is_chronological() {
        assert_eq!(period(0), "1970-01");
        assert_eq!(period(951_782_400_000), "2000-02");
        assert!(period(1_785_274_108_570) < period(1_788_000_000_000));
    }

    #[test]
    fn writer_appends_and_load_reads_it_back() {
        let tmp = std::env::temp_dir().join(format!("cs-mw-{}", uuid::Uuid::new_v4()));
        let w = ManifestWriter::new(&tmp).unwrap();

        let mut first = ev("codex", "a.jsonl", Op::Seen, 10);
        first.ts = 1_785_274_108_570; // 2026-07
        w.append(&[first]).unwrap();

        let mut second = ev("codex", "a.jsonl", Op::Appended, 20);
        second.ts = 1_785_274_109_570;
        w.append(&[second]).unwrap();

        let m = Manifest::load(&tmp).unwrap();
        assert_eq!(m.get("codex", "a.jsonl").unwrap().fingerprint.size, 20);
        assert!(tmp.join("manifest/2026-07.jsonl").is_file());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_batch_spanning_two_months_splits_across_logs() {
        let tmp = std::env::temp_dir().join(format!("cs-mw-{}", uuid::Uuid::new_v4()));
        let w = ManifestWriter::new(&tmp).unwrap();
        let mut june = ev("codex", "a.jsonl", Op::Seen, 1);
        june.ts = 1_780_000_000_000;
        let mut july = ev("codex", "b.jsonl", Op::Seen, 2);
        july.ts = 1_785_274_108_570;
        w.append(&[june.clone(), july.clone()]).unwrap();

        assert!(tmp.join(format!("manifest/{}.jsonl", period(june.ts))).is_file());
        assert!(tmp.join(format!("manifest/{}.jsonl", period(july.ts))).is_file());
        assert_eq!(Manifest::load(&tmp).unwrap().len(), 2);
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_truncated_last_line_is_survivable() {
        let tmp = std::env::temp_dir().join(format!("cs-manifest-{}", uuid::Uuid::new_v4()));
        let dir = tmp.join("manifest");
        std::fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&ev("codex", "a.jsonl", Op::Seen, 10)).unwrap();
        std::fs::write(dir.join("2026-07.jsonl"), format!("{good}\n{{\"ts\":1,\"op\":\"se")).unwrap();

        let m = Manifest::load(&tmp).unwrap();
        assert_eq!(m.len(), 1, "the intact line must still load");
        std::fs::remove_dir_all(&tmp).ok();
    }
}
