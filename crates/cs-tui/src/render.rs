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
    for (source, _, _) in facet_boxes(app) {
        match source {
            None => spans.push(Span::styled(
                " All ",
                if app.source.is_none() { app.theme.selected } else { app.theme.dim },
            )),
            Some(s) => {
                let active = app.source.as_deref() == Some(s.as_str());
                let style = if active { app.theme.selected } else { theme::source_style(&s) };
                let count = app.facets.iter().find(|(f, _)| *f == s).map_or(0, |(_, c)| *c);
                spans.push(Span::raw(" "));
                spans.push(Span::styled(format!(" {} ", theme::source_badge(&s)), style));
                spans.push(Span::styled(format!("· {count}"), app.theme.dim));
            }
        }
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
    let first = first_visible(app, height);
    for (offset, row) in app.rows[first..].iter().take(height).enumerate() {
        let y = body.y + offset as u16;
        let selected = first + offset == app.selected;
        match row {
            Row::Header { group } => header_row(frame, body, y, &cols, app, *group, selected),
            Row::Hit { group, hit } => hit_row(frame, body, y, &cols, app, *group, *hit, selected),
        }
    }
}

/// Index of the first row drawn, given the body height.
///
/// Shared with mouse hit-testing rather than recomputed there: a click that disagrees with
/// the renderer about which row is at the top selects the wrong conversation, and the two
/// drifting apart would be invisible until someone clicked.
pub fn first_visible(app: &App, body_height: usize) -> usize {
    first_visible_from(app.selected, body_height)
}

fn first_visible_from(selected: usize, body_height: usize) -> usize {
    selected.saturating_sub(body_height.saturating_sub(1))
}

/// Where each facet chip sits, as `(source, x, width)`. `None` is the "All" chip.
///
/// Built once and used for both drawing and hit-testing, for the same reason as
/// [`first_visible`].
pub fn facet_boxes(app: &App) -> Vec<(Option<String>, u16, u16)> {
    let mut out = Vec::new();
    let mut x = 0u16;
    let all = " All ";
    out.push((None, x, all.len() as u16));
    x += all.len() as u16;
    for (source, count) in &app.facets {
        let label = format!(" {} ", theme::source_badge(source));
        let tail = format!("· {count}");
        x += 1;
        out.push((Some(source.clone()), x, (label.len() + tail.len()) as u16));
        x += (label.len() + tail.len()) as u16;
    }
    out
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
    // `match_count` comes from the ranking, so it is known without building any snippet —
    // which is what lets the marker be right while `hits` is still empty.
    let marker = if g.match_count == 0 { " " } else if expanded { "▾" } else { "▸" };
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

/// The conversation, folded (§8). Structure comes from `message` rows via
/// [`crate::preview`]; this only lays it out.
fn preview(frame: &mut Frame, area: Rect, app: &App) {
    let title = match app.preview.as_ref().map(|p| p.density) {
        Some(crate::preview::Density::Outline) => " Outline ",
        _ => " Preview ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(app.theme.border)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(g) = app.selected_group() else {
        frame.render_widget(Paragraph::new("  nothing selected").style(app.theme.dim), inner);
        return;
    };
    let w = inner.width as usize;

    let mut lines = vec![
        Line::from(vec![
            Span::styled(theme::source_badge(&g.source), theme::source_style(&g.source)),
            Span::raw("  "),
            Span::styled(
                text::truncate_end(g.title.as_deref().unwrap_or("(untitled)"), w.saturating_sub(14)),
                app.theme.header,
            ),
        ]),
        Line::from(Span::styled(
            text::truncate_end(
                &format!(
                    "{}   {}   {} turns{}",
                    text::display_dir(g.cwd.as_deref().unwrap_or(""), home().as_deref()),
                    // Absolute, and exactly once: the list carries relative ages, which have
                    // no timezone to get wrong, and this is where a date can be checked.
                    g.ended_date.as_deref().unwrap_or("—"),
                    g.user_turns,
                    match app.preview.as_ref() {
                        // Where the focused message sits, so scrolling a long conversation
                        // has somewhere to be relative to.
                        Some(p) if p.drawn_count() > 0 => {
                            let (at, of) = p.position();
                            format!("   msg {at}/{of}")
                        }
                        _ => String::new(),
                    }
                ),
                w,
            ),
            app.theme.dim,
        )),
        Line::raw(""),
    ];

    match app.preview.as_ref() {
        Some(p) => lines.extend(p.lines(&app.theme, w)),
        // Distinguished from an empty conversation: the read failed, and saying "no
        // messages" would blame the data for what the query did (me9.5, one layer down).
        None => lines.push(Line::from(Span::styled(
            "  could not read this conversation's messages",
            app.theme.error,
        ))),
    }

    frame.render_widget(
        Paragraph::new(lines).scroll((app.preview.as_ref().map_or(0, |p| p.scroll), 0)),
        inner,
    );
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
    // Every key here costs a modifier because the search box owns the unmodified keyspace
    // (§8). Advertising them is not optional: an unadvertised modifier key is an unusable one.
    for (k, label) in [
        ("Enter", " open  "),
        ("Tab", " hits  "),
        ("^O", " outline  "),
        ("^E", " fold all  "),
        ("⌥↑↓", " message  "),
        ("⌥⏎", " fold  "),
        ("^P", " pane  "),
        ("Esc", " quit  "),
        ("", "mouse: wheel scrolls the pane under it, click selects"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer and the click handler both derive the top row from this. If they ever
    /// disagreed, a click would select a different conversation than the one under the
    /// pointer — and nothing would look wrong until someone noticed the preview was off.
    #[test]
    fn the_visible_window_ends_at_the_selection() {
        // Selection inside the first screenful: the list has not scrolled.
        assert_eq!(super::first_visible_from(0, 20), 0);
        assert_eq!(super::first_visible_from(19, 20), 0);
        // Past it: the selection sits on the last drawn row.
        assert_eq!(super::first_visible_from(20, 20), 1);
        assert_eq!(super::first_visible_from(57, 20), 38);
        // A pane too short to draw anything must not underflow.
        assert_eq!(super::first_visible_from(3, 0), 3);
    }
}
