//! The query, parsed once.
//!
//! Before this module the query was a `&str` re-tokenised on every path that needed it, by
//! two extractors with different rules: `search::to_match_expr_opts` built the FTS5 MATCH
//! expression, `highlight::query_terms` built the list of words to mark, and `marking_terms`
//! reconciled them by *string-inspecting the first one's output* for a trailing `"…"*`.
//! `App::holds` then re-derived the same prefix threshold a third time, as its logical
//! complement, in another crate.
//!
//! Measured 2026-07-30, the two extractors disagreed in two ways:
//!
//! ```text
//! agent:codex borrow   ranker: "agent" "codex" "borrow"*   marker: ["borrow*"]
//! learn deep learn     ranker: "learn" "deep" "learn"*     marker: ["learn", "deep"]
//! ```
//!
//! The first is a filter keyword the ranker did not know was a filter, so it became two
//! required words. Run against the 2,976-conversation index, `agent:codex borrow` returned 11
//! rows of which **10 were claude-code** — the exact opposite of what was asked for, because
//! what it really searched for was prose containing the words "agent" and "codex". A query of
//! filters alone degrades further: `agent:codex` left the marker with no terms at all, so all
//! 20 rows came back labelled ⟨no match⟩.
//!
//! The second is the bridge failing: `query_terms` deduplicates, so its last term is `deep`
//! while the expression ends with `"learn"*`, the tail check misses, and a row that ranked on
//! the stem expansion is marked with the exact term or not at all.
//!
//! Both come from one cause — two readings of one string — so the fix is one reading. The
//! MATCH expression and the marking terms are now two renderings of the same [`Query`], and
//! there is no bridge between them to get wrong.

use crate::search::MIN_PREFIX_LEN;

/// Whether a query can be run, and if not, why.
///
/// The distinction is a ranking cost, not a matter of taste: a one- or two-character prefix
/// matches a large fraction of the corpus and BM25 must score every matching row before it
/// can sort. Measured on 40k prose messages, `h*` is 2510 ms against `hov*` at 16 ms.
///
/// `cs-core` owns the *fact*; a client owns what to do about it. The TUI shows recent
/// conversations for anything that is not [`Mode::Searchable`], and says which of the two
/// reasons applies — silently listing recent conversations under a half-typed query reads as
/// "your query matched these".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Nothing searchable was typed. `""`, `"   "`, `"-"` and `"??"` all land here: they are
    /// non-empty input that still produces no terms, and an empty MATCH expression is an
    /// FTS5 syntax error rather than an empty result.
    Empty,
    /// One term, shorter than [`MIN_PREFIX_LEN`], in a typeahead. Only a *lone* short term
    /// qualifies: with another term present the prefix is bounded by the intersection, so it
    /// is both meaningful and cheap. Measured, `"le"*` alone matches 14,135 rows while
    /// `"deep" "le"*` matches 845 in 6 ms.
    TooShort,
    Searchable,
}

/// Which facet a filter names. The values live in [`Filter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facet {
    /// Selects on `conversation.source`.
    Agent,
    /// Selects on `conversation.cwd`, as a case-insensitive substring: enums want equality,
    /// paths want substring.
    Dir,
    /// Selects on `conversation.ended_at`.
    Date,
}

impl Facet {
    /// The keyword as typed, including its colon.
    pub fn keyword(self) -> &'static str {
        match self {
            Facet::Agent => "agent:",
            Facet::Dir => "dir:",
            Facet::Date => "date:",
        }
    }

    fn parse(word: &str) -> Option<(Self, &str)> {
        for facet in [Facet::Agent, Facet::Dir, Facet::Date] {
            if let Some(value) = word.strip_prefix(facet.keyword()) {
                return Some((facet, value));
            }
        }
        None
    }
}

/// One filter token lifted out of the query text.
///
/// Recognising a filter is not the same as applying one. Only a plain positive `agent:` has
/// somewhere to go today — it feeds the `source` clause the ranker already has. Everything
/// else is carried and reported rather than dropped, because a filter that is silently
/// ignored returns unfiltered results that look filtered. Wiring the rest is
/// `chat-search-6eb.11`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub facet: Facet,
    pub value: String,
    /// From the `-agent:codex` form.
    pub negated: bool,
}

impl Filter {
    /// Whether this filter reaches the SQL.
    ///
    /// Deliberately narrow. `dir:` and `date:` have columns but no clause yet; negation and
    /// the multi-value `agent:claude,codex` form need SQL this change does not write. Each
    /// of those is surfaced by [`Query::unapplied`] instead of quietly doing nothing.
    pub fn is_active(&self) -> bool {
        self.facet == Facet::Agent && !self.negated && !self.value.contains(',') && !self.value.is_empty()
    }

    /// The token as the user typed it, for reporting back to them.
    pub fn as_typed(&self) -> String {
        let dash = if self.negated { "-" } else { "" };
        format!("{dash}{}{}", self.facet.keyword(), self.value)
    }
}

/// A parsed query: what was asked, as distinct from [`crate::SearchOptions`], which is how to
/// run it.
///
/// Construct with [`Query::typeahead`] or [`Query::exact`]. Prefix handling is fixed at parse
/// time rather than passed to each renderer, because [`Mode::TooShort`] exists *only* because
/// of prefix expansion — a query that is too short to expand is perfectly cheap to match
/// exactly. Were prefix a render-time argument, `mode`, `match_expr` and `marking_terms`
/// would each take the same flag and have to be passed it consistently, which is the failure
/// this module exists to remove.
#[derive(Debug, Clone)]
pub struct Query {
    raw: String,
    /// Lowercased, in the order typed, **not** deduplicated: the ranker ANDs repeats, and
    /// `learn deep learn` must still put its star on the final `learn`.
    terms: Vec<String>,
    filters: Vec<Filter>,
    /// The final term is still being typed and is long enough to expand.
    expand_last: bool,
    mode: Mode,
}

impl Query {
    /// The typeahead reading: the final word is open-ended unless a separator closed it.
    ///
    /// Every keystroke in the TUI takes this path, which is why the short-prefix floor and
    /// [`Mode::TooShort`] exist at all.
    pub fn typeahead(text: &str) -> Self {
        Self::parse(text, true)
    }

    /// The exact reading: every word is a complete term. The CLI default, and what the eval
    /// harness ranks with.
    pub fn exact(text: &str) -> Self {
        Self::parse(text, false)
    }

    /// Parsing never fails.
    ///
    /// A half-typed `dir:` or a bare `-` is literal text, not an error. Mid-word is the
    /// normal state in a typeahead, so a parse error would be a broken UI rather than a
    /// message anyone could act on (`chat-search-6eb.11`).
    fn parse(text: &str, prefix: bool) -> Self {
        let lower = text.to_lowercase();
        let mut terms: Vec<String> = Vec::new();
        let mut filters: Vec<Filter> = Vec::new();

        for word in lower.split_whitespace() {
            let (negated, bare) = match word.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, word),
            };
            if let Some((facet, value)) = Facet::parse(bare) {
                filters.push(Filter { facet, value: value.to_string(), negated });
                continue;
            }
            // Not a filter, so it is text. Split the *whole* word, leading dash included:
            // `-` is only negation in front of a facet keyword, and treating it as negation
            // for free text would silently drop a word the ranker is about to require.
            for term in word.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if !term.is_empty() {
                    terms.push(term.to_string());
                }
            }
        }

        // A trailing separator means the last word is finished. Read off the original text,
        // not the term list, which has already thrown the punctuation away.
        let ends_open = lower.chars().last().is_some_and(|c| c.is_alphanumeric() || c == '_');
        // The floor applies only to a lone term: an earlier term bounds the posting list,
        // which is the cost the floor guards against. See `MIN_PREFIX_LEN`.
        let long_enough =
            terms.len() > 1 || terms.last().is_some_and(|t| t.chars().count() >= MIN_PREFIX_LEN);
        let expand_last = prefix && ends_open && long_enough && !terms.is_empty();

        let mode = if terms.is_empty() {
            Mode::Empty
        } else if prefix && !long_enough {
            Mode::TooShort
        } else {
            Mode::Searchable
        };

        Self { raw: text.to_string(), terms, filters, expand_last, mode }
    }

    /// Fold a structured source selection — a `--source` flag, a facet click — into the
    /// query's own filters.
    ///
    /// This is the one desugaring point. Keeping a selection beside the query as a second
    /// piece of state is what `TUI-DESIGN.md` §5 records fast-resume paying six reconciliation
    /// methods for; a filter has exactly one home, and this is how a flag reaches it. A source
    /// already named in the text wins, since the user typed it more recently than they passed
    /// a flag.
    pub fn with_source(mut self, source: Option<&str>) -> Self {
        if let Some(source) = source {
            if self.source_filter().is_none() {
                self.filters.push(Filter {
                    facet: Facet::Agent,
                    value: source.to_lowercase(),
                    negated: false,
                });
            }
        }
        self
    }

    /// The text as typed, for redisplay. Not what gets searched — see [`Query::match_expr`].
    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn is_searchable(&self) -> bool {
        self.mode == Mode::Searchable
    }

    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }

    /// A valid FTS5 MATCH expression, with implicit AND between terms.
    ///
    /// Raw user input is not one — a bare `-`, `*` or quote is a syntax error — so every term
    /// is re-quoted, with embedded quotes doubled.
    pub fn match_expr(&self) -> String {
        let last = self.terms.len().saturating_sub(1);
        self.terms
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let t = t.replace('"', "\"\"");
                if self.expand_last && i == last {
                    format!("\"{t}\"*")
                } else {
                    format!("\"{t}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The terms a highlighter should mark, carrying the same prefix star the ranker used.
    ///
    /// Rendered from the same `terms` as [`Query::match_expr`] rather than recovered from its
    /// output, which is what the old `ends_with` bridge did and what deduplication defeated.
    /// Deduplicated here and not there because the two want different things: the ranker ANDs
    /// a repeated word, a marker would only paint it twice. A term that appears both plain and
    /// as the open final token marks as the star, which subsumes the exact form.
    pub fn marking_terms(&self) -> Vec<String> {
        let last = self.terms.last();
        let mut out: Vec<String> = Vec::new();
        for (i, term) in self.terms.iter().enumerate() {
            let starred = self.expand_last && Some(term) == last;
            let rendered = if starred { format!("{term}*") } else { term.clone() };
            // A starred form replaces an exact one already collected; otherwise first wins.
            if let Some(pos) = out.iter().position(|t| t.trim_end_matches('*') == term) {
                if starred {
                    out[pos] = rendered;
                }
            } else {
                let _ = i;
                out.push(rendered);
            }
        }
        out
    }

    /// The source this query selects on, if it names one that can be applied.
    pub fn source_filter(&self) -> Option<&str> {
        self.filters
            .iter()
            .find(|f| f.facet == Facet::Agent && f.is_active())
            .map(|f| f.value.as_str())
    }

    /// Filter tokens that were understood but are not in force, as typed.
    ///
    /// A surface is expected to say so. Returning unfiltered results for a query that names a
    /// filter is a worse answer than returning none, because it looks like it worked.
    pub fn unapplied(&self) -> Vec<String> {
        self.filters.iter().filter(|f| !f.is_active()).map(Filter::as_typed).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expr(q: &str) -> String {
        Query::typeahead(q).match_expr()
    }

    #[test]
    fn a_filter_keyword_is_not_text_to_find() {
        // The divergence this module was built for: the ranker used to AND `agent` and
        // `codex` in as literal words while the highlighter dropped them, so every row it
        // returned carried the ⟨no match⟩ label.
        let q = Query::typeahead("agent:codex borrow");
        assert_eq!(q.terms(), ["borrow"]);
        assert_eq!(q.match_expr(), "\"borrow\"*");
        assert_eq!(q.source_filter(), Some("codex"));
    }

    #[test]
    fn a_repeated_final_word_keeps_its_prefix_star() {
        // The bridge used to read the star back off the expression and compare it against a
        // deduplicated list, so `learn deep learn` marked `learn` exact while the ranker had
        // matched `learn*`.
        let q = Query::typeahead("learn deep learn");
        assert_eq!(q.match_expr(), "\"learn\" \"deep\" \"learn\"*");
        assert_eq!(q.marking_terms(), ["learn*", "deep"]);
    }

    #[test]
    fn the_ranker_and_the_marker_never_disagree_about_the_star() {
        // The structural invariant. Whatever the expression stars, the marking list stars,
        // because both are rendered from one term list rather than one from the other.
        for q in [
            "borrow",
            "borrow ",
            "deep learn",
            "learn deep learn",
            "agent:codex borrow",
            "le",
            "deep le",
            "a-b_c",
            "\"quoted\"",
        ] {
            let query = Query::typeahead(q);
            let starred_in_expr = query.match_expr().contains("\"*");
            let starred_in_marks = query.marking_terms().iter().any(|t| t.ends_with('*'));
            assert_eq!(starred_in_expr, starred_in_marks, "{q:?} stars one side only");
        }
    }

    #[test]
    fn every_marking_term_is_a_term_the_ranker_matched() {
        for q in ["agent:codex borrow", "dir:web date:today rust", "learn deep learn", "-borrow"] {
            let query = Query::typeahead(q);
            for mark in query.marking_terms() {
                let bare = mark.trim_end_matches('*').to_string();
                assert!(
                    query.terms().contains(&bare),
                    "{q:?} marks {mark:?}, which the ranker never matched on"
                );
            }
        }
    }

    #[test]
    fn input_that_produces_no_terms_is_empty_rather_than_a_syntax_error() {
        // An empty MATCH expression is an FTS5 syntax error, not an empty result, so this
        // has to be detectable before the query reaches SQLite.
        for q in ["", "   ", "-", "??", "  ..  "] {
            assert_eq!(Query::typeahead(q).mode(), Mode::Empty, "{q:?}");
            assert_eq!(Query::typeahead(q).match_expr(), "", "{q:?}");
        }
    }

    #[test]
    fn a_lone_short_term_is_held_but_a_bounded_one_is_not() {
        assert_eq!(Query::typeahead("le").mode(), Mode::TooShort);
        // A preceding term bounds the posting list, which is the cost the floor guards
        // against, so `deep le` searches rather than holding.
        assert_eq!(Query::typeahead("deep le").mode(), Mode::Searchable);
        assert_eq!(Query::typeahead("lea").mode(), Mode::Searchable);
    }

    #[test]
    fn exact_mode_never_holds_a_short_query() {
        // Nothing is expanded, so there is no unbounded posting list to guard against and a
        // two-character term is perfectly cheap to match.
        assert_eq!(Query::exact("le").mode(), Mode::Searchable);
        assert_eq!(Query::exact("le").match_expr(), "\"le\"");
    }

    #[test]
    fn a_finished_word_is_not_expanded() {
        assert_eq!(expr("borrow"), "\"borrow\"*");
        assert_eq!(expr("borrow "), "\"borrow\"");
        assert_eq!(expr("deep learn"), "\"deep\" \"learn\"*");
    }

    #[test]
    fn punctuation_that_would_be_a_syntax_error_survives() {
        assert_eq!(expr("a-b"), "\"a\" \"b\"*");
        assert_eq!(expr("say \"hi\""), "\"say\" \"hi\"");
        assert_eq!(expr("*"), "");
    }

    #[test]
    fn a_half_typed_filter_is_text_rather_than_an_error() {
        // Mid-word is the normal state in a typeahead. `age` is not `agent:`, so it is a
        // word being typed, and `agent:` with no value names no source to select.
        assert_eq!(Query::typeahead("age").terms(), ["age"]);
        assert!(Query::typeahead("age").filters().is_empty());
        let bare = Query::typeahead("agent:");
        assert_eq!(bare.filters().len(), 1);
        assert_eq!(bare.source_filter(), None, "an empty value selects nothing");
        assert_eq!(bare.unapplied(), ["agent:"]);
    }

    #[test]
    fn a_filter_that_cannot_be_applied_is_reported_rather_than_dropped() {
        let q = Query::typeahead("dir:web-app date:<3d -agent:codex agent:claude,gemini rust");
        assert_eq!(q.terms(), ["rust"]);
        assert_eq!(q.source_filter(), None);
        assert_eq!(
            q.unapplied(),
            ["dir:web-app", "date:<3d", "-agent:codex", "agent:claude,gemini"],
            "each is understood, none is in force, and the user has to be told"
        );
    }

    #[test]
    fn a_leading_dash_negates_a_facet_but_not_a_word() {
        // Measured 2026-07-30: both extractors already agreed that `-borrow` matches
        // `"borrow"`, so this pins the behaviour rather than changing it. Negating free text
        // is not in the DSL; negating a facet is.
        assert_eq!(Query::typeahead("-borrow").terms(), ["borrow"]);
        assert_eq!(expr("-borrow"), "\"borrow\"*");
        let q = Query::typeahead("-agent:codex");
        assert!(q.terms().is_empty());
        assert!(q.filters()[0].negated);
    }

    #[test]
    fn case_is_folded_before_anything_else_looks_at_the_query() {
        assert_eq!(Query::typeahead("Agent:Codex Borrow").source_filter(), Some("codex"));
        assert_eq!(Query::typeahead("Agent:Codex Borrow").terms(), ["borrow"]);
    }

    #[test]
    fn the_expressions_this_parser_produces_are_pinned() {
        // Every row was produced by `search::to_match_expr_opts`, the code this module
        // replaced, and checked against it byte for byte before that function was deleted
        // (the throwaway `tests/differential.rs` in the preceding commit). Keeping the corpus
        // as literals is what survives the deletion: a future change to the tokenising rules
        // has to state which of these it means to alter.
        let cases: [(&str, &str, &str); 31] = [
            // query, typeahead expression, exact expression
            ("borrow", "\"borrow\"*", "\"borrow\""),
            ("borrow ", "\"borrow\"", "\"borrow\""),
            ("le", "\"le\"", "\"le\""),
            ("lea", "\"lea\"*", "\"lea\""),
            ("deep le", "\"deep\" \"le\"*", "\"deep\" \"le\""),
            ("deep learn", "\"deep\" \"learn\"*", "\"deep\" \"learn\""),
            ("learn deep learn", "\"learn\" \"deep\" \"learn\"*", "\"learn\" \"deep\" \"learn\""),
            ("rust async", "\"rust\" \"async\"*", "\"rust\" \"async\""),
            ("a-b_c", "\"a\" \"b_c\"*", "\"a\" \"b_c\""),
            ("say \"hi\"", "\"say\" \"hi\"", "\"say\" \"hi\""),
            ("*", "", ""),
            ("-", "", ""),
            ("??", "", ""),
            ("", "", ""),
            ("   ", "", ""),
            ("Borrow Checker", "\"borrow\" \"checker\"*", "\"borrow\" \"checker\""),
            ("the", "\"the\"*", "\"the\""),
            ("emb", "\"emb\"*", "\"emb\""),
            ("rus", "\"rus\"*", "\"rus\""),
            ("café", "\"café\"*", "\"café\""),
            ("x", "\"x\"", "\"x\""),
            ("xy", "\"xy\"", "\"xy\""),
            ("xyz", "\"xyz\"*", "\"xyz\""),
            ("a b c d e", "\"a\" \"b\" \"c\" \"d\" \"e\"*", "\"a\" \"b\" \"c\" \"d\" \"e\""),
            ("under_score", "\"under_score\"*", "\"under_score\""),
            ("123", "\"123\"*", "\"123\""),
            ("-borrow", "\"borrow\"*", "\"borrow\""),
            ("a.b.c", "\"a\" \"b\" \"c\"*", "\"a\" \"b\" \"c\""),
            ("foo/bar", "\"foo\" \"bar\"*", "\"foo\" \"bar\""),
            ("agent:codex borrow", "\"borrow\"*", "\"borrow\""),
            ("dir:web-app rust", "\"rust\"*", "\"rust\""),
        ];
        for (q, typeahead, exact) in cases {
            assert_eq!(Query::typeahead(q).match_expr(), typeahead, "typeahead {q:?}");
            assert_eq!(Query::exact(q).match_expr(), exact, "exact {q:?}");
        }
    }

    // ---- lifted from search.rs, where these lived while the rules did ----

    #[test]
    fn adding_characters_never_widens_the_result_set() {
        // Reported from the TUI 2026-07-30: `deep le` returned 6 conversations and
        // `deep lea` returned over 100. The floor was being applied to the final token of a
        // multi-term query, so `le` was matched as a literal word while `lea` became a
        // prefix — the semantics flipped mid-word and the set grew.
        assert_eq!(Query::typeahead("deep l").match_expr(), "\"deep\" \"l\"*");
        assert_eq!(Query::typeahead("deep le").match_expr(), "\"deep\" \"le\"*");
        assert_eq!(Query::typeahead("deep lea").match_expr(), "\"deep\" \"lea\"*");
    }

    #[test]
    fn a_lone_short_term_keeps_the_floor() {
        // Nothing to intersect with, so the posting list is unbounded and the floor is the
        // only thing standing between a keystroke and scoring a large fraction of the corpus.
        assert_eq!(Query::typeahead("l").match_expr(), "\"l\"");
        assert_eq!(Query::typeahead("le").match_expr(), "\"le\"");
        assert_eq!(Query::typeahead("lea").match_expr(), "\"lea\"*");
    }

    #[test]
    fn prefix_applies_only_to_an_unfinished_final_token() {
        assert_eq!(Query::typeahead("borrow check").match_expr(), r#""borrow" "check"*"#);
        // trailing space means that word is finished
        assert_eq!(Query::typeahead("borrow check ").match_expr(), r#""borrow" "check""#);
        // below the floor, matched exactly: a 1-2 char prefix matches most of the corpus
        // and BM25 must score every row before sorting
        assert_eq!(Query::typeahead("ho").match_expr(), r#""ho""#);
        assert_eq!(Query::typeahead("hov").match_expr(), r#""hov"*"#);
        // off by default, so ordinary search is unaffected
        assert_eq!(Query::exact("borrow check").match_expr(), r#""borrow" "check""#);
    }

    #[test]
    fn a_query_with_no_terms_produces_no_expression_however_it_is_spelled() {
        for q in ["", "   ", "-", "??", "  *  "] {
            assert_eq!(Query::exact(q).match_expr(), "", "{q:?} yields no FTS terms, so it cannot be MATCHed");
        }
        assert_ne!(Query::exact("hov").match_expr(), "");
        assert_ne!(Query::exact("a").match_expr(), "");
    }

    #[test]
    fn the_raw_text_survives_parsing() {
        // The input box redisplays what was typed, not what was matched.
        assert_eq!(Query::typeahead("Agent:Codex  borrow").raw(), "Agent:Codex  borrow");
    }
}
