use crate::config::Source;
use crate::error::{IoContext, Result};
use crate::manifest::{Fingerprint, Manifest, Op};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

/// How much of a file's head identifies it. Enough to catch a rewrite; small enough that
/// a scan costs kilobytes per changed file rather than gigabytes (ADR 19).
pub const PREFIX_MAX: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Size and mtime match the last observation — not even hashed.
    Unchanged,
    /// Never captured, or captured and later vanished and now back.
    New,
    /// Grew with its prefix intact. The overwhelmingly common case.
    Appended,
    /// Prefix changed or the file shrank — compaction or in-place edit.
    Rewritten,
    /// Recorded before, absent now.
    Vanished,
}

impl Change {
    /// Whether this change requires copying bytes into the archive.
    pub fn needs_capture(self) -> bool {
        matches!(self, Change::New | Change::Appended | Change::Rewritten)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Change::Unchanged => "unchanged",
            Change::New => "new",
            Change::Appended => "appended",
            Change::Rewritten => "rewritten",
            Change::Vanished => "vanished",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path relative to the source root, always `/`-separated.
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub change: Change,
    /// `None` only for `Vanished`, where there is no longer a file to fingerprint.
    pub fingerprint: Option<Fingerprint>,
}

#[derive(Debug)]
pub struct SourceScan {
    pub source: String,
    pub changes: Vec<FileChange>,
}

impl SourceScan {
    pub fn count(&self, change: Change) -> usize {
        self.changes.iter().filter(|c| c.change == change).count()
    }
}

/// Compare a source directory against the manifest and classify every file.
///
/// Reads nothing when size and mtime are unchanged, so a steady-state scan is stat-only.
pub fn scan_source(source: &Source, manifest: &Manifest) -> Result<SourceScan> {
    let matcher = build_matcher(&source.include)?;

    let mut files = Vec::new();
    walk(&source.path, &mut files);
    // ADR 7: sort on the string form. Path's own Ord compares component-wise, which orders
    // `<id>/subagents/x.jsonl` before `<id>.jsonl` while a string compare does the opposite.
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let mut changes = Vec::new();
    let mut present: HashSet<String> = HashSet::new();

    for abs in files {
        let Some(rel) = relative(&source.path, &abs) else { continue };
        if !matcher.is_match(&rel) {
            continue;
        }
        present.insert(rel.clone());

        let meta = std::fs::metadata(&abs).at(&abs)?;
        let size = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let prior = manifest.get(&source.id, &rel).filter(|e| e.op != Op::Vanished);

        // Fast path: identical size and mtime means identical content in practice, and
        // skipping the read is what keeps a no-op scan cheap over ~1,900 files.
        if let Some(p) = prior {
            if p.fingerprint.size == size && p.fingerprint.mtime_ms == mtime_ms {
                changes.push(FileChange {
                    rel_path: rel,
                    abs_path: abs,
                    change: Change::Unchanged,
                    fingerprint: Some(p.fingerprint.clone()),
                });
                continue;
            }
        }

        let (change, fingerprint) = classify(&abs, size, mtime_ms, prior.map(|p| &p.fingerprint))?;
        changes.push(FileChange {
            rel_path: rel,
            abs_path: abs,
            change,
            fingerprint: Some(fingerprint),
        });
    }

    // Anything the manifest still considers live but that the walk did not find is gone.
    for rel in manifest.live_paths(&source.id) {
        if !present.contains(rel) {
            changes.push(FileChange {
                rel_path: rel.to_string(),
                abs_path: source.path.join(rel),
                change: Change::Vanished,
                fingerprint: None,
            });
        }
    }

    Ok(SourceScan { source: source.id.clone(), changes })
}

/// Read the head of the file once, and use it to answer two questions: does the *previously
/// recorded* prefix still hash the same, and what is the prefix now?
fn classify(
    path: &Path,
    size: u64,
    mtime_ms: u64,
    prior: Option<&Fingerprint>,
) -> Result<(Change, Fingerprint)> {
    let want = size.min(PREFIX_MAX);
    let mut head = Vec::with_capacity(want as usize);
    std::fs::File::open(path)
        .at(path)?
        .take(want)
        .read_to_end(&mut head)
        .at(path)?;

    let fingerprint = Fingerprint {
        size,
        mtime_ms,
        prefix_hash: blake3::hash(&head).to_hex().to_string(),
        prefix_len: head.len() as u64,
    };

    let change = match prior {
        None => Change::New,
        Some(p) => {
            let recorded = p.prefix_len as usize;
            if size < p.size || head.len() < recorded {
                // Shrank, or there is no longer enough head to re-verify the old prefix.
                Change::Rewritten
            } else if blake3::hash(&head[..recorded]).to_hex().to_string() == p.prefix_hash {
                if size == p.size {
                    Change::Unchanged // touched but not modified
                } else {
                    Change::Appended
                }
            } else {
                Change::Rewritten
            }
        }
    };

    Ok((change, fingerprint))
}

/// Empty patterns means "every file". Patterns are matched against the source-relative path.
fn build_matcher(patterns: &[String]) -> Result<GlobSet> {
    if patterns.is_empty() {
        return Ok(GlobSetBuilder::new()
            .add(Glob::new("**").expect("literal glob"))
            .build()
            .expect("literal globset"));
    }
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        // A bad pattern is a config error, but failing the whole scan over one typo would
        // stop capture entirely — which is worse than over-matching. Skip and continue.
        if let Ok(g) = Glob::new(p) {
            builder.add(g);
        }
    }
    Ok(builder.build().unwrap_or_else(|_| {
        GlobSetBuilder::new().add(Glob::new("**").unwrap()).build().unwrap()
    }))
}

fn relative(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    Some(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

/// Recursive walk that does not follow symlinks: `DirEntry::file_type` does not traverse
/// them, so a symlinked directory is neither descended nor mistaken for a file. That keeps
/// a stray link from pulling unrelated trees into the archive.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => walk(&path, out),
            Ok(ft) if ft.is_file() => out.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Layout;
    use crate::manifest::Event;

    struct Fixture {
        root: PathBuf,
        source: Source,
    }

    impl Fixture {
        fn new(include: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!("cs-scan-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let source = Source {
                id: "codex".into(),
                path: root.clone(),
                layout: Layout::Mirror,
                include: include.iter().map(|s| s.to_string()).collect(),
            };
            Fixture { root, source }
        }
        fn write(&self, rel: &str, body: &str) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        fn append(&self, rel: &str, body: &str) {
            use std::io::Write;
            let p = self.root.join(rel);
            let existing = std::fs::read_to_string(&p).unwrap();
            // rewrite wholesale so mtime definitely moves
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(existing.as_bytes()).unwrap();
            f.write_all(body.as_bytes()).unwrap();
        }
        fn scan(&self, m: &Manifest) -> SourceScan {
            scan_source(&self.source, m).unwrap()
        }
        fn commit(&self, m: &mut Manifest, s: &SourceScan) {
            for c in &s.changes {
                let op = match c.change {
                    Change::New => Op::Seen,
                    Change::Appended => Op::Appended,
                    Change::Rewritten => Op::Rewritten,
                    Change::Vanished => Op::Vanished,
                    Change::Unchanged => continue,
                };
                m.apply(Event {
                    ts: 0,
                    op,
                    source: self.source.id.clone(),
                    path: c.rel_path.clone(),
                    fingerprint: c.fingerprint.clone().unwrap_or(Fingerprint {
                        size: 0,
                        mtime_ms: 0,
                        prefix_hash: String::new(),
                        prefix_len: 0,
                    }),
                });
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    #[test]
    fn lifecycle_new_then_appended_then_unchanged() {
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("2026/07/a.jsonl", "line one\n");
        let mut m = Manifest::default();

        let s = f.scan(&m);
        assert_eq!(s.count(Change::New), 1);
        f.commit(&mut m, &s);

        let s = f.scan(&m);
        assert_eq!(s.count(Change::Unchanged), 1, "a second scan must not re-capture");

        f.append("2026/07/a.jsonl", "line two\n");
        let s = f.scan(&m);
        assert_eq!(s.count(Change::Appended), 1);
    }

    #[test]
    fn rewriting_the_head_is_detected_even_when_the_file_grows() {
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("a.jsonl", "original head\n");
        let mut m = Manifest::default();
        let s = f.scan(&m);
        f.commit(&mut m, &s);

        f.write("a.jsonl", "DIFFERENT head\nplus more content\n");
        let s = f.scan(&m);
        assert_eq!(s.count(Change::Rewritten), 1);
    }

    #[test]
    fn a_small_file_that_grows_is_appended_not_rewritten() {
        // The prefix_len guard: without it, hashing "first 64 KiB" of a 10-byte file that
        // grew would cover new bytes and look like a rewrite.
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("a.jsonl", "tiny\n");
        let mut m = Manifest::default();
        let s = f.scan(&m);
        f.commit(&mut m, &s);

        f.append("a.jsonl", "much more content appended after the fact\n");
        let s = f.scan(&m);
        assert_eq!(s.count(Change::Appended), 1, "growth must not read as rewrite");
    }

    #[test]
    fn shrinking_is_a_rewrite() {
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("a.jsonl", "aaaaaaaaaaaaaaaaaaaa\n");
        let mut m = Manifest::default();
        let s = f.scan(&m);
        f.commit(&mut m, &s);

        f.write("a.jsonl", "aaa\n");
        assert_eq!(f.scan(&m).count(Change::Rewritten), 1);
    }

    #[test]
    fn deleting_a_file_reports_vanished_once() {
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("a.jsonl", "x\n");
        let mut m = Manifest::default();
        let s = f.scan(&m);
        f.commit(&mut m, &s);

        std::fs::remove_file(f.root.join("a.jsonl")).unwrap();
        let s = f.scan(&m);
        assert_eq!(s.count(Change::Vanished), 1);
        f.commit(&mut m, &s);

        let s = f.scan(&m);
        assert_eq!(s.count(Change::Vanished), 0, "vanished must not repeat every scan");
    }

    #[test]
    fn a_file_that_comes_back_is_new_again() {
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("a.jsonl", "x\n");
        let mut m = Manifest::default();
        let s = f.scan(&m);
        f.commit(&mut m, &s);
        std::fs::remove_file(f.root.join("a.jsonl")).unwrap();
        let s = f.scan(&m);
        f.commit(&mut m, &s);

        f.write("a.jsonl", "x\n");
        assert_eq!(f.scan(&m).count(Change::New), 1);
    }

    #[test]
    fn include_globs_filter_and_double_star_matches_at_root() {
        let f = Fixture::new(&["**/rollout-*.jsonl"]);
        f.write("rollout-top.jsonl", "a\n"); // at the root, no directory prefix
        f.write("2026/07/rollout-nested.jsonl", "b\n");
        f.write("2026/07/other.jsonl", "c\n");
        f.write("notes.txt", "d\n");

        let s = f.scan(&Manifest::default());
        let mut got: Vec<_> = s.changes.iter().map(|c| c.rel_path.as_str()).collect();
        got.sort_unstable();
        assert_eq!(got, vec!["2026/07/rollout-nested.jsonl", "rollout-top.jsonl"]);
    }

    #[test]
    fn empty_include_matches_everything() {
        let f = Fixture::new(&[]);
        f.write("a.jsonl", "a\n");
        f.write("b.txt", "b\n");
        assert_eq!(f.scan(&Manifest::default()).changes.len(), 2);
    }

    #[test]
    fn walk_order_is_deterministic_and_string_sorted() {
        let f = Fixture::new(&["**/*.jsonl"]);
        f.write("s.jsonl", "a\n");
        f.write("s/subagents/x.jsonl", "b\n");
        let s = f.scan(&Manifest::default());
        let got: Vec<_> = s.changes.iter().map(|c| c.rel_path.as_str()).collect();
        // '.' (0x2E) sorts before '/' (0x2F): the file precedes the directory.
        assert_eq!(got, vec!["s.jsonl", "s/subagents/x.jsonl"]);
    }
}
