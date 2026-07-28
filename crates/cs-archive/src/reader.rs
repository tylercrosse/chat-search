use crate::error::{IoContext, Result};
use std::path::{Path, PathBuf};

/// Reads archived files back out, hiding physical layout from importers.
///
/// Today every source is mirrored, so this is a walk. When bundled sources land (ADR 18)
/// this is where unpacking happens, so an importer never learns whether its source was
/// mirrored or bundled and layout policy stays free to change.
pub struct ArchiveReader {
    machine_dir: PathBuf,
}

/// One archived file: the path it had in the *source*, and where it lives now.
#[derive(Debug, Clone)]
pub struct ArchivedFile {
    pub logical_path: String,
    pub archived_path: PathBuf,
}

impl ArchiveReader {
    pub fn new(machine_dir: impl Into<PathBuf>) -> Self {
        Self { machine_dir: machine_dir.into() }
    }

    pub fn machine_dir(&self) -> &Path {
        &self.machine_dir
    }

    /// Every archived file for a source, in a deterministic order.
    ///
    /// Sorted on the string form of the logical path: `Path`'s own ordering is
    /// component-wise, which puts `<id>/subagents/x.jsonl` before `<id>.jsonl` while a
    /// string compare does the opposite, and that difference silently changes which file
    /// wins a metadata merge (ADR 7).
    pub fn files(&self, source_id: &str) -> Result<Vec<ArchivedFile>> {
        let root = self.machine_dir.join(source_id);
        let mut out = Vec::new();
        if !root.is_dir() {
            return Ok(out);
        }
        let mut paths = Vec::new();
        walk(&root, &mut paths);
        for p in paths {
            if let Ok(rel) = p.strip_prefix(&root) {
                let logical = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push(ArchivedFile { logical_path: logical, archived_path: p });
            }
        }
        out.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
        Ok(out)
    }

    pub fn read(&self, file: &ArchivedFile) -> Result<Vec<u8>> {
        std::fs::read(&file.archived_path).at(&file.archived_path)
    }
}

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

    #[test]
    fn files_are_returned_in_string_sorted_logical_order() {
        let tmp = std::env::temp_dir().join(format!("cs-reader-{}", uuid::Uuid::new_v4()));
        let src = tmp.join("codex");
        std::fs::create_dir_all(src.join("s/subagents")).unwrap();
        std::fs::write(src.join("s.jsonl"), b"a").unwrap();
        std::fs::write(src.join("s/subagents/x.jsonl"), b"b").unwrap();

        let r = ArchiveReader::new(&tmp);
        let got: Vec<_> = r.files("codex").unwrap().into_iter().map(|f| f.logical_path).collect();
        assert_eq!(got, vec!["s.jsonl", "s/subagents/x.jsonl"]);
        assert_eq!(r.files("nonexistent").unwrap().len(), 0);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
