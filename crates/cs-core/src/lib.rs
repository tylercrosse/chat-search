//! Normalized model, index and search.
//!
//! `model` is the contract importers produce. `schema` and `index` turn that into
//! `index.db`. `search` queries it. Index and search are one crate on purpose: they share
//! the tokenizer, the schema and the ranking, and splitting them produces silent recall
//! bugs rather than compile errors.
//!
//! `build` owns the file rather than its contents: how a rebuild is swapped in whole, and
//! what a reader is allowed to conclude from what it finds at the path.
//!
//! `time` holds what every client needs and no client should re-derive: the clock, and the
//! rule for naming the local day an instant fell on. `blocks` is the same idea one level up:
//! which messages a reader draws and which matches may claim to have ranked the conversation,
//! answered once for the TUI, `cs show --json`, and everything downstream of that JSON.
//!
//! `answer` is `blocks` for the other half of the interface: the reply to a search, owned here
//! so that clients adapt to it rather than each assembling one (ADR 23). `search` ranks; the
//! envelope, its totals and its routing are `answer`'s.

pub mod answer;
pub mod blocks;
pub mod build;
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

// `Group` at the root is the answer's, now that no client reads the ranked row directly. The two
// are one `From` apart, and the name belongs to the one clients decode: `search::Group` is the
// ranking's own row, on its way to becoming this one.
pub use answer::{answer, Answer, FlatAnswer, Group, Match, Reason, Refusal};
pub use blocks::{Block, Density, Fold, MarkKind, Transcript, WireBlock};
pub use build::{open_for_read, IndexBuild, IndexState, Reader, Unreadable};
pub use destination::{destinations, Destination};
pub use eval::{Grade, Judged, QueryScore, Report};
pub use index::{
    built_by, ensure_current, open, record_importer_version, reset, write_conversations,
    write_conversations_with, IndexOptions, IndexStats, TOOL_TEXT_MAX,
};
pub use model::{Conversation, Kind, Message, Role, Titles};
pub use query::{Age, DateSpec, Facet, Filter, FilterKind, Mode, Query, Selection, Window};
pub use schema::IMPORTER_VERSION;
pub use search::{
    count_matching, explain, match_density, recent, search, search_grouped, search_grouped_counted,
    snippet_marked, Counted, Explain, Field, Hit, SearchOptions, Total, DECAY, REPEAT_WEIGHT,
};
pub use time::{
    day_start_in, local_day_start, local_ymd, now_ms, shift_days_in, shift_months_in, ymd_in,
};
