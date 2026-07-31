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

/// The values one `agent:` or `dir:` token names, with the two negation forms already folded
/// together.
///
/// `-agent:codex` and `agent:!codex` mean the same thing and land in the same place, so the
/// SQL never has to know which spelling arrived. A prefix `-` distributes over every comma
/// value, and an inline `!` flips its own — so `-agent:claude,!codex` excludes claude and
/// includes codex, which is the only reading under which both marks keep meaning "not".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    fn merge(&mut self, other: &Selection) {
        for v in &other.include {
            if !self.include.contains(v) {
                self.include.push(v.clone());
            }
        }
        for v in &other.exclude {
            if !self.exclude.contains(v) {
                self.exclude.push(v.clone());
            }
        }
    }
}

/// A span of time measured backwards from now.
///
/// Split by whether the unit is a duration or a calendar step, because that is exactly the
/// distinction DST makes real: an hour is always 3,600,000 ms, a day is 23, 24 or 25 of them.
/// See [`crate::time::shift_days_in`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Millis(i64),
    Days(i64),
    Months(i64),
}

/// What a `date:` value selects.
///
/// Named days are whole civil days; everything else is an age measured back from now, so
/// `date:week` and `date:<1w` are the same query written two ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateSpec {
    /// One whole civil day, `n` days back from today. `today` is 0, `yesterday` is 1.
    Day(i64),
    /// Younger than this age — `date:<3h`, and what `date:week` means.
    Younger(Age),
    /// Older than this age — `date:>1w`.
    Older(Age),
}

/// A resolved half-open window of epoch millis, `[from, until)`.
///
/// Half-open so that consecutive days tile without a millisecond falling in both or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: Option<i64>,
    pub until: Option<i64>,
}

impl DateSpec {
    /// Resolve against a clock and a zone. `None` if the arithmetic left chrono's range.
    pub fn window_in<Tz: chrono::TimeZone>(self, tz: &Tz, now_ms: i64) -> Option<Window> {
        match self {
            DateSpec::Day(back) => {
                let on = crate::time::shift_days_in(tz, now_ms, -back)?;
                let from = crate::time::day_start_in(tz, on)?;
                let until = crate::time::shift_days_in(tz, from, 1)?;
                Some(Window { from: Some(from), until: Some(until) })
            }
            DateSpec::Younger(age) => {
                Some(Window { from: Some(age.before(tz, now_ms)?), until: None })
            }
            DateSpec::Older(age) => {
                Some(Window { from: None, until: Some(age.before(tz, now_ms)?) })
            }
        }
    }

    /// Resolve against the machine's own zone.
    pub fn window(self, now_ms: i64) -> Option<Window> {
        self.window_in(&chrono::Local, now_ms)
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "today" => return Some(DateSpec::Day(0)),
            "yesterday" => return Some(DateSpec::Day(1)),
            "week" => return Some(DateSpec::Younger(Age::Days(7))),
            "month" => return Some(DateSpec::Younger(Age::Months(1))),
            _ => {}
        }
        let (rest, wrap): (&str, fn(Age) -> DateSpec) = match value.split_at_checked(1) {
            Some(("<", rest)) => (rest, DateSpec::Younger),
            Some((">", rest)) => (rest, DateSpec::Older),
            _ => return None,
        };
        Age::parse(rest).map(wrap)
    }
}

impl Age {
    /// The instant this far before `now_ms`.
    fn before<Tz: chrono::TimeZone>(self, tz: &Tz, now_ms: i64) -> Option<i64> {
        match self {
            Age::Millis(ms) => now_ms.checked_sub(ms),
            Age::Days(d) => crate::time::shift_days_in(tz, now_ms, -d),
            Age::Months(m) => crate::time::shift_months_in(tz, now_ms, -m),
        }
    }

    /// `3h`, `2d`, `1mo`. `mo` is matched before `m`, or every month would be a minute.
    fn parse(text: &str) -> Option<Self> {
        let split = text.find(|c: char| !c.is_ascii_digit())?;
        let count: i64 = text.get(..split)?.parse().ok()?;
        match text.get(split..)? {
            "m" => count.checked_mul(60_000).map(Age::Millis),
            "h" => count.checked_mul(3_600_000).map(Age::Millis),
            "d" => Some(Age::Days(count)),
            "w" => count.checked_mul(7).map(Age::Days),
            "mo" => Some(Age::Months(count)),
            "y" => count.checked_mul(12).map(Age::Months),
            _ => None,
        }
    }
}

/// What a filter token turned out to mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterKind {
    /// `agent:` and `dir:` — a set to keep and a set to drop.
    Names(Selection),
    /// `date:`, with the flag set by either negation form.
    Date(DateSpec, bool),
    /// Understood as a filter token, but the value names nothing that can be selected on:
    /// a half-typed `agent:`, a `date:nope`, a `date:today,week` that asks for two days at
    /// once. Kept rather than discarded so a surface can say the query is not doing what it
    /// reads as doing.
    Rejected,
}

/// One filter token lifted out of the query text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    facet: Facet,
    kind: FilterKind,
    as_typed: String,
}

impl Filter {
    fn new(facet: Facet, value: &str, negated: bool, as_typed: String) -> Self {
        let kind = match facet {
            Facet::Date => match DateSpec::parse(value) {
                Some(spec) => FilterKind::Date(spec, negated),
                None => FilterKind::Rejected,
            },
            Facet::Agent | Facet::Dir => match parse_selection(value, negated) {
                Some(selection) => FilterKind::Names(selection),
                None => FilterKind::Rejected,
            },
        };
        Self { facet, kind, as_typed }
    }

    pub fn facet(&self) -> Facet {
        self.facet
    }

    pub fn kind(&self) -> &FilterKind {
        &self.kind
    }

    /// Whether this filter reaches the SQL. False only for a value nothing can select on.
    pub fn is_active(&self) -> bool {
        self.kind != FilterKind::Rejected
    }

    /// The token as the user typed it, for reporting back to them.
    pub fn as_typed(&self) -> &str {
        &self.as_typed
    }
}

/// Split a comma list into keep and drop, honouring both negation marks.
///
/// `None` when nothing selectable survives — `agent:`, `dir:,,` — which is the half-typed
/// state a typeahead is in for most of its life and not something to raise an error over.
fn parse_selection(value: &str, prefix_negated: bool) -> Option<Selection> {
    let mut selection = Selection::default();
    for piece in value.split(',') {
        let (negated, name) = match piece.strip_prefix('!') {
            Some(rest) => (!prefix_negated, rest),
            None => (prefix_negated, piece),
        };
        if name.is_empty() {
            continue;
        }
        let bucket = if negated { &mut selection.exclude } else { &mut selection.include };
        if !bucket.iter().any(|v| v == name) {
            bucket.push(name.to_string());
        }
    }
    (!selection.is_empty()).then_some(selection)
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
                filters.push(Filter::new(facet, value, negated, word.to_string()));
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
            let already_named = self.filters.iter().any(|f| f.facet == Facet::Agent && f.is_active());
            if !already_named {
                let value = source.to_lowercase();
                let as_typed = format!("{}{value}", Facet::Agent.keyword());
                self.filters.push(Filter::new(Facet::Agent, &value, false, as_typed));
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

    /// Everything this query keeps and drops on one name-valued facet.
    ///
    /// Merged across tokens rather than last-one-wins, so `agent:codex agent:claude` selects
    /// both. That is the reading the facet bar needs: it filters by rewriting the query text
    /// (`TUI-DESIGN.md` §5), so clicking a second chip has to add to the first rather than
    /// replace it.
    pub fn selection(&self, facet: Facet) -> Selection {
        let mut out = Selection::default();
        for filter in self.filters.iter().filter(|f| f.facet == facet) {
            if let FilterKind::Names(names) = &filter.kind {
                out.merge(names);
            }
        }
        out
    }

    /// Every `date:` window in force, resolved against `now_ms`, each with its negation flag.
    ///
    /// Tokens intersect rather than union: `date:>1d date:<7d` is the week before yesterday,
    /// which is the only reading that lets two bounds describe a range. A window whose
    /// arithmetic overflowed chrono's range is dropped here and reported by [`Query::rejected`]
    /// — it cannot be silently widened to "everything".
    pub fn date_windows_in<Tz: chrono::TimeZone>(&self, tz: &Tz, now_ms: i64) -> Vec<(Window, bool)> {
        self.filters
            .iter()
            .filter_map(|f| match f.kind {
                FilterKind::Date(spec, negated) => Some((spec.window_in(tz, now_ms)?, negated)),
                _ => None,
            })
            .collect()
    }

    /// [`Query::date_windows_in`] against the machine's own zone.
    pub fn date_windows(&self, now_ms: i64) -> Vec<(Window, bool)> {
        self.date_windows_in(&chrono::Local, now_ms)
    }

    /// Filter tokens that were understood as filters but select nothing, as typed.
    ///
    /// A surface is expected to say so. Returning unfiltered results for a query that names a
    /// filter is a worse answer than returning none, because it looks like it worked. Since
    /// `chat-search-6eb.11` every filter the parser accepts is applied, so this is now the
    /// narrow case of a value nothing can be selected on — `date:nope`, a half-typed `agent:`
    /// — rather than the broad "not wired up yet" it started as.
    pub fn rejected(&self) -> Vec<String> {
        self.filters
            .iter()
            .filter(|f| !f.is_active())
            .map(|f| f.as_typed().to_string())
            .collect()
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
        assert_eq!(q.selection(Facet::Agent).include, ["codex"]);
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
        // Lifted from cs-tui's `App::holds`, which re-derived this threshold as its logical
        // complement in another crate. Same cases, asserted where the rule lives.
        assert_eq!(Query::typeahead("").mode(), Mode::Empty, "blank browses, it is not held");
        assert_eq!(Query::typeahead("e").mode(), Mode::TooShort);
        assert_eq!(Query::typeahead("le").mode(), Mode::TooShort);
        assert_eq!(Query::typeahead("  e  ").mode(), Mode::TooShort, "padding is not typing");
        assert_eq!(Query::typeahead("lea").mode(), Mode::Searchable, "the floor is where searching starts");
        // A preceding term bounds the posting list, which is the cost the floor guards
        // against, so `deep le` searches rather than holding.
        assert_eq!(Query::typeahead("deep l").mode(), Mode::Searchable);
        assert_eq!(Query::typeahead("deep le").mode(), Mode::Searchable);
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
        assert!(bare.selection(Facet::Agent).is_empty(), "an empty value selects nothing");
        assert_eq!(bare.rejected(), ["agent:"]);
    }

    #[test]
    fn every_form_of_the_dsl_parses_out_of_one_input_string() {
        // The acceptance criterion of `chat-search-6eb.11`, as one query: multi-value,
        // both negation forms, a substring facet, a relative date and free text, mixed in
        // any order, with none of it reaching the ranker as words to find.
        let q = Query::typeahead("agent:claude,codex -agent:gemini dir:!web-app date:<3h rust");
        assert_eq!(q.terms(), ["rust"], "a filter is never also a word to search for");

        let agents = q.selection(Facet::Agent);
        assert_eq!(agents.include, ["claude", "codex"]);
        assert_eq!(agents.exclude, ["gemini"]);

        let dirs = q.selection(Facet::Dir);
        assert!(dirs.include.is_empty());
        assert_eq!(dirs.exclude, ["web-app"], "an inline ! excludes just as a leading - does");

        assert_eq!(q.filters().len(), 4);
        assert!(q.rejected().is_empty(), "every one of these is in force");
    }

    #[test]
    fn the_two_negation_spellings_are_the_same_filter() {
        for text in ["-agent:codex", "agent:!codex"] {
            let selection = Query::typeahead(text).selection(Facet::Agent);
            assert_eq!(selection.exclude, ["codex"], "{text}");
            assert!(selection.include.is_empty(), "{text}");
        }
        // A leading `-` distributes over the whole comma list, and an inline `!` flips its
        // own value back — the only reading under which both marks keep meaning "not".
        let mixed = Query::typeahead("-agent:claude,!codex").selection(Facet::Agent);
        assert_eq!(mixed.exclude, ["claude"]);
        assert_eq!(mixed.include, ["codex"]);
    }

    #[test]
    fn repeating_a_facet_widens_the_selection_rather_than_replacing_it() {
        // The facet bar filters by rewriting the query text (`TUI-DESIGN.md` §5), so
        // clicking a second chip has to add to the first. Last-one-wins would make the bar
        // unable to express "these two sources" at all.
        let q = Query::typeahead("agent:codex agent:claude");
        assert_eq!(q.selection(Facet::Agent).include, ["codex", "claude"]);
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
        assert_eq!(q.selection(Facet::Agent).exclude, ["codex"]);
    }

    #[test]
    fn every_prefix_of_a_filter_being_typed_is_safe() {
        // A typeahead parses what is in the box on every keystroke, so it parses every
        // prefix of every filter anyone ever types. None of them may panic, and none may
        // silently become a filter that does something other than what the finished token
        // will do — `date:<3` must not filter as though it said `date:<3h`.
        for finished in ["date:<3h", "date:>12mo", "agent:claude,codex", "dir:!web-app", "date:today"] {
            for end in 1..=finished.len() {
                let Some(prefix) = finished.get(..end) else { continue };
                let q = Query::typeahead(prefix);
                // The invariant: parsing succeeded and produced a coherent value.
                let _ = q.match_expr();
                let _ = q.rejected();
                let _ = q.selection(Facet::Agent);
                let _ = q.date_windows(1_785_000_000_000);
            }
        }
    }

    #[test]
    fn a_date_value_that_cannot_be_a_date_is_rejected_rather_than_guessed_at() {
        for text in ["date:nope", "date:", "date:<", "date:>", "date:<h", "date:3h", "date:<3z", "date:<-3h"] {
            let q = Query::typeahead(text);
            assert_eq!(q.rejected(), [text], "{text} should be reported, not applied");
            assert!(q.date_windows(1_785_000_000_000).is_empty(), "{text} must not filter");
        }
        for text in ["date:<3h", "date:>1w", "date:today", "date:yesterday", "date:week", "date:month", "date:<90mo"] {
            let q = Query::typeahead(text);
            assert!(q.rejected().is_empty(), "{text} should be understood");
            assert_eq!(q.date_windows(1_785_000_000_000).len(), 1, "{text} should resolve");
        }
    }

    #[test]
    fn an_age_too_large_to_resolve_is_reported_rather_than_silently_unbounded() {
        // The failure that matters is not the panic, it is the fallback: an overflow that
        // quietly produced "no lower bound" would turn `date:<9999999999y` into a filter
        // that matches everything while still reading as a narrow one.
        let q = Query::typeahead("date:<9999999999999999999y borrow");
        assert!(q.date_windows(1_785_000_000_000).is_empty(), "nothing resolvable, so nothing applied");
        assert_eq!(q.terms(), ["borrow"], "and the rest of the query still works");
    }

    #[test]
    fn case_is_folded_before_anything_else_looks_at_the_query() {
        let q = Query::typeahead("Agent:Codex Borrow");
        assert_eq!(q.selection(Facet::Agent).include, ["codex"]);
        assert_eq!(q.terms(), ["borrow"]);
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
