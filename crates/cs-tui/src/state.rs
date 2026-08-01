//! What the TUI knows between keystrokes.
//!
//! No worker thread, no generation counter, no debounce. The search is synchronous in the
//! event loop because it costs 1.4–6.4 ms in-process, so there is never an in-flight query to
//! go stale (ADR 14, and `me9.4` was closed for this reason). If any keystroke path ever
//! measures past ~30 ms that stops being true and generation numbering has to come back —
//! reopen `me9.4` rather than reinventing it.

use std::collections::HashSet;

use cs_core::querylog::{self, Event};
use cs_core::search::Group;
use rusqlite::Connection;

use crate::preview::Preview;
use crate::rows::{self, Row};
use crate::theme::Theme;

/// Characters required before a query is run.
///
/// Tied to `cs_core`'s prefix threshold rather than picked for feel: below it the final
/// token is matched exactly instead of expanded, so a shorter query returns matches for a
/// word nobody typed. Only the ghost-text countdown reads this now — whether a query is
/// held is [`cs_core::Mode`]'s answer, not a threshold this crate re-applies.
pub const MIN_QUERY_CHARS: usize = cs_core::search::MIN_PREFIX_LEN;

pub struct App {
    conn: Connection,
    /// Corpus size, for the header's "shown / indexed". Read once: the index is rebuilt by a
    /// separate process, never mutated underneath us.
    pub indexed: i64,
    /// How many conversations the current query selects, `limit` ignored — the header's
    /// middle number.
    ///
    /// Written only by a search that succeeded, so a failed one leaves the previous count
    /// beside the previous rows rather than claiming the query matched nothing.
    pub matched: usize,
    /// Per-source counts for the facet bar, index-derived and sorted for a stable order.
    ///
    /// A configured source holding zero rows is absent here, which is exactly the gap
    /// `me9.14` closes — it needs the config, which this crate deliberately cannot see (§1).
    pub facets: Vec<(String, i64)>,
    /// Captured per search rather than per frame, so every age on screen is measured from the
    /// same instant and the column cannot renumber itself mid-scroll.
    pub now: i64,

    /// The whole of the filter state, and the whole of what is searched for.
    ///
    /// There is deliberately no `source` field beside this. `TUI-DESIGN.md` §5 records what
    /// that shape cost fast-resume — six methods reconciling a facet field against the query
    /// text — and the visible half of the same bug here was a facet click that filtered the
    /// list without appearing in the box, so it could not be seen, edited or copied out.
    pub query: String,
    /// Caret position as a *char* index, not a byte offset — the query holds arbitrary text.
    pub cursor: usize,
    pub limit: i64,

    pub groups: Vec<Group>,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub expanded: HashSet<String>,

    pub show_preview: bool,
    /// The selected conversation's messages, loaded on selection change rather than per
    /// frame. `None` before the first load and whenever the read failed — the pane says so
    /// rather than rendering as though the conversation were empty.
    pub preview: Option<Preview>,
    /// Shown in the footer in place of the key hints. Set by a failed search, cleared by the
    /// next one that succeeds.
    pub status: Option<String>,
    pub last_ms: f64,
    pub theme: Theme,

    /// Whether a `Pick` was emitted this session. Decides if quitting should emit the
    /// abandonment `Search` — the signal that the ranking showed nothing worth opening,
    /// which `6eb.21` cannot get any other way.
    picked: bool,
}

impl App {
    pub fn new(conn: Connection, opts: &crate::Opts, theme: Theme) -> rusqlite::Result<Self> {
        let indexed =
            conn.query_row("SELECT COUNT(*) FROM conversation", [], |r| r.get(0)).unwrap_or(0);
        let facets = read_facets(&conn);
        // `--source` is desugared here, once, and never again: `with_source` rewrites the
        // query *text*, so what lands in the box is a query the user could have typed and the
        // flag has nowhere left to live. This is the whole of the flag's life in this crate.
        let query =
            cs_core::Query::typeahead(&opts.query).with_source(opts.source.as_deref()).raw().to_string();
        let mut app = App {
            conn,
            indexed,
            matched: 0,
            facets,
            now: cs_core::now_ms(),
            cursor: query.chars().count(),
            query,
            limit: if opts.limit > 0 { opts.limit } else { 100 },
            groups: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            expanded: HashSet::new(),
            show_preview: true,
            preview: None,
            status: None,
            last_ms: 0.0,
            theme,
            picked: false,
        };
        app.search();
        Ok(app)
    }

    /// Re-run the query and rebuild the row vector.
    ///
    /// A failed search keeps the previous results on screen and reports the error
    /// (`me9.5`): rendering an empty successful search would say "nothing matched" when the
    /// truth is "the index could not be read".
    pub fn search(&mut self) {
        self.now = cs_core::now_ms();
        let t0 = std::time::Instant::now();
        // No blanking of the query text. `search_grouped` routes anything that is not
        // `Searchable` to the recent list itself, so this used to hand it an empty string to
        // force a branch it would have taken anyway — and that rewrite is what made
        // `rows::build` see a different answer here than in `rebuild_rows`.
        let query = self.parsed();
        let q = cs_core::SearchOptions { limit: self.limit, ..cs_core::SearchOptions::new(self.now) };
        // `nested = 0`: no snippets. Building one means tokenizing the whole message, because
        // the highlighter has to agree with the ranker about which words matched
        // (chat-search-6eb.20) — and the row model draws hits only for an expanded
        // conversation, so per keystroke this would pay for 150 snippets and draw none.
        // Measured at limit 50 on the 180k-message index, that is a further 4–21 ms on top of
        // the ranking; it was 11–35 ms before chat-search-6eb.30 batched the marking.
        // Counted, because "100 shown" says nothing about whether there were 101 or 2,000 —
        // and the count is free unless the ranking pass stopped at its own scan ceiling.
        match cs_core::search_grouped_counted(&self.conn, &query, &q) {
            Ok(found) => {
                self.last_ms = t0.elapsed().as_secs_f64() * 1000.0;
                self.matched = found.matched;
                self.groups = found.groups;
                self.status = None;
                self.rows = rows::build(&self.groups, &self.expanded, !query.is_searchable());
                // A query edit is a new question, so the cursor belongs on the best answer to
                // it. Following the previously-selected conversation here — which an earlier
                // version did — drags the cursor down the ranking as the query narrows: type
                // one more letter and you are looking at rank 15 of a list whose top just
                // changed. Preservation is for rebuilds the user did not ask for (an
                // expansion toggle, and the background reindex of `6eb.16`), which is why it
                // lives in `rebuild_rows` and not here.
                self.selected = 0;
                self.sync_preview();
            }
            Err(e) => self.status = Some(format!("search failed: {e}")),
        }
    }

    /// Rebuild the row vector without re-querying, keeping the cursor on `previous`.
    fn rebuild_rows(&mut self, previous: Option<&str>) {
        self.rows = rows::build(&self.groups, &self.expanded, !self.parsed().is_searchable());
        self.selected = rows::reselect(&self.rows, &self.groups, previous, self.selected);
        self.sync_preview();
    }

    /// The query as `cs-core` reads it. Filters included — they are in the text.
    ///
    /// Parsed on demand rather than cached beside `self.query`, because a cached copy is a
    /// second thing to keep in step and this crate has just finished removing one. The parse
    /// is string work on a few dozen characters against a search measured in milliseconds.
    ///
    /// Typeahead because the TUI is: every keystroke leaves a word half-typed, and the
    /// ranking has to open the final token or the list widens as you type.
    pub fn parsed(&self) -> cs_core::Query {
        cs_core::Query::typeahead(&self.query)
    }

    /// Which sources the query selects, for the facet bar to draw itself from.
    pub fn selected_sources(&self) -> cs_core::Selection {
        self.parsed().selection(cs_core::Facet::Agent)
    }

    /// What the query can do, as `cs-core` sees it. The UI decides what to *show* for each.
    pub fn mode(&self) -> cs_core::Mode {
        self.parsed().mode()
    }

    /// Something has been typed, but not yet enough to search on.
    ///
    /// The list underneath is recent conversations, not a partial answer, so the UI has to
    /// say which it is showing — silently listing recent conversations under a half-typed
    /// query reads as "your query matched these".
    pub fn holding(&self) -> bool {
        self.mode() == cs_core::Mode::TooShort
    }

    /// True when the results are the recent-conversations fallback (`6eb.5`) rather than
    /// matches, for either reason.
    pub fn browsing(&self) -> bool {
        self.mode() != cs_core::Mode::Searchable
    }

    /// Reload the preview if the cursor has moved to a different conversation.
    ///
    /// Keyed on `conv_id`, not on the row index: moving between a header and its own hit
    /// rows is the same conversation, and rereading it there would throw away the fold state
    /// the user just set.
    /// Scroll the preview so the focused message is visible, given the pane's height.
    ///
    /// The height is only known at render time, so the caller passes it. Focus that does not
    /// drag the viewport with it is focus you cannot see.
    pub fn follow_preview_focus(&mut self, width: usize, viewport_rows: usize) {
        let theme = self.theme;
        if let Some(p) = self.preview.as_mut() {
            p.follow_focus(&theme, width, viewport_rows);
        }
    }

    /// The terms the preview marks against — the same text the ranker was given, reduced the
    /// same way.
    ///
    /// A held or blank query is not run at all (see [`App::search`]), so the list below is
    /// recent conversations rather than matches; marking against a query nothing was ranked on
    /// would put highlights on rows that never matched it.
    fn preview_terms(&self) -> Vec<String> {
        if self.browsing() {
            return Vec::new();
        }
        // `true` because the TUI is typeahead by definition and `search` below ranks with the
        // final token open; the preview has to mark what that ranking actually matched.
        cs_core::Query::typeahead(&self.query).marking_terms()
    }

    pub fn sync_preview(&mut self) {
        let wanted = self.selected_conv().map(str::to_owned);
        // The terms are half the key, not a detail of it. Keying on the conversation alone —
        // which this did — left a still-selected row showing marks located against the previous
        // query, so every keystroke after the first made the highlighting a little more wrong
        // while looking entirely deliberate.
        let terms = self.preview_terms();
        let fresh = self
            .preview
            .as_ref()
            .is_some_and(|p| Some(&p.conv_id) == wanted.as_ref() && p.terms == terms);
        if fresh {
            return;
        }
        // Folds and scroll are dropped with the old preview, and that is the same rule
        // `cycle_density` and the post-search reselect already follow: an edited query is a new
        // question, so a fold decided against the old one is not evidence about this one.
        self.preview = wanted.and_then(|id| Preview::load(&self.conn, &id, &terms).ok());
    }

    pub fn selected_group(&self) -> Option<&Group> {
        self.groups.get(self.rows.get(self.selected)?.group())
    }

    pub fn selected_conv(&self) -> Option<&str> {
        self.selected_group().map(|g| g.conv_id.as_str())
    }

    /// Scroll the preview without moving the list cursor. Wheel over the preview pane.
    ///
    /// Takes the pane's geometry because the stop at the bottom depends on how tall the
    /// conversation renders, which with wrapping is a function of the width.
    pub fn scroll_preview(&mut self, delta: isize, width: usize, viewport_rows: usize) {
        let theme = self.theme;
        if let Some(p) = self.preview.as_mut() {
            p.scroll_by(&theme, width, viewport_rows, delta);
        }
    }

    /// Select a row directly, as a click does.
    pub fn select(&mut self, index: usize) {
        if index >= self.rows.len() {
            return;
        }
        self.selected = index;
        self.sync_preview();
    }

    /// Act on a facet chip: `Some` toggles one source, `None` is the All chip and clears them.
    ///
    /// The bar filters by *rewriting the query text*, which is what makes it a projection of
    /// the query rather than a filter beside it (`TUI-DESIGN.md` §5). Everything about which
    /// tokens that means — widen an existing `agent:`, drop a standing exclusion, keep the
    /// last word open or closed — is `cs_core::Query`'s, because the grammar is.
    pub fn toggle_source(&mut self, source: Option<String>) {
        let parsed = self.parsed();
        self.query = match source {
            Some(s) => parsed.toggling(cs_core::Facet::Agent, &s),
            None => parsed.without(cs_core::Facet::Agent),
        };
        // The caret goes to the end because a rewrite can move or remove a token anywhere in
        // the string, so the old offset names a different word than it did. The end is the one
        // position that still means what it meant — and new filter tokens are inserted in
        // front of the free text, so it is also where typing continues.
        self.cursor = self.query.chars().count();
        self.search();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let max = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        self.sync_preview();
    }

    /// Expand or collapse the selected conversation, keeping the cursor on it.
    ///
    /// Expanding is where snippets get paid for. Deliberate and occasional, so the cost sits
    /// on a keypress the user chose rather than on every keystroke — the opposite of asking
    /// for them up front and drawing one set in fifty.
    pub fn toggle_expand(&mut self) {
        let Some(conv) = self.selected_conv().map(str::to_owned) else {
            return;
        };
        let expanding = rows::toggle(&mut self.expanded, &conv);
        if expanding && self.groups.iter().all(|g| g.hits.is_empty()) {
            self.hydrate_hits();
        }
        self.rebuild_rows(Some(&conv));
    }

    /// Re-run the current query asking for nested hits this time.
    ///
    /// A failure leaves the existing groups alone: an expansion that cannot find its
    /// snippets should show the conversation without them, not empty the list.
    fn hydrate_hits(&mut self) {
        let query = self.parsed();
        if !query.is_searchable() {
            return;
        }
        let q = cs_core::SearchOptions {
            limit: self.limit,
            nested: rows::MAX_HITS,
            ..cs_core::SearchOptions::new(self.now)
        };
        if let Ok(groups) = cs_core::search_grouped(&self.conn, &query, &q) {
            self.groups = groups;
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        let at = byte_index(&self.query, self.cursor);
        self.query.insert(at, ch);
        self.cursor += 1;
        self.search();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let range = byte_index(&self.query, self.cursor - 1)..byte_index(&self.query, self.cursor);
        self.query.replace_range(range, "");
        self.cursor -= 1;
        self.search();
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.query.chars().count() {
            return;
        }
        let range = byte_index(&self.query, self.cursor)..byte_index(&self.query, self.cursor + 1);
        self.query.replace_range(range, "");
        self.search();
    }

    /// The event for opening the selected conversation.
    ///
    /// Unlike `cs pick`, nothing is recomputed: this list is the one the ranking produced, so
    /// the rank is already known. `shown` is conversation ids in rank order — a hit row
    /// resolves to its parent, because the conversation is what was opened.
    pub fn pick_event(&mut self) -> Option<Event> {
        let conv_id = self.selected_conv()?.to_owned();
        let shown: Vec<String> = self.groups.iter().map(|g| g.conv_id.clone()).collect();
        let n = shown.len();
        let rank = shown.iter().position(|c| *c == conv_id).map(|i| i + 1);
        self.picked = true;
        Some(Event::Pick {
            ts: cs_core::now_ms(),
            q: self.query.clone(),
            // `source` is for a client whose filter arrives beside the query, as `--source`
            // does on the CLI. This one has no such filter left: `q` carries the `agent:`
            // token verbatim, so replaying it reproduces the result set exactly, which a
            // separate field could only ever promise.
            source: None,
            conv_id,
            rank,
            shown: querylog::truncate_shown(shown),
            n,
        })
    }

    /// The event for quitting without opening anything.
    ///
    /// `None` unless a real query went unanswered — a blank query has no information need
    /// behind it, and a session that ended in a pick already recorded what mattered. This is
    /// the only way the log ever says the ranking showed nothing worth opening.
    pub fn abandon_event(&self) -> Option<Event> {
        if self.picked || self.browsing() {
            return None;
        }
        let shown: Vec<String> = self.groups.iter().map(|g| g.conv_id.clone()).collect();
        let n = shown.len();
        Some(Event::Search {
            ts: cs_core::now_ms(),
            q: self.query.clone(),
            // See `pick_event`: the filter is in `q`.
            source: None,
            shown: querylog::truncate_shown(shown),
            n,
            ms: self.last_ms,
        })
    }
}

/// Char index to byte offset, saturating at the end so a caret past the last char is the
/// end rather than a panic.
fn byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map(|(i, _)| i).unwrap_or(s.len())
}

/// Per-source conversation counts, ordered by id so the facet bar never reshuffles between
/// runs. A read failure yields no facets rather than failing startup: the bar is a census,
/// and losing it costs orientation, not the ability to search.
fn read_facets(conn: &Connection) -> Vec<(String, i64)> {
    let Ok(mut stmt) =
        conn.prepare("SELECT source, COUNT(*) FROM conversation GROUP BY source ORDER BY source")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_core::model::{Conversation, Kind, Message, Role, Titles};

    #[test]
    fn one_query_decides_whether_the_list_is_matches_or_recent() {
        // `search` and `rebuild_rows` used to answer this from different inputs: `search`
        // blanked the query text before asking, so it saw "blank" while `rebuild_rows` asked
        // the raw query and saw "not blank" for the same state on screen. Latent rather than
        // visible — the browse path returns groups with no hits, so the two row vectors came
        // out identical anyway — but it is two derivations of one fact, and the second one
        // becomes wrong the moment the recent list carries hits.
        let mut app = app_with(&[("a", "alpha beta"), ("b", "alpha gamma")]);

        app.query = "al".into();
        app.search();
        assert_eq!(app.mode(), cs_core::Mode::TooShort, "a lone two-character term is held");
        assert!(app.browsing(), "so the list is recent conversations, not matches");
        let before = app.rows.clone();

        // Tab, which rebuilds rows without re-querying — the path that used to disagree.
        app.toggle_expand();
        assert_eq!(app.rows, before, "expansion cannot change a list that is not matches");

        app.query = "alpha".into();
        app.search();
        assert_eq!(app.mode(), cs_core::Mode::Searchable);
        assert!(!app.browsing(), "now the rows are matches and expansion means something");
    }

    /// Two conversations, both matching `alpha`, ordered so the ranking has a choice to make.
    fn app_with(convs: &[(&str, &str)]) -> App {
        let mut conn = cs_core::open(":memory:").unwrap();
        let built: Vec<Conversation> = convs
            .iter()
            .enumerate()
            .map(|(i, (id, text))| Conversation {
                source: "codex".into(),
                native_id: (*id).into(),
                titles: Titles {
                    custom: None,
                    generated: Some((*id).into()),
                    first_user: None,
                },
                cwd: Some("/tmp/p".into()),
                git_branch: None,
                model: None,
                surface: None,
                forked_from_native_id: None,
                head_native_id: None,
                messages: vec![Message {
                    native_id: format!("{id}-m0"),
                    parent_native_id: None,
                    thread_key: "main".into(),
                    is_sidechain: false,
                    is_error: false,
                    seq: 0,
                    role: Role::User,
                    kind: Kind::Prose,
                    ts: Some(i as i64 * 1000),
                    text: (*text).into(),
                }],
            })
            .collect();
        cs_core::write_conversations(&mut conn, &built).unwrap();
        App::new(conn, &crate::Opts { query: String::new(), source: None, limit: 50 }, Theme::plain())
            .unwrap()
    }

    #[test]
    fn a_query_edit_puts_the_cursor_on_the_new_best_answer() {
        // Caught by running the thing: the cursor used to follow the previously-selected
        // conversation across a keystroke, so narrowing a query scrolled you *away* from the
        // results the narrowing just promoted. A real session logged rank 15 after two
        // presses of Down from the top.
        let mut app = app_with(&[("c0", "alpha beta"), ("c1", "alpha gamma")]);
        app.move_selection(1);
        assert_eq!(app.selected, 1, "precondition: cursor moved off the top");

        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        assert_eq!(app.selected, 0, "a query edit resets to the best answer");
    }

    #[test]
    fn the_header_can_say_how_many_conversations_the_limit_hid() {
        // Without this the two situations that matter most — a list that is the whole answer
        // and a list that is the first page of a long one — draw the same screen.
        let mut app = app_with(&[("c0", "alpha beta"), ("c1", "alpha gamma")]);
        app.limit = 1;
        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        assert_eq!(app.groups.len(), 1, "one row, because that is all limit allows");
        assert_eq!(app.matched, 2, "and the header can still say there were two");
    }

    #[test]
    fn a_failed_search_leaves_the_count_beside_the_rows_it_describes() {
        // The rows survive a failed search (`me9.5`), so the number describing them has to
        // as well — a zero here would report "nothing matched" for a query never run.
        let mut app = app_with(&[("c0", "alpha beta"), ("c1", "alpha gamma")]);
        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        let (rows, matched) = (app.groups.len(), app.matched);
        assert_eq!(matched, 2, "precondition: a search that worked");

        app.conn.execute_batch("DROP TABLE fts_prose").unwrap();
        app.insert_char('x');
        assert!(app.status.is_some(), "the search failed");
        assert_eq!(app.groups.len(), rows, "and left the rows alone");
        assert_eq!(app.matched, matched, "along with the count that describes them");
    }

    #[test]
    fn expanding_keeps_the_cursor_on_its_conversation() {
        let mut app = app_with(&[("c0", "alpha beta"), ("c1", "alpha gamma")]);
        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        app.move_selection(1);
        let before = app.selected_conv().map(str::to_owned);
        app.toggle_expand();
        assert_eq!(app.selected_conv().map(str::to_owned), before, "expansion is not a new question");
    }

    #[test]
    fn quitting_a_blank_session_logs_nothing() {
        // A blank query has no information need behind it, so there is nothing for `6eb.21`
        // to learn from abandoning one.
        let app = app_with(&[("c0", "alpha")]);
        assert!(app.abandon_event().is_none());
    }

    #[test]
    fn quitting_an_unanswered_query_is_the_abandonment_signal() {
        let mut app = app_with(&[("c0", "alpha")]);
        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        assert!(matches!(app.abandon_event(), Some(Event::Search { .. })));
    }

    #[test]
    fn a_session_that_ended_in_a_pick_does_not_also_report_abandonment() {
        let mut app = app_with(&[("c0", "alpha")]);
        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        assert!(app.pick_event().is_some());
        assert!(app.abandon_event().is_none(), "a pick already recorded what mattered");
    }

    #[test]
    fn choosing_a_facet_puts_the_filter_in_the_box_the_user_is_looking_at() {
        // The bead: a source chosen from the bar used to live in an `App.source` field, so it
        // filtered the list while being invisible in the input — nothing to see, edit, or copy
        // out. The click is now an edit to the query text like any other.
        let mut app = app_with(&[("c0", "alpha")]);
        app.toggle_source(Some("codex".into()));
        assert_eq!(app.query, "agent:codex ");
        assert_eq!(app.selected_sources().include, ["codex"]);
        assert_eq!(app.cursor, app.query.chars().count(), "and the caret is ready to type after it");

        // The chip that is on is the chip you press to turn it off, so the bar needs no
        // separate gesture for "all" beyond the chip already labelled All.
        app.toggle_source(Some("codex".into()));
        assert_eq!(app.query, "", "pressing it again takes the token back out");
        assert!(app.selected_sources().is_empty());
    }

    #[test]
    fn a_second_chip_adds_to_the_selection_rather_than_replacing_it() {
        let mut app = app_with(&[("c0", "alpha")]);
        app.toggle_source(Some("codex".into()));
        app.toggle_source(Some("claude-code".into()));
        assert_eq!(app.selected_sources().include, ["codex", "claude-code"]);
        // And the All chip is the one gesture that clears the facet outright.
        app.toggle_source(None);
        assert!(app.selected_sources().is_empty());
        assert_eq!(app.query, "");
    }

    #[test]
    fn clicking_a_facet_mid_query_does_not_disturb_what_is_being_typed() {
        // The filter token goes in front of the free text, so the caret parked at the end of
        // the box is still at the end of the *word*, and the ranking of that word is unmoved.
        let mut app = app_with(&[("c0", "alpha beta")]);
        for ch in "alph".chars() {
            app.insert_char(ch);
        }
        let before = app.parsed().match_expr();
        app.toggle_source(Some("codex".into()));
        assert_eq!(app.query, "agent:codex alph");
        assert_eq!(app.parsed().match_expr(), before, "the facet bar does not rank anything");
        app.insert_char('a');
        assert_eq!(app.query, "agent:codex alpha", "typing carries on where it left off");
    }

    #[test]
    fn the_source_flag_arrives_as_query_text_and_is_then_just_a_query() {
        // `--source` is desugared once, at startup, into the same token a click writes. After
        // that the TUI has no notion of a source flag at all — which is the whole point.
        let conn = cs_core::open(":memory:").unwrap();
        let opts = crate::Opts { query: "alpha".into(), source: Some("Codex".into()), limit: 50 };
        let mut app = App::new(conn, &opts, Theme::plain()).unwrap();
        assert_eq!(app.query, "agent:codex alpha");
        assert_eq!(app.selected_sources().include, ["codex"]);
        // So the chip it lit is the chip that turns it off.
        app.toggle_source(Some("codex".into()));
        assert_eq!(app.query, "alpha");
    }

    #[test]
    fn selecting_out_of_range_is_ignored_rather_than_clamped() {
        // A click lands on a row number, and a stale one can name a row that no longer
        // exists. Clamping would silently select something the user did not point at.
        let mut app = app_with(&[("c0", "alpha"), ("c1", "alpha beta")]);
        let before = app.selected;
        app.select(999);
        assert_eq!(app.selected, before);
    }

    #[test]
    fn a_pick_carries_the_rank_it_was_chosen_from() {
        let mut app = app_with(&[("c0", "alpha beta"), ("c1", "alpha gamma")]);
        for ch in "alpha".chars() {
            app.insert_char(ch);
        }
        app.move_selection(1);
        let Some(Event::Pick { rank, n, conv_id, .. }) = app.pick_event() else {
            panic!("expected a pick");
        };
        assert_eq!(rank, Some(2), "1-based rank within the list actually shown");
        assert_eq!(n, 2);
        assert!(conv_id.ends_with("c1"));
    }
}
