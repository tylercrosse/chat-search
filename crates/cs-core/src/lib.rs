//! Normalized model, index and search.
//!
//! `model` is the contract importers produce. `schema` and `index` turn that into
//! `index.db`. `search` queries it. Index and search are one crate on purpose: they share
//! the tokenizer, the schema and the ranking, and splitting them produces silent recall
//! bugs rather than compile errors.
//!
//! `time` holds what every client needs and no client should re-derive: the clock, and the
//! rule for naming the local day an instant fell on.

pub mod eval;
pub mod highlight;
pub mod index;
pub mod model;
pub mod querylog;
pub mod schema;
pub mod search;
pub mod time;

pub use eval::{Grade, Judged, QueryScore, Report};
pub use index::{
    built_by, ensure_current, open, open_fresh, reset, write_conversations, write_conversations_with, IndexOptions,
    IndexStats, TOOL_TEXT_MAX,
};
pub use model::{Conversation, Kind, Message, Role, Titles};
pub use schema::IMPORTER_VERSION;
pub use search::{
    explain, is_blank, match_density, recent, search, search_grouped, Explain, Field, Group, Hit,
    SearchOptions, DECAY, REPEAT_WEIGHT,
};
pub use time::{local_ymd, now_ms, ymd_in};
