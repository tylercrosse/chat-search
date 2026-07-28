//! Importers: archived bytes -> `cs_core::Conversation`.
//!
//! One module per source *format*. Each is a pure function over bytes: no database, no
//! search, and no knowledge of where the bytes came from. That is what makes them cheap to
//! test with golden fixtures and what makes retroactive reparse possible (ADR 1).

pub mod chatgpt_export;
pub mod claude_code;
pub mod codex;
