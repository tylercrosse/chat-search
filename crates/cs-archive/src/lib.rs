//! Capture side of chat-search: copy raw transcript bytes into an immutable archive
//! and record what was observed.
//!
//! This crate deliberately never parses transcript content (ADR 16). It knows paths,
//! sizes, mtimes, hashes and per-source layout policy — nothing about conversations or
//! messages. Interpreting bytes is the importer's job, and keeping the two apart is what
//! makes retroactive reparse possible.

pub mod capture;
pub mod config;
pub mod drift;
pub mod error;
pub mod machine;
pub mod manifest;
pub mod reader;
pub mod scan;
pub mod staleness;
pub mod throttle;
pub mod unreachable;

pub use capture::{capture_file, CaptureKind};
pub use config::{Config, Layout, Source};
pub use drift::Drift;
pub use error::{Error, Result};
pub use staleness::Stale;
pub use unreachable::Surface;
pub use machine::Machine;
pub use reader::{ArchivedFile, ArchiveReader};
pub use manifest::{Event, Fingerprint, Manifest, ManifestWriter, Op};
pub use scan::{scan_source, Change, FileChange, SourceScan};
