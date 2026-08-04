//! Normalized model, index and search.
//!
//! `model` is the contract importers produce. `schema` and `index` turn that into
//! `index.db`. `search` queries it. Index and search are one crate on purpose: they share
//! the tokenizer, the schema and the ranking, and splitting them produces silent recall
//! bugs rather than compile errors.
//!
//! `time` holds what every client needs and no client should re-derive: the clock, and the
//! rule for naming the local day an instant fell on. `blocks` is the same idea one level up:
//! which messages a reader draws and which matches may claim to have ranked the conversation,
//! answered once for the TUI, `cs show --json`, and everything downstream of that JSON.

pub mod blocks;
pub mod destination;
pub mod eval;
pub mod highlight;
pub mod index;
pub mod model;
pub mod query;
pub mod querylog;
pub mod schema;
pub mod search;
pub mod time;

pub use blocks::{Block, Density, Fold, MarkKind, Transcript, WireBlock};
pub use destination::{destinations, Destination};
pub use eval::{Grade, Judged, QueryScore, Report};
pub use index::{
    built_by, ensure_current, open, open_fresh, reset, write_conversations, write_conversations_with, IndexOptions,
    IndexStats, TOOL_TEXT_MAX,
};
pub use model::{Conversation, Kind, Message, Role, Titles};
pub use query::{Age, DateSpec, Facet, Filter, FilterKind, Mode, Query, Selection, Window};
pub use schema::IMPORTER_VERSION;
pub use search::{
    count_matching, explain, match_density, recent, search, search_grouped, search_grouped_counted,
    snippet_marked, Counted, Explain, Field, Group, Hit, SearchOptions, Total, DECAY,
    REPEAT_WEIGHT,
};
pub use time::{
    day_start_in, local_day_start, local_ymd, now_ms, shift_days_in, shift_months_in, ymd_in,
};
