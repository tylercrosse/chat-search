//! The conversation, folded (docs/TUI-DESIGN.md §8).
//!
//! Renders from `message` rows, so `role`, `kind` and `on_head_path` are read rather than
//! guessed back out of prose. fast-resume cannot do this — it flattens messages into one
//! string and re-derives structure by sniffing sigils, which loses the role of every
//! paragraph after the first.
//!
//! Folding is the whole design. Prose is 24% of this corpus; `tool_call` and `tool_result`
//! are 33% each and `reasoning` 8% (measured 2026-07-30 over 168k messages). Rendering all
//! of it verbatim is a file dump with a conversation buried in it.

use std::collections::HashMap;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use rusqlite::Connection;

use crate::text;
use crate::theme::Theme;

/// How much of every message is shown by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// Prose in full, everything else collapsed.
    Full,
    /// One line per message — the whole conversation as a map.
    Outline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    Collapsed,
    Expanded,
}

pub struct Block {
    pub msg_id: String,
    pub role: String,
    pub kind: String,
    pub seq: i64,
    pub on_path: bool,
    pub text: String,
    /// A subagent strand. Worth marking rather than hiding: it is part of what happened, but
    /// it is not the conversation you were having.
    pub is_sidechain: bool,
    pub thread_key: String,
}

impl Block {
    /// Whether this message is drawn at all.
    ///
    /// `tool_result` is omitted rather than collapsed, revised from the original spec after
    /// first use: the result is a blob whose existence the call already implies, so a line
    /// reading `↳ 1.2 KB` spends a row repeating what `⚙ Read(schema.rs)` just said.
    ///
    /// The exception the spec wants — a *failed* result staying legible, because "the tool
    /// broke here" is recognition information — is not implementable yet. Nothing in the
    /// schema marks a result as an error; `kind` has four values and none of them is
    /// `tool_error`. Filed rather than guessed at from the text, because "contains the word
    /// error" would hide real results and show working ones.
    pub fn drawn(&self) -> bool {
        self.kind != "tool_result"
    }
}

pub struct Preview {
    pub conv_id: String,
    blocks: Vec<Block>,
    pub density: Density,
    /// Explicit per-message folds, which always beat the density default.
    overrides: HashMap<String, Fold>,
    /// Index into the drawn blocks.
    pub focus: usize,
    pub scroll: u16,
}

impl Preview {
    /// Read one conversation's messages.
    ///
    /// Head path only. A message edited away is still searchable and still indexed, but
    /// showing it inline without saying so would present an abandoned branch as the
    /// conversation — see the off-path toggle in §8, which is not built yet.
    pub fn load(conn: &Connection, conv_id: &str) -> rusqlite::Result<Self> {
        // `seq` is per *thread*, not per conversation — ADR 4 makes a conversation a DAG and
        // `thread_key` carries which strand a message belongs to. Ordering by `seq` alone
        // therefore interleaves every thread at once: on a real 9-thread conversation it put
        // all nine opening user turns first, then all nine first replies, which reads as
        // nonsense. Main thread before sidechains, each strand contiguous. `idx_message_thread`
        // is (conv_id, thread_key, seq), which exists for exactly this.
        let mut stmt = conn.prepare_cached(
            "SELECT id, role, kind, seq, on_head_path, text, is_sidechain, thread_key
             FROM message WHERE conv_id = ?1 AND on_head_path = 1
             ORDER BY is_sidechain, thread_key, seq",
        )?;
        let blocks = stmt
            .query_map(rusqlite::params![conv_id], |r| {
                Ok(Block {
                    msg_id: r.get(0)?,
                    role: r.get(1)?,
                    kind: r.get(2)?,
                    seq: r.get(3)?,
                    on_path: r.get::<_, i64>(4)? != 0,
                    text: r.get(5)?,
                    is_sidechain: r.get::<_, i64>(6)? != 0,
                    thread_key: r.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Self {
            conv_id: conv_id.to_string(),
            blocks,
            density: Density::Full,
            overrides: HashMap::new(),
            focus: 0,
            scroll: 0,
        })
    }

    pub fn drawn(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(|b| b.drawn())
    }

    pub fn drawn_count(&self) -> usize {
        self.blocks.iter().filter(|b| b.drawn()).count()
    }

    /// Resolve a block's fold: an explicit override, else the default for its kind under the
    /// current density.
    pub fn fold_of(&self, block: &Block) -> Fold {
        if let Some(explicit) = self.overrides.get(&block.msg_id) {
            return *explicit;
        }
        match (self.density, block.kind.as_str()) {
            (Density::Outline, _) => Fold::Collapsed,
            (Density::Full, "prose") => Fold::Expanded,
            (Density::Full, _) => Fold::Collapsed,
        }
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
    }

    pub fn toggle_focused(&mut self) {
        let Some(block) = self.drawn().nth(self.focus) else { return };
        let next = match self.fold_of(block) {
            Fold::Collapsed => Fold::Expanded,
            Fold::Expanded => Fold::Collapsed,
        };
        let id = block.msg_id.clone();
        self.overrides.insert(id, next);
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
    /// pane still shows 1 to 20. Rendered height varies per block, so the offset is counted
    /// from the same line generation the renderer uses rather than assumed to be one row per
    /// message, which only holds in outline mode.
    pub fn follow_focus(&mut self, viewport_rows: usize) {
        let mut before = 0usize;
        for (i, block) in self.drawn().enumerate() {
            if i >= self.focus {
                break;
            }
            before += self.rendered_rows(block);
        }
        let focused_rows = self.drawn().nth(self.focus).map_or(1, |b| self.rendered_rows(b));
        let bottom = before + focused_rows;
        if bottom > (self.scroll as usize) + viewport_rows {
            self.scroll = bottom.saturating_sub(viewport_rows) as u16;
        } else if before < self.scroll as usize {
            self.scroll = before as u16;
        }
    }

    /// How many terminal rows a block occupies once folded.
    fn rendered_rows(&self, block: &Block) -> usize {
        match self.fold_of(block) {
            Fold::Collapsed => 1,
            // Speaker line, the body, and the blank separator.
            Fold::Expanded => 2 + block.text.lines().count(),
        }
    }

    /// Where the focused message sits in the conversation, for the header.
    pub fn position(&self) -> (usize, usize) {
        (self.focus + 1, self.drawn_count())
    }

    /// Render to lines. `terms` are the query's, for highlighting once
    /// `chat-search-6eb.20` lands a matcher that agrees with the ranker — a substring scan
    /// would mark nothing on a stemmed hit, which is worse than marking nothing at all
    /// because it looks deliberate.
    pub fn lines(&self, theme: &Theme, width: usize) -> Vec<Line<'static>> {
        let mut out = Vec::new();
        for (i, block) in self.drawn().enumerate() {
            let focused = i == self.focus;
            let fold = self.fold_of(block);
            let marker = if focused { "▸" } else { " " };
            match fold {
                Fold::Collapsed => out.push(Line::from(vec![
                    Span::styled(format!("{marker} "), theme.dim),
                    collapsed_span(block, theme, width.saturating_sub(2)),
                ])),
                Fold::Expanded => {
                    out.push(Line::from(vec![
                        Span::styled(format!("{marker} "), theme.dim),
                        Span::styled(speaker(block), speaker_style(block, theme)),
                    ]));
                    for raw in block.text.lines() {
                        out.push(Line::from(Span::styled(
                            format!("  {}", text::truncate_end(raw, width.saturating_sub(2))),
                            Style::new(),
                        )));
                    }
                    out.push(Line::raw(""));
                }
            }
        }
        if out.is_empty() {
            out.push(Line::from(Span::styled("  no messages on the head path", theme.dim)));
        }
        out
    }
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

/// Distinct strands in this conversation, for the header.
pub fn thread_count(blocks: &[Block]) -> usize {
    let mut keys: Vec<&str> = blocks.iter().map(|b| b.thread_key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    keys.len()
}

fn speaker_style(block: &Block, theme: &Theme) -> Style {
    if block.role == "user" { theme.accent } else { theme.header }
}

/// One line standing in for a whole message.
fn collapsed_span(block: &Block, theme: &Theme, width: usize) -> Span<'static> {
    let (body, style) = match block.kind.as_str() {
        // Reasoning opens with its own bolded summary — `**Planning README creation**` — so
        // the model has already written the one-line version. Use it rather than counting
        // lines at the user.
        "reasoning" => (format!("⋯ {}", first_line(&block.text).trim_matches('*')), theme.dim),
        "tool_call" => (format!("⚙ {}", tool_summary(&block.text)), theme.dim),
        _ => (format!("{} {}", speaker(block), first_line(&block.text)), Style::new()),
    };
    Span::styled(text::truncate_end(&body, width), style)
}

fn first_line(text: &str) -> String {
    text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string()
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
        }
    }

    fn preview(blocks: Vec<Block>) -> Preview {
        Preview {
            conv_id: "c".into(),
            blocks,
            density: Density::Full,
            overrides: HashMap::new(),
            focus: 0,
            scroll: 0,
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
        let mut b = |thread: &str, side: bool, seq: i64, id: &str| Block {
            msg_id: id.into(),
            role: "user".into(),
            kind: "prose".into(),
            seq,
            on_path: true,
            text: "x".into(),
            is_sidechain: side,
            thread_key: thread.into(),
        };
        let blocks = vec![
            b("main", false, 0, "m0"),
            b("main", false, 1, "m1"),
            b("sub-a", true, 0, "a0"),
            b("sub-a", true, 1, "a1"),
        ];
        assert_eq!(thread_count(&blocks), 2);
        let p = preview(blocks);
        let order: Vec<&str> = p.drawn().map(|x| x.msg_id.as_str()).collect();
        assert_eq!(order, ["m0", "m1", "a0", "a1"]);
        assert!(!p.drawn().next().unwrap().is_sidechain, "the main strand reads first");
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
        let mut p = preview(blocks);
        p.move_focus(39);
        p.follow_focus(10);
        assert!(p.scroll > 0, "the viewport followed the cursor down");
        p.move_focus(-39);
        p.follow_focus(10);
        assert_eq!(p.scroll, 0, "and back to the top");
    }

    #[test]
    fn an_empty_conversation_says_so_rather_than_drawing_nothing() {
        let p = preview(vec![]);
        let lines = p.lines(&Theme::plain(), 40);
        assert_eq!(lines.len(), 1);
    }
}
