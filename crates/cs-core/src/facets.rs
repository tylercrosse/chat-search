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
//! All three facets have a rail (`chat-search-1ld`), and they are three sections rather than one
//! generic list because what each is a census *of* differs. Sources are config ∪ index, so a
//! configured source at zero can be drawn. Directories are the index alone, ordered by weight and
//! cut off, because there is no config that names a project and the tail of that distribution is
//! scratch directories. Spans are arithmetic — the same four for an empty index as for a full one.

use serde::Serialize;

use crate::inventory::{DateCoverage, DirCensus, SourceCoverage};
use crate::query::{Facet, Query, Selection, DATE_SPANS};

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

/// What the query says about one value, decided the way the SQL decides it.
///
/// Comparison is [`Facet::selects`], so a chip never claims a state the filter would not produce
/// — which for `agent:` is equality against the lowercased id, and for `dir:` is the substring
/// match that lets one `dir:web-app` light every directory beneath it.
///
/// **An exclusion wins**, because in the SQL it is an `AND NOT`: under `dir:dev -dir:dev/scratch`
/// the scratch directory is dropped, so its chip says dropped. Drawing it as kept would be a chip
/// disagreeing with the list beside it about a conversation the reader can see is missing.
fn state(selection: &Selection, facet: Facet, candidate: &str) -> ChipState {
    if selection.exclude.iter().any(|v| facet.selects(v, candidate)) {
        ChipState::Exclude
    } else if selection.include.iter().any(|v| facet.selects(v, candidate)) {
        ChipState::Include
    } else {
        ChipState::Off
    }
}

/// The `agent:` rail for one query and one census.
///
/// Every source the caller supplies gets a chip, in the order the census is in — which
/// [`crate::inventory::join`] sorts by id, so the rail does not reshuffle between runs.
///
/// Selection is decided against the *lowercased* value, because that is what `search`'s SQL
/// compares `conversation.source` to: a chip whose id the filter would miss must not claim to be
/// on. A source id that is not already lowercase therefore draws off and stays off, which is
/// visible rather than silent — and `Config::validate` does not admit one today.
pub fn sources(query: &Query, census: &[SourceCoverage]) -> SourceFacet {
    let selection = query.selection(Facet::Agent);
    let values = census
        .iter()
        .map(|source| SourceChip {
            state: state(&selection, Facet::Agent, &source.id),
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

/// One directory chip: the census fact, the query fact, and the click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirChip {
    /// The `cwd` as the index recorded it, which is what `dir:` selects on. Not a project name:
    /// `chat-search-6eb.26` measured basename collapsing seven unrelated directories onto one
    /// label, and the nearest-`.git`-ancestor alternative reads the live filesystem and so breaks
    /// ADR 1. Shortening a path for display is a client's business and reversible; naming it is
    /// neither.
    pub value: String,
    pub state: ChipState,
    /// Conversations the index holds for it.
    pub conversations: i64,
    /// The whole query text after clicking this chip.
    pub query: String,
}

/// The `dir:` rail: the All chip, then the busiest directories the caller counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirFacet {
    pub keyword: &'static str,
    pub all: AllChip,
    pub values: Vec<DirChip>,
    /// Distinct directories in the index, which may be more than there are chips. What lets a
    /// client say it is showing the head of a list rather than the whole of one.
    pub indexed: i64,
    /// Conversations that record no directory at all, and which therefore no `dir:` token can
    /// ever reach. Two thirds of this corpus: only the agent sources have a working directory,
    /// and a rail that did not carry this number would read as a filter over everything
    /// (`chat-search-6eb.26`).
    pub undirected: i64,
}

/// The `dir:` rail for one query and one census.
///
/// **A directory is offered only when its click can be proved.** No `dir:` token can carry a path
/// with a space or a comma — whitespace separates words and commas separate values, so
/// `~/Mobile Documents` comes back as a filter *and* a search term (`chat-search-me9.8.16`). So
/// each click is reparsed here and a directory the round trip does not name is dropped, which
/// costs a chip and saves a chip that lies. The check is the grammar's own reading rather than a
/// list of forbidden characters, so it stays true the day the grammar gains quoting.
///
/// A directory the query names but the census did not reach gets no chip of its own. The rail is
/// the busiest directories, not the query's, and the token is visible and editable where every
/// filter is: in the box (`docs/TUI-DESIGN.md` §5). What says so is the All chip, which is off
/// while any `dir:` stands.
pub fn dirs(query: &Query, census: &DirCensus) -> DirFacet {
    let selection = query.selection(Facet::Dir);
    let values = census
        .busiest
        .iter()
        .filter_map(|dir| {
            let state = state(&selection, Facet::Dir, &dir.path);
            let click = query.toggling(Facet::Dir, &dir.path);
            let after = Query::typeahead(&click).selection(Facet::Dir);
            let names_it = after.include.iter().any(|v| *v == dir.path.to_lowercase());
            let honest = match state {
                // Turning off is stripping, which no path can spell wrongly.
                ChipState::Include => !after.include.iter().any(|v| Facet::Dir.selects(v, &dir.path)),
                _ => names_it,
            };
            honest.then(|| DirChip {
                value: dir.path.clone(),
                state,
                conversations: dir.conversations,
                query: click,
            })
        })
        .collect();

    DirFacet {
        keyword: Facet::Dir.keyword(),
        all: AllChip { selected: selection.is_empty(), query: query.without(Facet::Dir) },
        values,
        indexed: census.indexed,
        undirected: census.undirected,
    }
}

/// One span chip: how far back it reaches, what is in it, and the click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DateChip {
    /// The `date:` value, as the query text will carry it.
    pub value: &'static str,
    /// The same span in words. Carried because `>1mo` reads as syntax rather than as something to
    /// click, and two clients writing "Older" beside it is one rule written twice.
    pub label: &'static str,
    pub state: ChipState,
    /// Conversations the index holds inside this span. The spans nest, so these do not sum to the
    /// corpus — each answers "how many if I go back this far".
    pub conversations: i64,
    /// The whole query text after clicking this chip. It **replaces** whatever `date:` was there,
    /// because two date tokens intersect; see [`Query::toggling`].
    pub query: String,
}

/// The `date:` rail: the All chip, then [`DATE_SPANS`], newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DateFacet {
    pub keyword: &'static str,
    pub all: AllChip,
    pub values: Vec<DateChip>,
}

/// The `date:` rail for one query and one census.
///
/// The vocabulary is [`DATE_SPANS`] rather than the census, so an index that answered for none of
/// them still draws four chips at zero — the same reading as a configured source with no rows,
/// and for the same reason: a rail that omits what it cannot count is a rail that says the corpus
/// has no history.
///
/// A `date:` value outside the four — `date:yesterday`, `date:<3h`, a bound the reader typed —
/// lights no chip and turns the All chip off, which is the honest pair: something is filtering,
/// and it is not one of these. Two spellings of one span *are* one selection, because
/// [`Facet::selects`] compares the windows they resolve to rather than their text.
pub fn dates(query: &Query, census: &[DateCoverage]) -> DateFacet {
    let selection = query.selection(Facet::Date);
    let values = DATE_SPANS
        .iter()
        .map(|(value, label)| DateChip {
            value,
            label,
            state: state(&selection, Facet::Date, value),
            conversations: census
                .iter()
                .find(|c| c.value == *value)
                .map_or(0, |c| c.conversations),
            query: query.toggling(Facet::Date, value),
        })
        .collect();

    DateFacet {
        keyword: Facet::Date.keyword(),
        all: AllChip { selected: selection.is_empty(), query: query.without(Facet::Date) },
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Coverage, DirCoverage};

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

    #[test]
    fn a_value_the_query_both_keeps_and_drops_is_drawn_dropped() {
        // Which is what the SQL does with it: the exclusion is an `AND NOT`, so it survives the
        // include beside it. A chip drawn as kept would disagree with a list the reader can see
        // the conversation is missing from.
        assert_eq!(chip(&rail("agent:codex -agent:codex"), "codex").state, ChipState::Exclude);
    }

    // ---- the other two rails (chat-search-1ld) ----

    fn census_of(dirs: &[(&str, i64)], indexed: i64, undirected: i64) -> DirCensus {
        DirCensus {
            busiest: dirs
                .iter()
                .map(|(path, n)| DirCoverage { path: (*path).into(), conversations: *n })
                .collect(),
            indexed,
            undirected,
        }
    }

    fn dir_rail(text: &str) -> DirFacet {
        dirs(
            &Query::typeahead(text),
            &census_of(&[("/home/t/dev/web-app", 88), ("/home/t/dev/api-server", 12)], 40, 2011),
        )
    }

    #[test]
    fn one_dir_token_lights_every_chip_it_selects() {
        // `dir:` is a substring in the SQL, so a fragment filters to several directories at once
        // and the rail has to say which ones. Equality here would draw a rail of dark chips over
        // a list that is plainly filtered.
        let rail = dir_rail("dir:dev rust");
        assert!(rail.values.iter().all(|c| c.state == ChipState::Include));
        assert!(!rail.all.selected);
        // And the click on a lit chip takes out what lit it, rather than leaving a filter the
        // chip no longer reflects.
        assert_eq!(rail.values[0].query, "rust");
    }

    #[test]
    fn clicking_a_second_directory_widens_rather_than_replacing() {
        let first = dir_rail("rust").values[0].query.clone();
        assert_eq!(first, "dir:/home/t/dev/web-app rust");
        let both = dir_rail(&first).values[1].query.clone();
        assert_eq!(both, "dir:/home/t/dev/web-app,/home/t/dev/api-server rust");
        assert_eq!(dir_rail(&both).values[1].state, ChipState::Include, "and both are lit");
    }

    #[test]
    fn a_directory_whose_click_cannot_be_written_is_not_offered_at_all() {
        // No `dir:` token can carry a space or a comma (`chat-search-me9.8.16`), so this click
        // would come back as a filter *and* a search term. The chip is dropped rather than
        // handed over: a rail that cannot prove a click may not offer it.
        let census = census_of(&[("/home/t/Mobile Documents", 9), ("/home/t/dev/api", 4)], 2, 0);
        let rail = dirs(&Query::typeahead(""), &census);
        assert_eq!(rail.values.len(), 1);
        assert_eq!(rail.values[0].value, "/home/t/dev/api");
        // The count of what exists is unchanged by what could be drawn, or the client would
        // report a corpus that shrank when a rule was applied to it.
        assert_eq!(rail.indexed, 2);
    }

    #[test]
    fn the_dir_rail_carries_what_it_did_not_draw_and_what_it_cannot_reach() {
        let rail = dir_rail("");
        assert_eq!(rail.values.len(), 2);
        assert_eq!(rail.indexed, 40, "so a client can say it is showing two of forty");
        assert_eq!(rail.undirected, 2011, "and that `dir:` reaches none of these at all");
        assert!(rail.all.selected);
        assert_eq!(rail.keyword, "dir:");
    }

    fn date_rail(text: &str) -> DateFacet {
        dates(
            &Query::typeahead(text),
            &[
                DateCoverage { value: "today", conversations: 12 },
                DateCoverage { value: "week", conversations: 96 },
                DateCoverage { value: "month", conversations: 402 },
                DateCoverage { value: ">1mo", conversations: 2453 },
            ],
        )
    }

    #[test]
    fn every_span_gets_a_chip_whatever_the_census_could_answer() {
        // The `a7k.29` reading, carried to the facet that has no config half: an index that can
        // count nothing still has four spans, because they are arithmetic. Omitting them would
        // say the corpus has no history rather than that it has nothing in it yet.
        let empty = dates(&Query::typeahead(""), &[]);
        assert_eq!(empty.values.len(), 4);
        assert!(empty.values.iter().all(|c| c.conversations == 0));
        let counted = date_rail("");
        assert_eq!(
            counted.values.iter().map(|c| (c.value, c.label, c.conversations)).collect::<Vec<_>>(),
            [
                ("today", "Today", 12),
                ("week", "This week", 96),
                ("month", "This month", 402),
                (">1mo", "Older", 2453)
            ]
        );
    }

    #[test]
    fn clicking_a_second_span_replaces_the_first() {
        // Where a source chip would widen. Two `date:` tokens intersect, so a rail that added
        // beside would hand back `date:today date:>1mo` — a query that cannot match anything
        // that has ever been indexed.
        let rail = date_rail("date:today rust");
        let today = &rail.values[0];
        assert_eq!(today.state, ChipState::Include);
        assert_eq!(today.query, "rust", "the lit chip is the one that turns it off");
        assert_eq!(rail.values[3].query, "date:>1mo rust", "and any other replaces it");
    }

    #[test]
    fn a_span_typed_another_way_lights_the_chip_it_is() {
        // `date:<7d` *is* this week, and a rail comparing text would draw the chip dark while
        // filtering to exactly it — then add a second token when it was clicked.
        assert_eq!(date_rail("date:<7d").values[1].state, ChipState::Include);
        assert!(!date_rail("date:<7d").all.selected);
    }

    #[test]
    fn a_span_the_rail_does_not_offer_lights_nothing_and_is_still_not_all() {
        // `date:yesterday` is in the grammar and has no chip. The honest pair is every chip off
        // and All off: something is filtering, and it is not one of these.
        let rail = date_rail("date:yesterday rust");
        assert!(rail.values.iter().all(|c| c.state == ChipState::Off));
        assert!(!rail.all.selected);
        assert_eq!(rail.all.query, "rust", "and All is still the way out of it");
    }
}
