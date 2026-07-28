use crate::error::{IoContext, Result};
use crate::scan::Change;
use std::path::{Path, PathBuf};

/// How the bytes actually got into the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    /// APFS `clonefile` / Linux `FICLONE`: shares blocks with the source, so it costs
    /// almost nothing until one side diverges (ADR 20).
    Cloned,
    /// Same volume was not available, or the filesystem cannot reflink. Real bytes copied.
    Copied,
}

impl CaptureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Cloned => "cloned",
            CaptureKind::Copied => "copied",
        }
    }
}

/// Where an archived file lives: `<machine_dir>/<source_id>/<rel_path>`.
pub fn dest_path(machine_dir: &Path, source_id: &str, rel_path: &str) -> PathBuf {
    let mut p = machine_dir.join(source_id);
    for segment in rel_path.split('/') {
        p.push(segment);
    }
    p
}

/// Where a superseded copy is parked when a source file is rewritten rather than appended.
fn superseded_path(machine_dir: &Path, source_id: &str, rel_path: &str, ts: u64) -> PathBuf {
    let mut p = machine_dir.join("_superseded").join(source_id);
    for segment in rel_path.split('/') {
        p.push(segment);
    }
    let name = format!(
        "{}.{ts}",
        p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
    );
    p.set_file_name(name);
    p
}

/// Copy one source file into the archive.
///
/// `Appended` overwrites, which is safe because the new file is a strict superset — the
/// scanner verified the prefix still hashes the same. `Rewritten` is *not* a superset, so
/// the existing archived copy is moved under `_superseded/` before being replaced;
/// discarding it would silently lose the pre-compaction history (ADR 18).
pub fn capture_file(
    machine_dir: &Path,
    source_id: &str,
    rel_path: &str,
    src: &Path,
    change: Change,
    now_ms: u64,
) -> Result<CaptureKind> {
    debug_assert!(change.needs_capture(), "capture_file called for {change:?}");

    let dest = dest_path(machine_dir, source_id, rel_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }

    if change == Change::Rewritten && dest.exists() {
        let parked = superseded_path(machine_dir, source_id, rel_path, now_ms);
        if let Some(parent) = parked.parent() {
            std::fs::create_dir_all(parent).at(parent)?;
        }
        std::fs::rename(&dest, &parked).at(&parked)?;
    }

    // clonefile refuses to overwrite, so the destination has to go first. This is the one
    // moment the archive is missing a file; the manifest event is written only after the
    // copy succeeds, so a crash here is re-detected as a change on the next scan.
    if dest.exists() {
        std::fs::remove_file(&dest).at(&dest)?;
    }

    match reflink_copy::reflink_or_copy(src, &dest).at(src)? {
        None => Ok(CaptureKind::Cloned),
        Some(_) => Ok(CaptureKind::Copied),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cs-capture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn new_file_lands_at_the_mirrored_path() {
        let root = tmp();
        let src = root.join("src.jsonl");
        std::fs::write(&src, "hello\n").unwrap();
        let machine = root.join("machine");

        let kind = capture_file(&machine, "codex", "2026/07/a.jsonl", &src, Change::New, 1).unwrap();
        let dest = machine.join("codex/2026/07/a.jsonl");
        assert!(dest.is_file());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello\n");
        // On APFS this should clone; anywhere else the fallback still has to work.
        assert!(matches!(kind, CaptureKind::Cloned | CaptureKind::Copied));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn append_overwrites_in_place_without_parking_a_copy() {
        let root = tmp();
        let src = root.join("src.jsonl");
        let machine = root.join("machine");
        std::fs::write(&src, "one\n").unwrap();
        capture_file(&machine, "codex", "a.jsonl", &src, Change::New, 1).unwrap();

        std::fs::write(&src, "one\ntwo\n").unwrap();
        capture_file(&machine, "codex", "a.jsonl", &src, Change::Appended, 2).unwrap();

        assert_eq!(std::fs::read_to_string(machine.join("codex/a.jsonl")).unwrap(), "one\ntwo\n");
        assert!(!machine.join("_superseded").exists(), "append must not park a copy");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rewrite_parks_the_previous_copy_instead_of_discarding_it() {
        let root = tmp();
        let src = root.join("src.jsonl");
        let machine = root.join("machine");
        std::fs::write(&src, "original\n").unwrap();
        capture_file(&machine, "codex", "2026/a.jsonl", &src, Change::New, 1).unwrap();

        std::fs::write(&src, "replaced\n").unwrap();
        capture_file(&machine, "codex", "2026/a.jsonl", &src, Change::Rewritten, 777).unwrap();

        assert_eq!(std::fs::read_to_string(machine.join("codex/2026/a.jsonl")).unwrap(), "replaced\n");
        let parked = machine.join("_superseded/codex/2026/a.jsonl.777");
        assert!(parked.is_file(), "pre-rewrite content must survive at {}", parked.display());
        assert_eq!(std::fs::read_to_string(&parked).unwrap(), "original\n");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn clone_is_independent_of_later_source_changes() {
        // The property the whole archive depends on: once captured, the archived bytes are
        // not affected by anything that happens to the source afterwards.
        let root = tmp();
        let src = root.join("src.jsonl");
        let machine = root.join("machine");
        std::fs::write(&src, "captured\n").unwrap();
        capture_file(&machine, "codex", "a.jsonl", &src, Change::New, 1).unwrap();

        std::fs::write(&src, "changed entirely\n").unwrap();
        std::fs::remove_file(&src).unwrap();

        assert_eq!(std::fs::read_to_string(machine.join("codex/a.jsonl")).unwrap(), "captured\n");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn superseded_names_do_not_collide_across_rewrites() {
        let a = superseded_path(Path::new("/m"), "codex", "x/y.jsonl", 100);
        let b = superseded_path(Path::new("/m"), "codex", "x/y.jsonl", 200);
        assert_ne!(a, b);
        assert_eq!(a, PathBuf::from("/m/_superseded/codex/x/y.jsonl.100"));
    }
}
