//! Normalized model, index and search.
//!
//! `model` is the contract importers produce. `schema` and `index` turn that into
//! `index.db`. `search` queries it. Index and search are one crate on purpose: they share
//! the tokenizer, the schema and the ranking, and splitting them produces silent recall
//! bugs rather than compile errors.

pub mod index;
pub mod model;
pub mod schema;
pub mod search;

pub use index::{
    open, open_fresh, reset, write_conversations, write_conversations_with, IndexOptions,
    IndexStats, TOOL_TEXT_MAX,
};
pub use model::{Conversation, Kind, Message, Role, Titles};
pub use schema::IMPORTER_VERSION;
pub use search::{explain, search, search_grouped, Explain, Field, Group, Hit, Query};
