//! The facet rail, as a value a client can draw without parsing anything.
//!
//! docs/TUI-DESIGN.md §5 settled what a facet bar is: a *projection of the query text*, never a
//! selection kept beside it. The TUI reaches that by calling [`Query::selection`] to light its
//! chips and [`Query::toggling`] to rewrite the text when one is clicked, both in this crate,
//! "because the grammar is". A client in another process cannot call either — and a Swift or
//! JavaScript surface that assembled `agent:codex` itself would be the second, partial parser
//! §5 costs out at six reconciliation methods in the tool it was lifted from.
//!
//! So the projection is computed here, once, and handed over whole: each chip arrives already
//! carrying what the query says about it *and* the query text that clicking it produces. A
//! client draws the rail and, on a click, puts a string it was given into its input box. There
//! is nothing left for it to parse and nothing for it to keep in step.
//!
//! **Facts in, one answer out**, the same arrangement as [`crate::inventory`] and for the same
//! reason: this crate reads no config, so the caller supplies the census and gets the join.
//!
//! Only the `agent:` facet has a rail today. `dir:` needs a corpus-true project list
//! (`chat-search-6eb.26`) and `date:` needs a toggling rule of its own — its tokens intersect
//! rather than union, so [`Query::toggling`] is the wrong verb for it — and both are additive
//! keys on the reply when they land. What is *not* deferred is that both already filter: they
//! are part of the grammar, they are applied, and a client with an input box can type them.

use serde::Serialize;

use crate::inventory::SourceCoverage;
use crate::query::{Facet, Query};

/// What the query says about one chip. Three states, because the query has three things to say
/// about a source, and an excluded one drawn like an untouched one is filtering the reader
/// cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChipState {
    /// `agent:codex` — the query keeps this source.
    Include,
    /// `-agent:codex` or `agent:!codex` — the query drops it.
    Exclude,
    /// Neither. Not the same as being included by a query with no `agent:` in it at all: that
    /// is every chip `off` and [`SourceFacet::all`] selected.
    Off,
}

/// One source chip: the census fact, the query fact, and the click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceChip {
    /// The source id, which is permanent and is what `agent:` selects on (ADR 16).
    pub value: String,
    pub state: ChipState,
    /// Where this source stands with the config and this disk — [`crate::Coverage::as_str`].
    /// Carried because the rail is the only place coverage is visible: a configured source
    /// holding nothing is a broken importer, and drawn from the index alone it is invisible.
    pub coverage: &'static str,
    /// Conversations the index holds for it. Zero is an answer, read beside `coverage`.
    pub conversations: i64,
    /// The whole query text after clicking this chip. Not a token to splice in — the rewrite
    /// widens an existing `agent:`, drops a standing exclusion and leaves the free text where
    /// it was, and none of that is a client's to reproduce.
    pub query: String,
}

/// The All chip: no `agent:` token at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AllChip {
    /// True when the query names no source, which is the only state in which every source is
    /// in the answer.
    pub selected: bool,
    /// The query text with every `agent:` token gone, exclusions included.
    pub query: String,
}

/// The `agent:` rail: the All chip, then one chip per source the machine knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceFacet {
    /// The keyword these chips write, so a client can name the facet in its own prose without
    /// hard-coding the grammar it is a rail for.
    pub keyword: &'static str,
    pub all: AllChip,
    pub values: Vec<SourceChip>,
}

/// The rail for one query and one census.
///
/// Every source the caller supplies gets a chip, in the order the census is in — which
/// [`crate::inventory::join`] sorts by id, so the rail does not reshuffle between runs.
///
/// Selection is decided by equality against the *lowercased* value, because that is what
/// `search`'s SQL compares `conversation.source` to: a chip whose id the filter would miss must
/// not claim to be on. A source id that is not already lowercase therefore draws off and stays
/// off, which is visible rather than silent — and `Config::validate` does not admit one today.
pub fn sources(query: &Query, census: &[SourceCoverage]) -> SourceFacet {
    let selection = query.selection(Facet::Agent);
    let values = census
        .iter()
        .map(|source| SourceChip {
            state: if selection.include.contains(&source.id) {
                ChipState::Include
            } else if selection.exclude.contains(&source.id) {
                ChipState::Exclude
            } else {
                ChipState::Off
            },
            query: query.toggling(Facet::Agent, &source.id),
            value: source.id.clone(),
            coverage: source.coverage.as_str(),
            conversations: source.conversations,
        })
        .collect();

    SourceFacet {
        keyword: Facet::Agent.keyword(),
        all: AllChip { selected: selection.is_empty(), query: query.without(Facet::Agent) },
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::Coverage;

    fn census() -> Vec<SourceCoverage> {
        vec![
            SourceCoverage {
                id: "chatgpt-export".into(),
                coverage: Coverage::Retired,
                conversations: 2011,
            },
            SourceCoverage {
                id: "claude-code".into(),
                coverage: Coverage::Live,
                conversations: 812,
            },
            SourceCoverage { id: "codex".into(), coverage: Coverage::Live, conversations: 236 },
            SourceCoverage {
                id: "gemini-cli".into(),
                coverage: Coverage::Unconfigured,
                conversations: 0,
            },
        ]
    }

    fn rail(text: &str) -> SourceFacet {
        sources(&Query::typeahead(text), &census())
    }

    fn chip<'a>(rail: &'a SourceFacet, value: &str) -> &'a SourceChip {
        rail.values.iter().find(|c| c.value == value).expect("every source gets a chip")
    }

    #[test]
    fn a_chip_is_lit_by_the_query_text_and_by_nothing_else() {
        // The whole point of §5 in one assertion: there is no state to set, so the rail cannot
        // disagree with the box. Typing the token by hand lights the chip.
        let rail = rail("borrow checker agent:codex");
        assert_eq!(chip(&rail, "codex").state, ChipState::Include);
        assert_eq!(chip(&rail, "claude-code").state, ChipState::Off);
        assert!(!rail.all.selected, "a query naming a source is not All");
    }

    #[test]
    fn an_excluded_source_is_not_drawn_like_an_untouched_one() {
        // Both spellings of "not", because both reach the same SQL and a chip that showed only
        // one of them would be filtering the reader cannot see.
        for text in ["-agent:codex", "agent:!codex"] {
            assert_eq!(chip(&rail(text), "codex").state, ChipState::Exclude, "{text}");
        }
    }

    #[test]
    fn clicking_a_chip_hands_back_a_whole_query_rather_than_a_token_to_splice() {
        // The client is given a string to put in the box. It never learns that a second source
        // widens an existing token rather than adding a second one — that rule is the parser's.
        let first = chip(&rail("rust"), "codex").query.clone();
        assert_eq!(first, "agent:codex rust");
        let second = chip(&rail(&first), "claude-code").query.clone();
        assert_eq!(second, "agent:codex,claude-code rust");
    }

    #[test]
    fn the_chip_that_is_on_is_the_chip_that_turns_it_off() {
        let on = rail("agent:codex rust");
        assert_eq!(chip(&on, "codex").query, "rust", "clicking a lit chip clears it");
    }

    #[test]
    fn all_is_selected_exactly_when_the_query_names_no_source() {
        assert!(rail("borrow checker").all.selected);
        assert!(rail("").all.selected);
        // An exclusion is still a claim about sources, so All is not on while one stands — and
        // clicking All takes it out with the rest.
        let excluding = rail("-agent:codex rust");
        assert!(!excluding.all.selected);
        assert_eq!(excluding.all.query, "rust");
    }

    #[test]
    fn a_source_the_index_holds_nothing_for_still_gets_a_chip() {
        // `a7k.29`'s finding, carried to the second client. A bar built from the index alone
        // cannot draw a configured source whose importer threw, so you search, find nothing,
        // and conclude you used a different tool.
        let rail = rail("");
        let gemini = chip(&rail, "gemini-cli");
        assert_eq!(gemini.conversations, 0);
        assert_eq!(gemini.coverage, "unconfigured");
        assert_eq!(rail.values.len(), census().len(), "every source the caller named");
    }

    #[test]
    fn the_rail_carries_the_keyword_it_writes() {
        // So a client can label the section without knowing the grammar it is a rail for.
        assert_eq!(rail("").keyword, "agent:");
    }
}
