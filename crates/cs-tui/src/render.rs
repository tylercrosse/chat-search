//! Drawing one frame.
//!
//! Reads `App` and writes to the frame; holds no state of its own. Every width decision comes
//! from [`crate::layout`] and every string that has to fit a cell goes through
//! [`crate::text`], so a cell can never overflow into its neighbour.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::layout::{self, Col, Columns};
use crate::rows::Row;
use crate::state::{self, App};
use crate::text;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let l = layout::app(frame.area(), app.show_preview);
    header(frame, l.header, app);
    search(frame, l.search, app);
    filters(frame, l.filters, app);
    results(frame, l.main.results(), app);
    if let Some(area) = l.main.preview() {
        preview(frame, area, app);
    }
    footer(frame, l.footer, app);
}

/// Corpus scale, result-set scale and latency, so a disappointing result set can be told
/// apart from a coverage problem without leaving the screen.
fn header(frame: &mut Frame, area: Rect, app: &App) {
    let left = Span::styled("cs", app.theme.accent);
    let right = format!(
        "{} shown / {} indexed   {:.1}ms",
        app.groups.len(),
        app.indexed,
        app.last_ms
    );
    let pad = (area.width as usize)
        .saturating_sub(2)
        .saturating_sub(text::width(&right));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            left,
            Span::raw(" ".repeat(pad)),
            Span::styled(right, app.theme.dim),
        ])),
        area,
    );
}

fn search(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(app.theme.accent)
        .title(" Search ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut spans = vec![Span::styled(" / ", app.theme.accent)];
    if app.query.is_empty() {
        spans.push(Span::styled(
            "type to search — blank lists recent conversations",
            app.theme.dim,
        ));
    } else {
        spans.push(Span::raw(app.query.clone()));
    }
    if app.holding() {
        // Ghost text rather than a status line: the explanation belongs beside the thing it
        // explains, and the list below is recent conversations, not partial matches.
        spans.push(Span::styled(
            format!(
                "   {} more character{} to search",
                state::MIN_QUERY_CHARS - app.query.trim().chars().count(),
                if state::MIN_QUERY_CHARS - app.query.trim().chars().count() == 1 { "" } else { "s" }
            ),
            app.theme.dim,
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);

    // The caret is the terminal's, not a drawn glyph, so it blinks like every other input.
    let caret = inner.x + 3 + text::width(&sub(&app.query, app.cursor)) as u16;
    if caret < inner.right() {
        frame.set_cursor_position((caret, inner.y));
    }
}

/// Source facets, doubling as a corpus census.
///
/// Index-derived for now, so a configured source holding zero rows is still invisible here —
/// that is `me9.14`, and it needs config the TUI deliberately does not have (§1).
fn filters(frame: &mut Frame, area: Rect, app: &App) {
    let mut spans = Vec::new();
    let all_active = app.source.is_none();
    spans.push(Span::styled(
        " All ",
        if all_active { app.theme.selected } else { app.theme.dim },
    ));
    for (source, count) in &app.facets {
        let active = app.source.as_deref() == Some(source.as_str());
        let style = if active {
            app.theme.selected
        } else {
            theme::source_style(source)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!(" {} ", theme::source_badge(source)), style));
        spans.push(Span::styled(format!("· {count}"), app.theme.dim));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn results(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(app.theme.border)
        // Named for what the list *is*: under a blank or still-too-short query these are the
        // most recent conversations, and calling them results would imply they matched.
        .title(if app.is_blank() || app.holding() { " Recent " } else { " Matches " });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let cols = layout::columns(inner.width);
    column_headings(frame, inner, &cols, app);

    let body = Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1));
    if app.rows.is_empty() {
        let msg = if app.status.is_some() {
            // Never claim "no results" when the search itself failed (me9.5).
            "  search failed — showing nothing rather than guessing"
        } else {
            "  no conversations match"
        };
        frame.render_widget(Paragraph::new(msg).style(app.theme.dim), body);
        return;
    }

    let height = body.height as usize;
    let first = app.selected.saturating_sub(height.saturating_sub(1));
    for (offset, row) in app.rows[first..].iter().take(height).enumerate() {
        let y = body.y + offset as u16;
        let selected = first + offset == app.selected;
        match row {
            Row::Header { group } => header_row(frame, body, y, &cols, app, *group, selected),
            Row::Hit { group, hit } => hit_row(frame, body, y, &cols, app, *group, *hit, selected),
        }
    }
}

fn column_headings(frame: &mut Frame, inner: Rect, cols: &Columns, app: &App) {
    let s = app.theme.header;
    cell(frame, inner, cols.agent, 0, "  Source", s);
    cell(frame, inner, cols.title, 0, "Title", s);
    cell(frame, inner, cols.dir, 0, "Directory", s);
    cell(frame, inner, cols.density, 0, "Matches", s);
    cell(frame, inner, cols.msgs, 0, "Msgs", s);
    cell(frame, inner, cols.age, 0, "Age", s);
}

fn header_row(
    frame: &mut Frame,
    body: Rect,
    y: u16,
    cols: &Columns,
    app: &App,
    group: usize,
    selected: bool,
) {
    let Some(g) = app.groups.get(group) else { return };
    let row_style = if selected { app.theme.selected } else { Style::new() };
    fill(frame, body, y, row_style);
    let dy = y - body.y;

    // The pointer, not only the fill: selection must survive a terminal with no colour (§7).
    cell(frame, body, Col { x: 0, w: 2 }, dy, if selected { " ▸" } else { "  " }, row_style);

    let badge = Col { x: cols.agent.x + 2, w: cols.agent.w.saturating_sub(2) };
    cell(frame, body, badge, dy, theme::source_badge(&g.source),
         merge(row_style, theme::source_style(&g.source), selected));

    let title = g.title.as_deref().unwrap_or("(untitled)");
    let expanded = app.expanded.contains(&g.conv_id);
    let marker = if g.hits.is_empty() { " " } else if expanded { "▾" } else { "▸" };
    cell(frame, body, cols.title, dy, &format!("{marker} {title}"), row_style);

    cell(frame, body, cols.dir, dy, &text::display_dir(g.cwd.as_deref().unwrap_or(""), home().as_deref()), merge(row_style, app.theme.dim, selected));
    cell(frame, body, cols.density, dy, &cs_core::search::match_density(&g.match_seqs, g.msg_count), row_style);
    cell(frame, body, cols.msgs, dy, &g.user_turns.to_string(), row_style);
    cell(frame, body, cols.age, dy, &age_of(g.ended_at, app.now), merge(row_style, app.theme.dim, selected));
}

/// A matching message, indented under its conversation. Only the snippet is worth the width —
/// every other column belongs to the conversation, and repeating it reads as a second result.
fn hit_row(
    frame: &mut Frame,
    body: Rect,
    y: u16,
    cols: &Columns,
    app: &App,
    group: usize,
    hit: usize,
    selected: bool,
) {
    let Some(h) = app.groups.get(group).and_then(|g| g.hits.get(hit)) else { return };
    let row_style = if selected { app.theme.selected } else { Style::new() };
    fill(frame, body, y, row_style);
    let dy = y - body.y;

    cell(frame, body, Col { x: 0, w: 2 }, dy, if selected { " ▸" } else { "  " }, row_style);
    let tag = if h.is_sidechain {
        " ↳ [subagent] "
    } else if !h.on_head_path {
        " ↳ [edited away] "
    } else {
        " ↳ "
    };
    let span = Col { x: cols.title.x, w: cols.title.w + cols.dir.w };
    cell(frame, body, span, dy, &format!("{tag}{}", h.snippet), merge(row_style, app.theme.dim, selected));
}

/// Placeholder until `me9.1.1` lands the fold model. Shows what the conversation is and where
/// its matches fell; the block model, tool collapsing and outline mode belong to that bead.
fn preview(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(app.theme.border)
        .title(" Preview ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(g) = app.selected_group() else {
        frame.render_widget(Paragraph::new("  nothing selected").style(app.theme.dim), inner);
        return;
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(theme::source_badge(&g.source), theme::source_style(&g.source)),
            Span::raw("  "),
            Span::styled(g.title.clone().unwrap_or_else(|| "(untitled)".into()), app.theme.header),
        ]),
        Line::from(Span::styled(
            format!(
                "{}   {}   {} turns",
                text::display_dir(g.cwd.as_deref().unwrap_or(""), home().as_deref()),
                // Absolute, and exactly once: the list carries relative ages, which have no
                // timezone to get wrong, and this is where a date can be checked (6eb.8).
                g.ended_date.as_deref().unwrap_or("—"),
                g.user_turns
            ),
            app.theme.dim,
        )),
        Line::raw(""),
    ];
    for h in &g.hits {
        lines.push(Line::from(Span::styled(format!("» {}", h.snippet), Style::new())));
        lines.push(Line::raw(""));
    }
    if g.match_count > g.hits.len() {
        lines.push(Line::from(Span::styled(
            format!("+{} more match(es)", g.match_count - g.hits.len()),
            app.theme.dim,
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    if let Some(status) = &app.status {
        frame.render_widget(
            Paragraph::new(text::truncate_end(status, area.width as usize))
                .style(app.theme.error),
            area,
        );
        return;
    }
    let key = app.theme.selected;
    let mut spans = Vec::new();
    for (k, label) in [
        ("Enter", " open  "),
        ("Tab", " expand  "),
        ("^P", " preview  "),
        ("Esc", " quit"),
    ] {
        spans.push(Span::styled(format!(" {k} "), key));
        spans.push(Span::styled(label, app.theme.dim));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).alignment(Alignment::Left), area);
}

/// Paint the row background first, so a cell that writes no text still carries the selection.
fn fill(frame: &mut Frame, body: Rect, y: u16, style: Style) {
    frame.render_widget(
        Paragraph::new(" ".repeat(body.width as usize)).style(style),
        Rect::new(body.x, y, body.width, 1),
    );
}

/// Write into one column, truncated to fit. A hidden column draws nothing.
fn cell(frame: &mut Frame, area: Rect, col: Col, dy: u16, s: &str, style: Style) {
    if col.is_hidden() || col.x >= area.width || dy >= area.height {
        return;
    }
    let w = col.w.min(area.width - col.x);
    let fitted = text::truncate_end(s, w as usize);
    frame.render_widget(
        Paragraph::new(fitted).style(style),
        Rect::new(area.x + col.x, area.y + dy, w, 1),
    );
}

/// Selection wins over a cell's own colour: the fill has to stay one continuous band, and a
/// dim directory on a selected row would read as a hole in it.
fn merge(row: Style, own: Style, selected: bool) -> Style {
    if selected { row } else { own }
}

fn age_of(ended_at: Option<i64>, now: i64) -> String {
    ended_at.map(|t| text::age(now - t)).unwrap_or_else(|| "—".into())
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

/// The first `n` chars of `s`, for measuring caret offset.
fn sub(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}
