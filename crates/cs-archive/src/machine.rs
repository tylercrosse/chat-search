use crate::error::{Error, IoContext, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Machine identity (ADR 17). `id` is canonical and never changes; `alias` is cosmetic
/// and is taken from the directory name, so renaming the directory renames the machine
/// while `id` proves continuity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub alias: String,
    pub created_at_ms: u64,
    pub hostname_at_creation: String,
}

impl Machine {
    /// `<archive_root>/raw/<alias>` — everything this machine captures lives here.
    pub fn dir(&self, archive_root: &Path) -> PathBuf {
        archive_root.join("raw").join(&self.alias)
    }
}

/// Find this archive's machine identity, creating it on first run.
///
/// Exactly one identity is expected under `<archive_root>/raw/`. More than one means two
/// machines were merged into a single archive root, which the caller has to resolve —
/// silently picking one would scatter captures across both namespaces.
pub fn load_or_create(archive_root: &Path, alias_override: Option<&str>) -> Result<Machine> {
    let raw = archive_root.join("raw");
    let mut found: Vec<PathBuf> = Vec::new();
    if raw.is_dir() {
        for entry in std::fs::read_dir(&raw).at(&raw)? {
            let entry = entry.at(&raw)?;
            let candidate = entry.path().join(".machine.json");
            if candidate.is_file() {
                found.push(candidate);
            }
        }
    }
    found.sort();

    match found.len() {
        0 => create(archive_root, alias_override),
        1 => {
            let path = &found[0];
            let text = std::fs::read_to_string(path).at(path)?;
            let mut machine: Machine = serde_json::from_str(&text)
                .map_err(|source| Error::Json { path: path.clone(), source })?;
            // Trust the directory name over the stored alias, so renaming the directory
            // is a supported way to rename the machine.
            if let Some(dir_name) = path.parent().and_then(|p| p.file_name()) {
                machine.alias = dir_name.to_string_lossy().into_owned();
            }
            Ok(machine)
        }
        count => Err(Error::AmbiguousMachine { root: archive_root.to_path_buf(), count }),
    }
}

fn create(archive_root: &Path, alias_override: Option<&str>) -> Result<Machine> {
    let hostname = hostname();
    // slugify() is called inside each arm rather than once outside: the `None` arm builds
    // a temporary String, and borrowing it past the end of the match would not compile.
    let alias = match alias_override {
        Some(a) => slugify(a),
        None => slugify(&friendly_name().unwrap_or_else(|| hostname.clone())),
    };

    let machine = Machine {
        id: uuid::Uuid::new_v4().to_string(),
        alias,
        created_at_ms: now_ms(),
        hostname_at_creation: hostname,
    };

    let dir = machine.dir(archive_root);
    std::fs::create_dir_all(&dir).at(&dir)?;
    let path = dir.join(".machine.json");
    let body = serde_json::to_vec_pretty(&machine)
        .map_err(|source| Error::Json { path: path.clone(), source })?;
    std::fs::write(&path, body).at(&path)?;
    Ok(machine)
}

/// The user-facing machine name. macOS keeps a nicer one than the hostname —
/// "Tyler's MacBook Air" rather than "Tylers-MacBook-Air.local".
fn friendly_name() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !name.is_empty()).then_some(name)
}

fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Lowercase, alphanumeric, dash-separated. Apostrophes are dropped rather than turned
/// into separators, so "Tyler's MacBook Air" becomes `tylers-macbook-air`.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in s.chars() {
        match c {
            '\'' | '\u{2019}' => continue,
            c if c.is_ascii_alphanumeric() => {
                if pending_dash && !out.is_empty() {
                    out.push('-');
                }
                pending_dash = false;
                out.push(c.to_ascii_lowercase());
            }
            _ => pending_dash = true,
        }
    }
    if out.is_empty() {
        "machine".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_handles_real_mac_names() {
        assert_eq!(slugify("Tyler's MacBook Air"), "tylers-macbook-air");
        assert_eq!(slugify("Tyler\u{2019}s MacBook Air"), "tylers-macbook-air");
        assert_eq!(slugify("Mac Studio (work)"), "mac-studio-work");
        assert_eq!(slugify("Tylers-MacBook-Air.local"), "tylers-macbook-air-local");
        assert_eq!(slugify("  "), "machine");
        assert_eq!(slugify("MBA"), "mba");
    }

    #[test]
    fn create_then_load_is_stable() {
        let tmp = std::env::temp_dir().join(format!("cs-machine-{}", uuid::Uuid::new_v4()));
        let a = load_or_create(&tmp, Some("test box")).unwrap();
        assert_eq!(a.alias, "test-box");
        let b = load_or_create(&tmp, None).unwrap();
        assert_eq!(a.id, b.id, "id must survive reload");
        assert_eq!(b.alias, "test-box");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn renaming_the_directory_renames_the_machine_but_keeps_the_id() {
        let tmp = std::env::temp_dir().join(format!("cs-machine-{}", uuid::Uuid::new_v4()));
        let a = load_or_create(&tmp, Some("before")).unwrap();
        std::fs::rename(tmp.join("raw/before"), tmp.join("raw/after")).unwrap();
        let b = load_or_create(&tmp, None).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.alias, "after");
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn two_identities_is_an_error_not_a_guess() {
        let tmp = std::env::temp_dir().join(format!("cs-machine-{}", uuid::Uuid::new_v4()));
        load_or_create(&tmp, Some("one")).unwrap();
        let second = tmp.join("raw/two");
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join(".machine.json"), r#"{"id":"x","alias":"two","created_at_ms":0,"hostname_at_creation":"h"}"#).unwrap();
        assert!(matches!(load_or_create(&tmp, None), Err(Error::AmbiguousMachine { .. })));
        std::fs::remove_dir_all(&tmp).ok();
    }
}
