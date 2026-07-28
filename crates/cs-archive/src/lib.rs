//! Capture side of chat-search: copy raw transcript bytes into an immutable archive
//! and record what was observed.
//!
//! This crate deliberately never parses transcript content (ADR 16). It knows paths,
//! sizes, mtimes, hashes and per-source layout policy — nothing about conversations or
//! messages. Interpreting bytes is the importer's job, and keeping the two apart is what
//! makes retroactive reparse possible.

pub mod config;
pub mod error;
pub mod machine;
pub mod manifest;
pub mod scan;

pub use config::{Config, Layout, Source};
pub use error::{Error, Result};
pub use machine::Machine;
pub use manifest::{Event, Fingerprint, Manifest, ManifestWriter, Op};
pub use scan::{scan_source, Change, FileChange, SourceScan};
