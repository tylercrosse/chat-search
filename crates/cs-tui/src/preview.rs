//! The conversation, drawn (docs/TUI-DESIGN.md §8).
//!
//! The *rules* — which messages are drawn, how each kind folds by default, and which matches
//! may claim to have ranked the conversation — live in [`cs_core::blocks`], because the TUI is
//! no longer the only thing that has to answer them. What is left here is this terminal's
//! answer to them: rows, wrapping, gutters, sigils and styles.
//!
//! Folding is the whole design. Prose is 24% of this corpus; `tool_call` and `tool_result`
//! are 33% each and `reasoning` 8% (measured 2026-07-30 over 168k messages). Rendering all
//! of it verbatim is a file dump with a conversation buried in it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cs_core::blocks::{self, Block, Density, Fold, MarkKind};
use cs_core::highlight::Span as Mark;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::text;
use crate::theme::Theme;

pub struct Preview {
    pub conv_id: String,
    blocks: Vec<Block>,
    /// The terms the blocks were marked against. Half of this preview's cache key — the caller
    /// compares it to decide whether a still-selected conversation needs reloading, because a
    /// query edit changes the marks without changing the conversation.
    pub terms: Vec<String>,
    pub density: Density,
    /// Explicit per-message folds, which always beat the density default.
    overrides: HashMap<String, Fold>,
    /// Index into the drawn blocks.
    pub focus: usize,
    pub scroll: u16,
    /// Rows from the last layout, and what they were built for.
    ///
    /// [`Preview::layout`] does not depend on `scroll`, so scrolling — the one thing done at
    /// speed — was rebuilding every row of the conversation to move the viewport a few lines.
    /// Measured on the corpus's longest conversation: 38.9 ms to lay out 9,261 rows, of which
    /// ~85% is wrapping, and none of it changes as the pane scrolls.
    ///
    /// Keyed rather than eagerly cleared, so a resize or a fold can never be served rows
    /// measured for a different pane. `Rc` because the point is to stop rebuilding these, and
    /// handing out a deep copy instead would spend most of what the cache saves.
    cache: RefCell<Option<(LayoutKey, Rc<Vec<Row>>)>>,
    /// Bumped by every edit that changes which rows exist.
    generation: u64,
}

/// What a cached layout was built for. Anything here changing means the rows must be rebuilt.
///
/// `scroll` is deliberately absent — that is the whole saving. `focus` is present because it
/// styles the focused message's chevron, and `generation` covers folds and density.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LayoutKey {
    width: usize,
    focus: usize,
    generation: u64,
}

impl Preview {
    /// Read one conversation's messages and open on the first that matched.
    ///
    /// The read, the head-path rule and the marking pass are [`cs_core::blocks::load`]; what is
    /// added here is the anchor, which is a property of a pane that has somewhere to scroll to
    /// rather than of the conversation.
    pub fn load(conn: &Connection, conv_id: &str, terms: &[String]) -> rusqlite::Result<Self> {
        let blocks = blocks::load(conn, conv_id, terms)?;
        let focus = anchor(&blocks);
        Ok(Self {
            conv_id: conv_id.to_string(),
            blocks,
            terms: terms.to_vec(),
            density: Density::Full,
            overrides: HashMap::new(),
            focus,
            scroll: 0,
            cache: RefCell::new(None),
            generation: 0,
        })
    }

    pub fn drawn(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(|b| b.drawn())
    }

    pub fn drawn_count(&self) -> usize {
        self.blocks.iter().filter(|b| b.drawn()).count()
    }

    /// Resolve a block's fold: an explicit override, else the default for its band under the
    /// current density.
    ///
    /// The override is session state and lives here; the default is a rule every client shares
    /// and lives in core.
    pub fn fold_of(&self, block: &Block) -> Fold {
        if let Some(explicit) = self.overrides.get(&block.msg_id) {
            return *explicit;
        }
        self.density.default_fold(block.band())
    }

    pub fn cycle_density(&mut self) {
        self.density = match self.density {
            Density::Full => Density::Outline,
            Density::Outline => Density::Full,
        };
        // Density is a statement about the default, so a previous per-message decision made
        // against the *old* default is not evidence about this one.
        self.overrides.clear();
        self.scroll = 0;
        self.resized();
    }

    /// Expand or collapse everything, whichever leaves more of the conversation visible than
    /// is showing now.
    pub fn toggle_all(&mut self) {
        let expanded = self
            .drawn()
            .filter(|b| self.fold_of(b) == Fold::Expanded)
            .count();
        let target = if expanded * 2 >= self.drawn_count() { Fold::Collapsed } else { Fold::Expanded };
        let ids: Vec<String> = self.drawn().map(|b| b.msg_id.clone()).collect();
        self.overrides = ids.into_iter().map(|id| (id, target)).collect();
        self.scroll = 0;
        self.resized();
    }

    pub fn toggle_focused(&mut self) {
        let Some(block) = self.drawn().nth(self.focus) else { return };
        let next = match self.fold_of(block) {
            Fold::Collapsed => Fold::Expanded,
            Fold::Expanded => Fold::Collapsed,
        };
        let id = block.msg_id.clone();
        self.overrides.insert(id, next);
        self.resized();
    }

    pub fn move_focus(&mut self, delta: isize) {
        let n = self.drawn_count();
        if n == 0 {
            self.focus = 0;
            return;
        }
        self.focus = (self.focus as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    /// Scroll so the focused message is on screen.
    ///
    /// Focus without this is focus you cannot see — the marker moves to message 40 while the
    /// pane still shows 1 to 20. Measured off [`Preview::layout`] rather than from a row count
    /// of its own, because with wrapping a message's height depends on the pane width and on
    /// where each line happens to break.
    pub fn follow_focus(&mut self, theme: &Theme, width: usize, viewport_rows: usize) {
        let rows = self.layout(theme, width);
        let owned = |want: usize| {
            rows.iter()
                .enumerate()
                .filter(move |(_, r)| r.owner.is_some_and(|(i, _)| i == want))
                .map(|(y, _)| y)
        };
        let Some(before) = owned(self.focus).next() else { return };
        // One past the focused message's last row, so a message taller than the viewport pins
        // its bottom rather than scrolling past it.
        let bottom = owned(self.focus).last().map_or(before, |y| y + 1);
        if bottom > (self.scroll as usize) + viewport_rows {
            self.scroll = bottom.saturating_sub(viewport_rows) as u16;
        } else if before < self.scroll as usize {
            self.scroll = before as u16;
        }
    }

    /// Which drawn block owns `row`, and whether `row` is that block's header — the collapsed
    /// one-liner, or the speaker line of an expanded block.
    ///
    /// Read straight off [`Preview::layout`], which is also what gets drawn. It used to be
    /// separate arithmetic over a per-block row count, and that only survived while one source
    /// line was exactly one terminal row; wrapping ends that, and two derivations of the same
    /// geometry would have drifted the moment a line wrapped — invisibly, until a click folded
    /// the wrong message. Same reason [`crate::render::first_visible`] is shared.
    ///
    /// The header/body distinction is what lets a click into body text move focus without
    /// collapsing the block out from under the pointer.
    pub fn block_at_row(&self, theme: &Theme, width: usize, row: usize) -> Option<(usize, bool)> {
        self.layout(theme, width).get(row).and_then(|r| r.owner)
    }

    /// Scroll the pane, stopping at the end of the conversation.
    ///
    /// Clamped against the rendered height rather than left to run up to `u16::MAX`. Scrolling
    /// past the last line used to keep counting: the pane went blank with nothing to say how far
    /// it had gone, and getting back meant winding the same distance in reverse. The floor is
    /// the last screenful rather than the last row, because a pane showing one line of a
    /// conversation and twenty of nothing is the same failure in miniature.
    pub fn scroll_by(&mut self, theme: &Theme, width: usize, viewport_rows: usize, delta: isize) {
        let total = self.height(theme, width);
        let max = total.saturating_sub(viewport_rows.max(1)) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max.max(0)) as u16;
    }

    /// Rows the conversation renders to at `width`. Served from the layout cache, so a scroll
    /// burst asks for it once per notch and pays for it once.
    fn height(&self, theme: &Theme, width: usize) -> usize {
        self.layout(theme, width).len()
    }

    /// Mark the drawn rows stale. Called by every edit that changes which rows exist — a fold
    /// changes both the row count and the row contents, and serving the pre-fold layout would
    /// let scrolling run past the new end and clicks land on the wrong message.
    fn resized(&mut self) {
        self.generation += 1;
    }

    /// Fold or unfold block `i`, which a click has just put the focus on.
    pub fn toggle_at(&mut self, i: usize) {
        self.focus = i;
        self.toggle_focused();
    }

    /// Where the focused message sits in the conversation, for the header.
    pub fn position(&self) -> (usize, usize) {
        (self.focus + 1, self.drawn_count())
    }

    /// Render to lines, with the query's matches marked.
    ///
    /// The marks were located at load against the same matcher that ranked the conversation
    /// (`chat-search-6eb.20`), so a stemmed hit marks the word that actually matched rather than
    /// marking nothing — which is worse than marking nothing at all, because it looks deliberate.
    pub fn lines(&self, theme: &Theme, width: usize) -> Vec<Line<'static>> {
        self.layout(theme, width).iter().map(|r| r.line.clone()).collect()
    }

    /// Every row the pane draws, each tagged with the message it belongs to.
    ///
    /// The single source of geometry: what gets drawn, what a click resolves to, and what
    /// scrolling counts all come out of this one walk, so they cannot disagree about how tall a
    /// message is. That matters most where a body line wraps — its height is a function of the
    /// pane width and of where the text happens to break, which no independent arithmetic could
    /// reproduce without eventually getting it wrong.
    pub fn layout(&self, theme: &Theme, width: usize) -> Rc<Vec<Row>> {
        let key = LayoutKey { width, focus: self.focus, generation: self.generation };
        // Scoped so the shared borrow is released before `build_rows` can ask for the mutable
        // one — `RefCell` would panic at runtime rather than fail to compile.
        {
            if let Some((cached, rows)) = self.cache.borrow().as_ref() {
                if *cached == key {
                    return Rc::clone(rows);
                }
            }
        }
        let rows = Rc::new(self.build_rows(theme, width));
        *self.cache.borrow_mut() = Some((key, Rc::clone(&rows)));
        rows
    }

    fn build_rows(&self, theme: &Theme, width: usize) -> Vec<Row> {
        let mut out = Vec::new();
        for (i, block) in self.drawn().enumerate() {
            let focused = i == self.focus;
            let fold = self.fold_of(block);
            // The glyph says folded-or-not and the style says focused-or-not, so one column
            // carries both and neither is encoded in colour alone (§7). It also settles a
            // collision: the results pane already spends ▸/▾ on fold state, and the preview
            // used to spend ▸ on focus — the same glyph meaning two things on one screen.
            let chevron = match fold {
                Fold::Collapsed => "▸",
                Fold::Expanded => "▾",
            };
            let chevron_style = if focused { theme.selected } else { theme.dim };
            let gutter = Span::styled(format!("{chevron} "), chevron_style);
            match fold {
                Fold::Collapsed => {
                    let mut runs = vec![gutter];
                    runs.extend(collapsed_runs(block, theme));
                    // Collapsed is one row by definition — it is the form that stands in for a
                    // message too long to show, so wrapping it would defeat the fold.
                    out.push(Row::header(i, Line::from(text::truncate_runs(runs, width))));
                }
                Fold::Expanded => {
                    out.push(Row::header(
                        i,
                        Line::from(vec![
                            gutter,
                            Span::styled(speaker(block), speaker_style(block, theme)),
                        ]),
                    ));
                    let mut fence = Fence::default();
                    for (at, raw) in offset_lines(&block.text) {
                        let (indent, base) = fence.enter(raw, theme);
                        // The gutter is spent out of the same budget as the text, so a wrapped
                        // or marked run cannot push the line into the results column. It is
                        // repeated on every wrapped row, which is what keeps a long paragraph
                        // and a fenced block visually distinct all the way down.
                        let marks = rebase(&block.marks, at, raw.len(), 0);
                        let body = text::marked_runs(raw, &marks, base, mark_style(block, theme));
                        for wrapped in wrap_body(indent.clone(), body, theme, width) {
                            out.push(Row::body(i, wrapped));
                        }
                    }
                    out.push(Row::body(i, Line::raw("")));
                }
            }
        }
        if out.is_empty() {
            out.push(Row {
                line: Line::from(Span::styled("  no messages on the head path", theme.dim)),
                owner: None,
            });
        }
        out
    }
}

/// One drawn row of the preview, and which message put it there.
pub struct Row {
    pub line: Line<'static>,
    /// Index into the drawn blocks, plus whether this row is that block's header. `None` for a
    /// row that belongs to no message, which today is only the empty-conversation notice.
    pub owner: Option<(usize, bool)>,
}

impl Row {
    fn header(block: usize, line: Line<'static>) -> Self {
        Row { line, owner: Some((block, true)) }
    }

    fn body(block: usize, line: Line<'static>) -> Self {
        Row { line, owner: Some((block, false)) }
    }
}

/// Which drawn block the preview opens on: the first that matched, else the top.
///
/// §8 says to anchor on the best hit and points at `Group::match_seqs` for it. That cannot work
/// here. `match_seqs` is a flat list of message *positions*, while [`Preview::load`] orders by
/// `(is_sidechain, thread_key, seq)` so each strand reads contiguously — on the multi-thread
/// conversations this corpus contains the two orders are simply different, and a position
/// resolved against the wrong one anchors on an unrelated message. The marks are per-message and
/// come from the matcher that did the ranking, so they answer the same question locally and
/// cannot be misaligned.
///
/// A conversation nothing matched — a blank query, or a hit that was in a tool field rather than
/// prose — opens at the top, which is the honest place when there is no better one.
fn anchor(blocks: &[Block]) -> usize {
    blocks.iter().filter(|b| b.drawn()).position(|b| !b.marks.is_empty()).unwrap_or(0)
}

/// Who is talking, for an expanded block.
fn speaker(block: &Block) -> String {
    let who = match block.role.as_str() {
        "user" => "» you",
        "assistant" => "assistant",
        other => other,
    };
    // A subagent turn is not the conversation you were having, so it says so rather than
    // sitting in the transcript pretending to be a reply you received.
    if block.is_sidechain { format!("{who} [subagent]") } else { who.to_string() }
}

fn speaker_style(block: &Block, theme: &Theme) -> Style {
    if block.role == "user" { theme.accent } else { theme.header }
}

/// This terminal's answer to [`MarkKind`] — which layer-3 style a match is entitled to.
///
/// Core decides *whether* a match may claim to have ranked the conversation; the two styles
/// here are how that claim is said out loud. They are told apart by a text modifier rather than
/// a colour, because §7 forbids encoding anything in colour alone and `NO_COLOR` is exactly
/// where a reader most needs to know which of the two they are looking at.
fn mark_style(block: &Block, theme: &Theme) -> Style {
    match block.mark_kind() {
        MarkKind::Ranked => theme.match_hl,
        MarkKind::Unranked => theme.match_unranked,
    }
}

/// One line standing in for a whole message, with any matches in it marked.
///
/// Untruncated: the caller prepends the fold chevron and spends one budget on the whole line,
/// because a mark is only useful if it is still on screen.
fn collapsed_runs(block: &Block, theme: &Theme) -> Vec<Span<'static>> {
    // Checked before `kind`, because a failed result is the one result that survives and it
    // is the failure, not the tool, that is worth the row.
    if block.is_error {
        let (at, line) = recognition_line(block);
        let sigil = "✗ ";
        let marks = rebase(&block.marks, at, line.len(), sigil.len());
        return text::marked_runs(&format!("{sigil}{line}"), &marks, theme.error, theme.match_hl);
    }
    let (body, marks, style) = match block.kind.as_str() {
        // Reasoning opens with its own bolded summary — `**Planning README creation**` — so
        // where nothing matched, the model has already written the one-line version. Use it
        // rather than counting lines at the user.
        "reasoning" => {
            let (at, line) = recognition_line(block);
            // The asterisks are the model's own emphasis syntax rather than content, and
            // dropping them moves every offset after them.
            let shown = line.trim_matches('*').to_string();
            let sigil = "⋯ ";
            let dropped = line.find(&shown).unwrap_or(0);
            let marks = rebase(&block.marks, at + dropped, shown.len(), sigil.len());
            (format!("{sigil}{shown}"), marks, theme.dim)
        }
        // The one collapsed form that is *synthesised* rather than quoted, so it is the one
        // that cannot show the line a query hit — and the one whose offsets would be fiction.
        "tool_call" => (format!("⚙ {}", tool_summary(&block.text)), Vec::new(), theme.dim),
        _ => {
            let (at, line) = recognition_line(block);
            let sigil = format!("{} ", speaker(block));
            let marks = rebase(&block.marks, at, line.len(), sigil.len());
            (format!("{sigil}{line}"), marks, Style::new())
        }
    };
    text::marked_runs(&body, &marks, style, mark_style(block, theme))
}

/// Marks rebased from `block.text` onto a line lifted out of it at `at`, then shifted by the
/// `prefix` the rendered form puts in front of it.
///
/// Dropping marks that straddle the slice rather than clamping them: a clamped mark would
/// highlight a word fragment that no term actually matched, which is the same class of lie as
/// the unlocated snippet `UNLOCATED` exists to label.
fn rebase(marks: &[Mark], at: usize, len: usize, prefix: usize) -> Vec<Mark> {
    marks
        .iter()
        .filter(|m| m.start >= at && m.end <= at + len)
        .map(|m| Mark { start: m.start - at + prefix, end: m.end - at + prefix })
        .collect()
}

/// The one line of a message worth standing in for all of it, with its byte offset.
///
/// The line the query hit, when there is one. §8 specs this and it is the whole reason outline
/// mode is worth more than a density knob: with every message reduced to the line that matched,
/// the pane becomes a map of where the query landed across the conversation, in conversation
/// order — complementary to the top-3 hit rows in the results list rather than a duplicate of
/// them. Falling back to the first non-empty line makes a non-matching message a table of
/// contents entry, which is all it can honestly be.
fn recognition_line(block: &Block) -> (usize, String) {
    let marked = offset_lines(&block.text)
        .find(|(at, line)| block.marks.iter().any(|m| m.start >= *at && m.start < at + line.len()));
    let (at, line) = marked
        .or_else(|| offset_lines(&block.text).find(|(_, l)| !l.trim().is_empty()))
        .unwrap_or((0, ""));
    // The offset is of the untrimmed line, so a caller rebasing marks onto the returned string
    // has to account for the leading whitespace this drops.
    (at + (line.len() - line.trim_start().len()), line.trim().to_string())
}

/// One body line, broken to fit the pane, with the gutter repeated down the left of every row.
///
/// The gutter is re-emitted rather than replaced by blanks so a wrapped fenced block keeps its
/// rule for its whole height — otherwise a code block taller than one row would announce itself
/// on the first line and look like prose from the second down.
fn wrap_body(
    indent: String,
    body: Vec<Span<'static>>,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let inner = width.saturating_sub(text::width(&indent));
    let mut rows = text::wrap_runs(body, inner);
    // An empty source line is a paragraph break and has to keep its row; losing it would run
    // two paragraphs together and, worse, change the height this message reports.
    if rows.is_empty() {
        rows.push(Vec::new());
    }
    rows.into_iter()
        .map(|row| {
            let mut runs = vec![Span::styled(indent.clone(), theme.dim)];
            runs.extend(row);
            // Belt and braces: the gutter plus a wrapped row should already fit, and a cell
            // that overflows here would bleed into the results pane rather than merely look
            // wrong.
            Line::from(text::truncate_runs(runs, width))
        })
        .collect()
}

/// Whether the walk down a message is currently inside a ``` fence.
///
/// §8: "render fenced blocks dim, keep the match highlight applied inside them, stop." The
/// alternative — the flat keyword union fast-resume spends ~150 lines on — is barely
/// language-aware and highlights `def` inside JavaScript, and a real grammar costs 2.1 MB of
/// binary and ~110 µs per line to colour the 3.9% of prose messages whose fence even names a
/// language. Set against a pane whose job is recognising a conversation, that is a bad trade.
///
/// The rule in the gutter is not decoration: §7 forbids encoding anything in colour alone, and
/// dim is an intensity, so a monochrome terminal would otherwise see no block at all.
#[derive(Default)]
struct Fence {
    inside: bool,
}

impl Fence {
    /// The gutter and body style for the next line, advancing the fence state.
    ///
    /// The ``` markers take the rule too. They are part of the block they delimit, and keeping
    /// them means every source line still renders as a line — which is what lets
    /// [`Preview::rendered_rows`] stay exact, and therefore what keeps a click on the message
    /// it was aimed at.
    fn enter(&mut self, raw: &str, theme: &Theme) -> (String, Style) {
        let marker = raw.trim_start().starts_with("```");
        let inside = self.inside || marker;
        if marker {
            self.inside = !self.inside;
        }
        // Two columns either way, so code and prose stay on the same left edge.
        if inside {
            ("┃ ".to_string(), theme.dim)
        } else {
            ("  ".to_string(), Style::new())
        }
    }
}

/// The lines of a message with their byte offsets, so a mark can be attributed to the line
/// holding it.
///
/// Splits exactly where `str::lines` splits — `split_inclusive` yields one chunk per line and
/// agrees with it on a trailing newline, where a plain `split('\n')` would report one line too
/// many. That agreement is load-bearing twice over: [`Preview::rendered_rows`] counts rows with
/// it, and [`Preview::block_at_row`] turns a click into a message with those counts, so an
/// off-by-one on the last line of any expanded message would fold the wrong one.
fn offset_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n').map(move |chunk| {
        let start = at;
        at += chunk.len();
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        (start, line.strip_suffix('\r').unwrap_or(line))
    })
}

/// `name` plus whatever argument identifies the call.
///
/// A `tool_call` body is the tool name on its own line followed by a JSON argument object,
/// so the name is free and the useful part is picking the one argument a human would use to
/// recognise the call — the command for a shell, the path for a read.
fn tool_summary(text: &str) -> String {
    let mut lines = text.lines();
    let name = lines.next().unwrap_or("tool").trim().to_string();
    let rest: String = lines.collect::<Vec<_>>().join("\n");
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&rest) else {
        return name;
    };

    // `command` is spelled differently by each tool and both spellings are common here:
    // Codex writes an argv array, Claude Code a single string. Handling only the array meant
    // every Claude Code `Bash` call collapsed to a bare `⚙ Bash` — the name with the one
    // piece of information stripped out.
    if let Some(cmd) = json.get("command") {
        if let Some(line) = cmd.as_str() {
            return format!("{name} {}", line.replace('\n', " "));
        }
        if let Some(argv) = cmd.as_array() {
            let parts: Vec<&str> = argv.iter().filter_map(|v| v.as_str()).collect();
            // `["bash", "-lc", "<script>"]` is the overwhelmingly common argv shape, and the
            // script is the only interesting element — the shell and its flag never vary.
            let shown = match parts.as_slice() {
                [_shell, flag, script, ..] if flag.starts_with('-') => script,
                _ => return format!("{name} {}", parts.join(" ")),
            };
            return format!("{name} {shown}");
        }
    }
    for key in ["path", "file_path", "pattern", "query", "url"] {
        if let Some(v) = json.get(key).and_then(|v| v.as_str()) {
            return format!("{name} {v}");
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_core::highlight;

    fn block(kind: &str, role: &str, id: &str, text: &str) -> Block {
        Block {
            msg_id: id.into(),
            role: role.into(),
            kind: kind.into(),
            seq: 0,
            on_path: true,
            text: text.into(),
            is_sidechain: false,
            thread_key: "main".into(),
            is_error: false,
            marks: Vec::new(),
        }
    }

    /// A block with its matches already located, as [`Preview::load`] would leave it.
    fn marked(kind: &str, role: &str, id: &str, text: &str, query: &str) -> Block {
        let mut b = block(kind, role, id, text);
        b.marks = highlight::spans(text, &cs_core::Query::exact(query).marking_terms());
        b
    }

    fn preview(blocks: Vec<Block>) -> Preview {
        Preview {
            conv_id: "c".into(),
            blocks,
            terms: Vec::new(),
            density: Density::Full,
            overrides: HashMap::new(),
            focus: 0,
            scroll: 0,
            cache: RefCell::new(None),
            generation: 0,
        }
    }

    #[test]
    fn tool_results_are_omitted_entirely() {
        let p = preview(vec![
            block("tool_call", "assistant", "a", "shell\n{\"command\":[\"bash\",\"-lc\",\"ls\"]}"),
            block("tool_result", "tool", "b", "a very long directory listing"),
            block("prose", "assistant", "c", "here is what I found"),
        ]);
        assert_eq!(p.drawn_count(), 2, "the result is not a row, not even a collapsed one");
        assert!(p.drawn().all(|b| b.kind != "tool_result"));
    }

    #[test]
    fn full_density_expands_prose_and_collapses_the_rest() {
        let p = preview(vec![
            block("prose", "user", "a", "what does this do"),
            block("reasoning", "assistant", "b", "**Planning**\nlots of detail"),
            block("tool_call", "assistant", "c", "shell\n{}"),
        ]);
        assert_eq!(p.fold_of(&p.blocks[0]), Fold::Expanded);
        assert_eq!(p.fold_of(&p.blocks[1]), Fold::Collapsed);
        assert_eq!(p.fold_of(&p.blocks[2]), Fold::Collapsed);
    }

    #[test]
    fn outline_collapses_everything_including_prose() {
        let mut p = preview(vec![block("prose", "user", "a", "what does this do")]);
        p.cycle_density();
        assert_eq!(p.density, Density::Outline);
        assert_eq!(p.fold_of(&p.blocks[0]), Fold::Collapsed);
    }

    #[test]
    fn an_explicit_fold_beats_the_density_default() {
        let mut p = preview(vec![block("tool_call", "assistant", "a", "shell\n{}")]);
        assert_eq!(p.fold_of(&p.blocks[0]), Fold::Collapsed);
        p.toggle_focused();
        assert_eq!(p.fold_of(&p.blocks[0]), Fold::Expanded, "the user asked for this one");
    }

    #[test]
    fn changing_density_forgets_per_message_decisions() {
        // An override was a decision about the old default. Carrying it across means
        // switching to outline leaves a message stubbornly expanded for no visible reason.
        let mut p = preview(vec![block("tool_call", "assistant", "a", "shell\n{}")]);
        p.toggle_focused();
        p.cycle_density();
        assert_eq!(p.fold_of(&p.blocks[0]), Fold::Collapsed);
    }

    #[test]
    fn toggle_all_moves_toward_whichever_state_is_not_dominant() {
        let mut p = preview(vec![
            block("prose", "user", "a", "one"),
            block("tool_call", "assistant", "b", "shell\n{}"),
            block("tool_call", "assistant", "c", "shell\n{}"),
        ]);
        // Two of three collapsed, so the first press expands.
        p.toggle_all();
        assert!(p.drawn().all(|b| p.fold_of(b) == Fold::Expanded));
        p.toggle_all();
        assert!(p.drawn().all(|b| p.fold_of(b) == Fold::Collapsed));
    }

    #[test]
    fn focus_stays_inside_the_drawn_blocks() {
        let mut p = preview(vec![
            block("prose", "user", "a", "one"),
            block("tool_result", "tool", "b", "omitted"),
            block("prose", "assistant", "c", "two"),
        ]);
        p.move_focus(50);
        assert_eq!(p.focus, 1, "clamped to the two drawn blocks, not the three rows");
        assert_eq!(p.position(), (2, 2));
        p.move_focus(-50);
        assert_eq!(p.focus, 0);
    }

    #[test]
    fn a_shell_call_collapses_to_the_script_not_the_shell() {
        let t = "shell\n{\"command\":[\"bash\",\"-lc\",\"sed -n '1,160p' config.py\"],\"workdir\":\".\"}";
        assert_eq!(tool_summary(t), "shell sed -n '1,160p' config.py");
    }

    #[test]
    fn a_command_string_reads_the_same_as_a_command_array() {
        // Codex writes argv, Claude Code writes a line. Both are `command`, and handling one
        // shape silently rendered the other as a bare tool name.
        assert_eq!(tool_summary("Bash\n{\"command\":\"ls -la\"}"), "Bash ls -la");
        assert_eq!(
            tool_summary("Bash\n{\"command\":\"echo one\\necho two\"}"),
            "Bash echo one echo two",
            "a multi-line script still occupies one row"
        );
    }

    #[test]
    fn a_call_with_an_identifying_path_uses_it() {
        assert_eq!(tool_summary("Read\n{\"file_path\":\"/x/schema.rs\"}"), "Read /x/schema.rs");
    }

    #[test]
    fn an_unparseable_body_still_yields_the_tool_name() {
        // Argument shapes vary per tool and per version; the name alone is always better
        // than a raw JSON fragment wrapped into the pane.
        assert_eq!(tool_summary("update_plan\nnot json at all"), "update_plan");
        assert_eq!(tool_summary("bare_name"), "bare_name");
    }

    #[test]
    fn threads_read_one_at_a_time_rather_than_interleaved() {
        // Found by running it against a real 9-thread conversation: `seq` restarts at 0 per
        // thread, so ordering by it alone put all nine opening user turns first and all nine
        // first replies after them. The SQL does the ordering, so this pins the property the
        // SQL has to deliver — construct the blocks in the order the query returns them and
        // assert each strand is contiguous with the main thread first.
        let b = |thread: &str, side: bool, seq: i64, id: &str| Block {
            msg_id: id.into(),
            role: "user".into(),
            kind: "prose".into(),
            seq,
            on_path: true,
            text: "x".into(),
            is_sidechain: side,
            thread_key: thread.into(),
            is_error: false,
            marks: Vec::new(),
        };
        let blocks = vec![
            b("main", false, 0, "m0"),
            b("main", false, 1, "m1"),
            b("sub-a", true, 0, "a0"),
            b("sub-a", true, 1, "a1"),
        ];
        assert_eq!(blocks::thread_count(&blocks), 2);
        let p = preview(blocks);
        let order: Vec<&str> = p.drawn().map(|x| x.msg_id.as_str()).collect();
        assert_eq!(order, ["m0", "m1", "a0", "a1"]);
        assert!(!p.drawn().next().unwrap().is_sidechain, "the main strand reads first");
    }

    #[test]
    fn a_failed_tool_result_survives_the_omission() {
        // The whole reason `is_error` exists. A successful result is implied by its call and
        // costs a row to repeat; a failed one is often what makes a conversation findable.
        let mut ok = block("tool_result", "tool", "a", "3 files");
        let mut bad = block("tool_result", "tool", "b", "error: no such file or directory");
        ok.is_error = false;
        bad.is_error = true;
        let p = preview(vec![ok, bad]);
        let kept: Vec<&str> = p.drawn().map(|x| x.msg_id.as_str()).collect();
        assert_eq!(kept, ["b"], "the failure stays, the success does not");
        let runs = collapsed_runs(p.drawn().next().unwrap(), &Theme::plain());
        assert!(runs[0].content.starts_with('✗'));
    }

    #[test]
    fn a_subagent_turn_says_it_is_one() {
        let mut blk = block("prose", "assistant", "a", "hi");
        blk.is_sidechain = true;
        assert!(speaker(&blk).contains("subagent"));
    }

    #[test]
    fn moving_focus_pulls_the_viewport_after_it() {
        // Focus without scroll is focus you cannot see: the marker moved to message 40 while
        // the pane still showed messages 1-20.
        let blocks: Vec<Block> = (0..40)
            .map(|i| block("prose", "user", &format!("m{i}"), "line"))
            .collect();
        let theme = Theme::plain();
        let mut p = preview(blocks);
        p.move_focus(39);
        p.follow_focus(&theme, 80, 10);
        assert!(p.scroll > 0, "the viewport followed the cursor down");
        p.move_focus(-39);
        p.follow_focus(&theme, 80, 10);
        assert_eq!(p.scroll, 0, "and back to the top");
    }

    #[test]
    fn offset_lines_splits_exactly_where_str_lines_splits() {
        // `rendered_rows` counts with this and `block_at_row` turns clicks into messages with
        // those counts, so one extra line on a trailing newline — which `split('\n')` reports
        // and `lines()` does not — puts every click after it on the wrong message.
        for text in ["", "a", "a\n", "a\nb", "a\nb\n", "\n\n", "a\r\nb\r\n", "\n"] {
            let got: Vec<&str> = offset_lines(text).map(|(_, l)| l).collect();
            let want: Vec<&str> = text.lines().collect();
            assert_eq!(got, want, "{text:?}");
        }
        // And the offsets point at the line they came from.
        let text = "alpha\nbeta\ngamma";
        for (at, line) in offset_lines(text) {
            assert_eq!(&text[at..at + line.len()], line);
        }
    }

    #[test]
    fn every_drawn_row_maps_back_to_the_message_that_put_it_there() {
        // The invariant holding the mouse, the keyboard and the renderer together. They all read
        // `layout`, so this asserts the map it hands back is total and ordered rather than
        // asserting two derivations agree — which is the arrangement that let them drift.
        let theme = Theme::plain();
        let mut p = preview(vec![
            block("prose", "user", "a", "one\ntwo"),
            block("tool_call", "assistant", "b", "shell\n{}"),
            block("prose", "assistant", "c", "three"),
        ]);
        for width in [12usize, 40, 200] {
            for _ in 0..2 {
                let rows = p.layout(&theme, width);
                assert_eq!(rows.len(), p.lines(&theme, width).len(), "lines and layout disagree");
                let owners: Vec<_> = rows.iter().filter_map(|r| r.owner).collect();
                assert_eq!(owners.len(), rows.len(), "a row belonged to no message");
                assert_eq!(
                    owners.iter().filter(|(_, header)| *header).count(),
                    p.drawn_count(),
                    "exactly one header per drawn message, width {width}",
                );
                assert!(owners.windows(2).all(|w| w[0].0 <= w[1].0), "messages came back out of order");
                assert_eq!(owners.first().map(|o| o.1), Some(true), "the first row is a header");
                assert_eq!(p.block_at_row(&theme, width, rows.len()), None, "past the end");
                // Again with every fold flipped, so this is not an artefact of the defaults.
                p.toggle_all();
            }
        }
    }

    #[test]
    fn a_click_on_a_header_folds_and_a_click_on_body_text_only_moves_focus() {
        let theme = Theme::plain();
        let (w, mut p) = (
            80,
            preview(vec![
                block("prose", "user", "a", "one"),
                block("prose", "assistant", "b", "two\nthree"),
            ]),
        );
        // Block 0 expanded by default: header, one body line, blank. Block 1 starts at row 3.
        assert_eq!(p.block_at_row(&theme, w, 3), Some((1, true)));
        p.toggle_at(1);
        assert_eq!(p.fold_of(&p.blocks[1]), Fold::Collapsed, "its header line folded it");
        assert_eq!(p.focus, 1, "and took the focus");

        // Body text of block 0 must not fold it — reading text that collapses under the
        // pointer is the reason the header/body distinction exists.
        assert_eq!(p.block_at_row(&theme, w, 1), Some((0, false)));
        assert_eq!(p.fold_of(&p.blocks[0]), Fold::Expanded);
    }

    #[test]
    fn a_wrapped_line_stays_one_message_for_the_whole_of_its_height() {
        // Wrapping is exactly where a separate row count would have drifted: this paragraph is
        // one source line and several terminal rows, and every one of them has to resolve to the
        // message it came from or a click lands on the neighbour.
        let theme = Theme::plain();
        let long = "the borrow checker complains about a value that does not live long enough";
        let p = preview(vec![
            block("prose", "user", "a", long),
            block("prose", "assistant", "b", "short"),
        ]);
        let narrow = p.layout(&theme, 24);
        let wide = p.layout(&theme, 200);
        assert!(narrow.len() > wide.len(), "the narrow pane must need more rows");
        let first: Vec<_> = narrow.iter().filter(|r| r.owner.is_some_and(|(i, _)| i == 0)).collect();
        assert!(first.len() > 3, "the paragraph wrapped to several rows: {}", first.len());
        assert_eq!(first[0].owner, Some((0, true)), "only the speaker line is the header");
        assert!(first[1..].iter().all(|r| r.owner == Some((0, false))), "a body row became a header");
    }

    #[test]
    fn a_collapsed_message_shows_the_line_the_query_hit() {
        // §8: collapsed prose is "the matching line, else the first non-empty line". Showing
        // the first line instead makes outline mode a table of contents when its whole purpose
        // is being a map of where the query landed.
        let text = "a preamble nobody asked about\nsome filler\nthe borrow checker complained";
        let b = marked("prose", "assistant", "a", text, "borrow");
        assert!(!b.marks.is_empty(), "the fixture has to actually match");
        assert_eq!(recognition_line(&b).1, "the borrow checker complained");

        // Stemmed, because the whole point is agreeing with the ranker rather than with a
        // substring scan: `commits` ranks this row on `Commit`.
        let b = marked("prose", "user", "b", "hello there\nCommit current changes", "commits");
        assert_eq!(recognition_line(&b).1, "Commit current changes");
    }

    #[test]
    fn a_message_that_matched_nothing_falls_back_to_its_first_real_line() {
        let b = marked("prose", "user", "a", "\n\n  first words here\nlater", "elephant");
        assert!(b.marks.is_empty());
        assert_eq!(recognition_line(&b).1, "first words here");
    }

    #[test]
    fn the_preview_opens_on_a_message_that_matched_not_at_the_top() {
        // Anchoring on `Group::match_seqs` cannot do this correctly — its positions are in a
        // different order than the thread-contiguous one this pane draws — so the anchor is
        // taken from the marks, which are per-message.
        let blocks = vec![
            marked("prose", "user", "a", "an opening question", "borrow"),
            marked("prose", "assistant", "b", "some preamble", "borrow"),
            marked("prose", "user", "c", "what about the borrow checker", "borrow"),
        ];
        assert_eq!(anchor(&blocks), 2);
        assert_eq!(anchor(&[]), 0, "an empty conversation opens at the top rather than panicking");

        // Nothing matched: the top is the honest place, not message 1 of a phantom match.
        let none = vec![marked("prose", "user", "a", "an opening question", "elephant")];
        assert_eq!(anchor(&none), 0);
    }

    #[test]
    fn the_anchor_counts_drawn_blocks_so_an_omitted_result_cannot_shift_it() {
        // `focus` indexes drawn blocks; counting raw ones would put the cursor one message off
        // for every successful tool result above the match.
        let mut ok = block("tool_result", "tool", "r", "a long listing");
        ok.is_error = false;
        let blocks = vec![
            block("prose", "user", "a", "opening"),
            ok,
            marked("prose", "assistant", "b", "the borrow checker", "borrow"),
        ];
        assert_eq!(anchor(&blocks), 1, "the omitted result is not a position");
    }

    /// The text of a rendered line, with the marked runs wrapped in brackets — the only way to
    /// assert *what* got highlighted rather than merely that something did.
    ///
    /// `[word]` is a ranked match and `<word>` one the ranker never saw, because those are two
    /// different claims and a test that could not tell them apart would pass either way
    /// (chat-search-8mb).
    fn marks_of(line: &Line<'static>, theme: &Theme) -> String {
        let has = |s: &Span<'static>, m: ratatui::style::Modifier| {
            !m.is_empty() && s.style.add_modifier.contains(m)
        };
        line.spans
            .iter()
            .map(|s| {
                if has(s, theme.match_hl.add_modifier) {
                    format!("[{}]", s.content)
                } else if has(s, theme.match_unranked.add_modifier) {
                    format!("<{}>", s.content)
                } else {
                    s.content.to_string()
                }
            })
            .collect()
    }

    #[test]
    fn an_expanded_body_marks_the_word_that_ranked_the_row() {
        let theme = Theme::plain();
        let b = marked("prose", "user", "a", "nothing here\nthe borrow checker again", "borrow");
        let p = preview(vec![b]);
        let rendered: Vec<String> =
            p.lines(&theme, 80).iter().map(|l| marks_of(l, &theme)).collect();
        assert!(
            rendered.iter().any(|l| l.contains("the [borrow] checker again")),
            "the match is not marked in the body: {rendered:?}"
        );
        assert!(
            rendered.iter().any(|l| l.contains("nothing here") && !l.contains('[')),
            "a line with no match must carry no marks: {rendered:?}"
        );
    }

    #[test]
    fn a_collapsed_line_marks_the_match_through_its_sigil() {
        // The collapsed forms put a sigil in front of a line lifted out of the message, so the
        // offsets have to move by the sigil's width. Off by that much and the highlight lands
        // on the neighbouring word, which reads as a highlighter that simply does not work.
        let theme = Theme::plain();
        let mut p = preview(vec![
            marked("prose", "user", "a", "ask about the borrow checker", "borrow"),
            marked("reasoning", "assistant", "b", "**thinking about borrow rules**", "borrow"),
        ]);
        p.density = Density::Outline;
        let rendered: Vec<String> =
            p.lines(&theme, 80).iter().map(|l| marks_of(l, &theme)).collect();
        assert!(rendered[0].contains("ask about the [borrow] checker"), "{rendered:?}");
        // Angle brackets: the reasoning line is marked in the same place, but as a match the
        // ranker never saw. See `a_match_in_reasoning_is_not_dressed_as_a_reason_it_ranked`.
        assert!(rendered[1].contains("thinking about <borrow> rules"), "{rendered:?}");
    }

    #[test]
    fn a_match_in_reasoning_is_not_dressed_as_a_reason_it_ranked() {
        // chat-search-8mb. Reasoning carries no postings, so a word highlighted here is a word
        // `cs search` will not return this conversation for and `cs explain` reports zero hits
        // for. Marking it exactly like a prose hit says the opposite, and says it in the one
        // place the user has gone to check.
        let theme = Theme::plain();
        let p = preview(vec![
            marked("prose", "user", "a", "ask about the borrow checker", "borrow"),
            marked("reasoning", "assistant", "b", "the borrow rules again", "borrow"),
        ]);
        let rendered: Vec<String> =
            p.lines(&theme, 80).iter().map(|l| marks_of(l, &theme)).collect();
        let all = rendered.join("\n");
        assert!(all.contains("[borrow]"), "the prose hit still marks as ranked: {rendered:?}");
        assert!(all.contains("<borrow>"), "the reasoning hit still marks: {rendered:?}");
        assert!(
            !all.contains("[borrow] rules"),
            "the reasoning hit must not claim the loud mark: {rendered:?}"
        );
    }

    #[test]
    fn the_two_mark_styles_are_told_apart_by_something_a_monochrome_terminal_renders() {
        // The distinction is worthless if it is carried by colour alone: `NO_COLOR` and
        // `TERM=dumb` get `Theme::plain`, and that is exactly where a reader most needs to know
        // which of the two they are looking at.
        let plain = Theme::plain();
        assert_ne!(plain.match_hl.add_modifier, plain.match_unranked.add_modifier);
        assert!(!plain.match_unranked.add_modifier.is_empty(), "it has to render as something");
        assert!(
            !plain.match_hl.add_modifier.contains(plain.match_unranked.add_modifier),
            "neither may be mistaken for the other by a `contains` test"
        );
        assert!(!plain.match_unranked.add_modifier.contains(plain.match_hl.add_modifier));
    }

    #[test]
    fn each_mark_kind_gets_the_style_that_says_what_it_claims() {
        // The mapping only. *Which* kinds mark quietly is core's rule and is tested there
        // against `Kind::is_indexed`; asserting it again here would be the second copy of that
        // rule this move exists to delete.
        let theme = Theme::plain();
        for kind in cs_core::Kind::ALL {
            let b = marked(kind.as_str(), "assistant", "a", "the borrow rules", "borrow");
            let expected = match b.mark_kind() {
                MarkKind::Ranked => theme.match_hl,
                MarkKind::Unranked => theme.match_unranked,
            };
            assert_eq!(mark_style(&b, &theme), expected, "{kind:?}");
        }
    }

    #[test]
    fn a_synthesised_tool_summary_carries_no_marks_it_cannot_justify() {
        // `⚙ Bash ls` is built from the tool name and one argument, not quoted from the
        // message, so `block.marks` index text that is not on the line. Rebasing them onto it
        // would highlight whatever happened to sit at that offset.
        let theme = Theme::plain();
        let b = marked(
            "tool_call",
            "assistant",
            "a",
            "Bash\n{\"command\":\"grep borrow src\"}",
            "borrow",
        );
        let p = preview(vec![b]);
        let rendered = marks_of(&p.lines(&theme, 80)[0], &theme);
        assert!(!rendered.contains('['), "synthesised line was marked: {rendered}");
    }

    /// The gutter each rendered row opens with. The blank separator between messages carries no
    /// spans at all, so it reads as `""` rather than panicking.
    fn gutters(p: &Preview, theme: &Theme) -> Vec<String> {
        p.lines(theme, 80)
            .iter()
            .map(|l| l.spans.first().map(|s| s.content.to_string()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_fenced_block_is_ruled_and_the_prose_around_it_is_not() {
        let theme = Theme::plain();
        let text = "before\n```rust\nfn main() {}\n```\nafter";
        let p = preview(vec![block("prose", "assistant", "a", text)]);
        let rows = gutters(&p, &theme);
        // Row 0 is the speaker line; the five body lines follow. The ``` markers are ruled too,
        // because dropping them would change the row count the click map counts with.
        assert_eq!(&rows[1..6], ["  ", "┃ ", "┃ ", "┃ ", "  "], "{rows:?}");
    }

    #[test]
    fn a_fence_left_open_stops_at_its_own_message() {
        // Half a fence is ordinary in an interrupted or truncated message. `Fence` is built per
        // block, so the rule cannot leak into the next speaker's turn.
        let theme = Theme::plain();
        // Prose is expanded by default at `Density::Full`, which is what this needs.
        let p = preview(vec![
            block("prose", "assistant", "a", "text\n```\nunclosed"),
            block("prose", "user", "b", "a later message"),
        ]);
        let rows = gutters(&p, &theme);
        assert_eq!(rows.iter().filter(|r| *r == "┃ ").count(), 2, "{rows:?}");
        // Block A is speaker + 3 body + blank = 5 rows, so block B's body is row 6.
        assert_eq!(rows[6], "  ", "the rule leaked into the next message: {rows:?}");
    }

    #[test]
    fn a_match_inside_a_fenced_block_is_still_the_loudest_thing_on_the_line() {
        // §8 keeps the highlight applied inside dimmed code. `match_hl` subtracts DIM for this.
        let theme = Theme::plain();
        let b = marked("prose", "assistant", "a", "```\nlet borrow = 1;\n```", "borrow");
        let p = preview(vec![b]);
        let line = &p.lines(&theme, 80)[2];
        let hit = line
            .spans
            .iter()
            .find(|s| s.style.add_modifier.contains(theme.match_hl.add_modifier))
            .expect("the match inside the fence is marked");
        assert_eq!(hit.content, "borrow");
        assert!(!hit.style.add_modifier.contains(ratatui::style::Modifier::DIM));
    }

    #[test]
    fn a_mark_straddling_the_quoted_line_is_dropped_rather_than_clamped() {
        // A clamped mark highlights a fragment no term matched — the same class of lie the
        // `UNLOCATED` label exists to prevent on the results side.
        let marks = [Mark { start: 4, end: 12 }, Mark { start: 6, end: 9 }];
        let kept = rebase(&marks, 5, 10, 2);
        assert_eq!(kept, [Mark { start: 3, end: 6 }], "only the fully-contained mark survives");
        assert!(rebase(&marks, 100, 10, 0).is_empty(), "a line past every mark keeps none");
    }

    #[test]
    fn scrolling_stops_at_the_end_of_the_conversation() {
        // Reported as the preview "freaking out" when scrolled too far. It used to count up to
        // u16::MAX: the pane went blank with nothing to say how far past the end it was, and
        // getting back meant winding the same distance in reverse.
        let theme = Theme::plain();
        let blocks: Vec<Block> =
            (0..20).map(|i| block("prose", "user", &format!("m{i}"), "a line")).collect();
        let mut p = preview(blocks);
        let (w, rows) = (60usize, 10usize);
        let total = p.layout(&theme, w).len();

        for _ in 0..10_000 {
            p.scroll_by(&theme, w, rows, 3);
        }
        assert_eq!(p.scroll as usize, total - rows, "scrolled past the last screenful");
        // And one notch back up is one notch, not a wind-back from 65535.
        p.scroll_by(&theme, w, rows, -3);
        assert_eq!(p.scroll as usize, total - rows - 3);

        for _ in 0..10_000 {
            p.scroll_by(&theme, w, rows, -3);
        }
        assert_eq!(p.scroll, 0, "and stops at the top");
    }

    #[test]
    fn scrolling_reuses_the_rows_and_everything_else_rebuilds_them() {
        // The whole point of the cache: `layout` does not depend on `scroll`, and scrolling is
        // the one thing done at speed. Identity rather than equality, because equal-but-rebuilt
        // is exactly the cost this exists to avoid and would pass an `==` check silently.
        let theme = Theme::plain();
        let mut p = preview(vec![
            block("prose", "user", "a", "one\ntwo"),
            block("prose", "assistant", "b", "three\nfour"),
        ]);
        let first = p.layout(&theme, 60);
        assert!(Rc::ptr_eq(&first, &p.layout(&theme, 60)), "rebuilt an unchanged pane");

        p.scroll = 40;
        assert!(Rc::ptr_eq(&first, &p.layout(&theme, 60)), "scrolling rebuilt the rows");

        // A different width is a different pane, and must not be served these rows.
        assert!(!Rc::ptr_eq(&first, &p.layout(&theme, 30)), "a resize reused stale rows");

        // Focus styles the focused chevron, so it has to rebuild too.
        let before = p.layout(&theme, 60);
        p.move_focus(1);
        assert!(!Rc::ptr_eq(&before, &p.layout(&theme, 60)), "moving focus reused stale rows");

        for edit in 0..3 {
            let before = p.layout(&theme, 60);
            match edit {
                0 => p.toggle_all(),
                1 => p.cycle_density(),
                _ => p.toggle_focused(),
            }
            assert!(!Rc::ptr_eq(&before, &p.layout(&theme, 60)), "edit {edit} reused stale rows");
        }
    }

    #[test]
    fn folding_a_message_forgets_the_cached_height() {
        // The height is memoised so a scroll burst does not lay the pane out per notch. A fold
        // changes the row count, and serving the pre-fold height would let scrolling run past
        // the new end of a conversation that just got much shorter.
        let theme = Theme::plain();
        let (w, rows) = (60usize, 5usize);
        let blocks: Vec<Block> = (0..10)
            .map(|i| block("prose", "user", &format!("m{i}"), "one\ntwo\nthree\nfour"))
            .collect();
        let mut p = preview(blocks);
        for _ in 0..10_000 {
            p.scroll_by(&theme, w, rows, 3);
        }
        let expanded_max = p.scroll;
        p.toggle_all(); // everything collapses to one row each, so the pane gets much shorter
        for _ in 0..10_000 {
            p.scroll_by(&theme, w, rows, 3);
        }
        assert!(p.scroll < expanded_max, "stale height: {} then {}", expanded_max, p.scroll);
        assert_eq!(p.scroll as usize, p.layout(&theme, w).len() - rows);
    }

    #[test]
    fn a_conversation_shorter_than_the_pane_does_not_scroll_at_all() {
        // The clamp has to floor at zero rather than go negative through the saturating cast,
        // or a short conversation would scroll to u16::MAX on the first notch.
        let theme = Theme::plain();
        let mut p = preview(vec![block("prose", "user", "a", "one line")]);
        p.scroll_by(&theme, 60, 40, 3);
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn an_empty_conversation_says_so_rather_than_drawing_nothing() {
        let p = preview(vec![]);
        let lines = p.lines(&Theme::plain(), 40);
        assert_eq!(lines.len(), 1);
    }
}
