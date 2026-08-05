//! The native search surface.
//!
//! Depends on `cs-core` and nothing else in this workspace (docs/TUI-DESIGN.md §1). It is
//! handed the index path and a query-log sink rather than resolving either, so it holds no
//! filesystem policy: `cs` knows where the archive is, this does not, and a test can capture
//! log events without touching disk.

use std::path::PathBuf;

use anyhow::Context;
use cs_core::querylog::Event;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEventKind,
    KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::Terminal;

pub mod layout;
pub mod preview;
pub mod render;
pub mod rows;
pub mod state;
pub mod text;
pub mod theme;

/// Where a sink for [`Event`] comes from. `cs` passes one that appends to `queries.jsonl`;
/// tests pass one that pushes to a `Vec`.
///
/// Failure is the caller's problem and is deliberately not surfaced here — a search that
/// cannot write its log line should still return results (`cs_core::querylog::append`).
pub type LogSink<'a> = &'a mut dyn FnMut(Event);

#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// Prefills the search box. Empty means the blank-query recent fallback.
    pub query: String,
    /// Restricts to one source id, as `--source` does on the CLI. Desugared into the query
    /// *text* at startup — `Query::with_source` — so the flag is visible in the search box and
    /// there is exactly one source of truth for filters (§5). Read once by `App::new`; nothing
    /// in this crate keeps it.
    pub source: Option<String>,
    /// Conversations per search. The header reports this against the corpus total.
    pub limit: i64,
}

/// How the TUI came back. `cs` turns [`Exit::Open`] into the actual open action; the TUI
/// never spawns anything itself.
#[derive(Debug, Clone, PartialEq)]
pub enum Exit {
    Quit,
    Open {
        conv_id: String,
        /// Every way this conversation can be reopened, best first — resolved here rather than
        /// re-derived by `cs`, because this is where §6's picker will choose between them.
        /// Empty means the source offers no reopen path, which the caller must report rather
        /// than treat as a failure.
        destinations: Vec<cs_core::Destination>,
        cwd: Option<String>,
    },
}

pub fn run(db_path: PathBuf, log: LogSink<'_>, opts: Opts) -> anyhow::Result<Exit> {
    // Checked before the terminal is taken over: an error printed into the alternate screen
    // is an error nobody reads. Missing, half-built and stale are separate answers here
    // (`cs_core::IndexState`), which is what stops a first run being told its brand-new index
    // predates the schema.
    let reader = cs_core::open_for_read(&db_path)?;
    let mut app = state::App::new(reader, &opts, theme::Theme::detect(theme::no_color_env()))?;

    // The guard owns the restore, so a `?` out of the loop below leaves a usable terminal
    // rather than a raw-mode shell with no echo.
    let mut term = Screen::enter()?;
    let exit = event_loop(term.terminal(), &mut app);

    // Restored before any event is written: the sink can touch the filesystem, and a slow or
    // failing write should not happen behind the alternate screen.
    drop(term);

    match &exit {
        Ok(Exit::Open { .. }) => {
            if let Some(event) = app.pick_event() {
                log(event);
            }
        }
        // Quitting without opening anything is the abandonment signal — the ranking showed
        // nothing worth opening, which `6eb.21` cannot learn any other way.
        Ok(Exit::Quit) => {
            if let Some(event) = app.abandon_event() {
                log(event);
            }
        }
        Err(_) => {}
    }
    exit
}

/// Queued events one frame will absorb before redrawing.
///
/// This is a smoothness budget, not a throughput one. Every absorbed scroll report is motion the
/// user never sees, because only the frame at the end of the burst is drawn — so the ceiling has
/// to be low enough that a flick still reads as movement rather than as a jump.
///
/// It was 512 when a frame cost 43 ms and coalescing was damage control: redrawing per event left
/// the pane crawling seconds behind the finger. Caching the layout took a frame to 3.5 ms (p50,
/// measured on the corpus's longest conversation at 140x40), which removed the reason for a
/// ceiling that high and left only its cost — 512 notches is 1,536 rows of motion in one frame.
///
/// Four is calibrated from that frame time rather than picked: at 3.5 ms the loop can absorb
/// ~1,100 events a second, several times faster than a trackpad delivers them, so the backlog
/// this exists to prevent still cannot build — while motion stays capped at 12 rows a frame.
const BURST: usize = 4;

/// Consecutive unreadable events tolerated before the loop gives up.
///
/// A garbled report in the middle of a fast scroll is a recoverable glitch, and it used to end
/// the session: `event::read()?` sent the error straight out of the loop, so a transient parse
/// or I/O failure threw away the user's query and left the terminal to the `Screen` guard
/// mid-gesture. Skipping one is right; a stream that is *persistently* unreadable is a real
/// failure and still stops.
const MAX_MISREADS: u32 = 64;

/// How still the keyboard has to go before the header's match total is worth establishing.
///
/// The total is free for any query narrow enough to finish typing, so this timer only ever
/// governs broad prefixes — where settling costs 5–36 ms against this corpus
/// (`cs_core::search::count_cost`). Paying that per keystroke put the spend at the one moment
/// the number buys nothing: `tes` is three letters into a word, not a result set anyone is
/// reading the size of.
///
/// 250 ms is measured against typing rather than picked for feel. A 100 wpm typist leaves
/// ~120 ms between keystrokes, so this clears a fast burst without firing inside it — which
/// matters, because a count started one keystroke early is not merely wasted, it is 36 ms of
/// latency handed to the keystroke that interrupts it. Erring long costs only how soon a
/// number appears after you stop, and a quarter second after stopping still reads as "already
/// there".
const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

fn event_loop(term: &mut Term, app: &mut state::App) -> anyhow::Result<Exit> {
    let mut misreads = 0u32;
    loop {
        term.draw(|f| render::draw(f, app))?;

        // Geometry for this frame, so a click and the renderer agree about what is where.
        let area = term.size().ok().map(|s| ratatui::layout::Rect::new(0, 0, s.width, s.height));
        let panes = area.map(|a| layout::app(a, app.show_preview));

        // The one thing worth waking up for. With a total still unsettled, wait `SETTLE` for
        // the next keystroke instead of indefinitely: if it does not come, the user has
        // stopped typing and the number they are about to read is worth the query.
        //
        // Deliberately gated on there being something to do. An unconditional timeout would
        // wake the process forever to redraw an unchanged screen, and a settle that fails
        // returns false so this falls through to the blocking read below rather than retrying
        // at 4 Hz against an index that is not going to answer.
        if app.unsettled() && !event::poll(SETTLE).unwrap_or(false) && app.settle() {
            continue;
        }

        // The first read blocks — with nothing pending there is nothing to poll for, and a
        // timeout would just wake the process to redraw an unchanged screen. Everything already
        // queued behind it is then taken without blocking and folded into the same frame.
        for absorbed in 0..BURST {
            match event::read() {
                Ok(ev) => {
                    misreads = 0;
                    if let Some(exit) = handle(app, ev, area, panes) {
                        return Ok(exit);
                    }
                }
                Err(e) => {
                    misreads += 1;
                    if misreads >= MAX_MISREADS {
                        return Err(e).context("reading terminal input");
                    }
                }
            }
            let _ = absorbed;
            if !event::poll(std::time::Duration::ZERO).unwrap_or(false) {
                break;
            }
        }
    }
}

/// Act on one event. `Some` ends the session.
fn handle(
    app: &mut state::App,
    ev: CEvent,
    area: Option<ratatui::layout::Rect>,
    panes: Option<layout::AppLayout>,
) -> Option<Exit> {
    {
        if let CEvent::Mouse(m) = ev {
            let (Some(area), Some(panes)) = (area, panes) else { return None };
            let target = layout::scroll_target(area, app.show_preview, m.column, m.row);
            // Three rows a notch is the usual terminal convention and keeps a wheel flick from
            // overshooting. Routed by where the pointer is, not by focus.
            let scroll = match m.kind {
                MouseEventKind::ScrollDown => Some(3isize),
                MouseEventKind::ScrollUp => Some(-3),
                _ => None,
            };
            match (scroll, target) {
                (Some(delta), Some(layout::ScrollTarget::Preview)) => {
                    // The pane's own geometry, because where the scroll stops depends on how
                    // tall the conversation renders at this width.
                    let pane = panes.main.preview();
                    let width = pane.map_or(40, |r| render::preview_body_width(r.width));
                    let rows = pane.map_or(10, |r| render::preview_body_rows(r.height));
                    app.scroll_preview(delta, width, rows);
                }
                (Some(delta), Some(layout::ScrollTarget::Results)) => app.move_selection(delta),
                _ => {
                    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                        click(app, &panes, m.column, m.row);
                    }
                }
            }
            return None;
        }
        let CEvent::Key(key) = ev else { return None };
        // Windows reports press and release; acting on both double-types every character.
        if key.kind != KeyEventKind::Press {
            return None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Alt is the preview's modifier and Ctrl the application's, so the two cursors —
        // the result list and the message inside it — never fight over a key.
        if key.modifiers.contains(KeyModifiers::ALT) {
            let pane = panes.and_then(|p| p.main.preview());
            let rows = pane.map_or(10, |r| render::preview_body_rows(r.height));
            let width = pane.map_or(40, |r| render::preview_body_width(r.width));
            if let Some(p) = app.preview.as_mut() {
                match key.code {
                    KeyCode::Up => p.move_focus(-1),
                    KeyCode::Down => p.move_focus(1),
                    KeyCode::Enter => p.toggle_focused(),
                    _ => {}
                }
            }
            app.follow_preview_focus(width, rows);
            return None;
        }
        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => return Some(Exit::Quit),
            (KeyCode::Enter, _) => {
                let g = app.selected_group()?;
                return Some(Exit::Open {
                    conv_id: g.conv_id.clone(),
                    destinations: g.destinations.clone(),
                    cwd: g.cwd.clone(),
                });
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), true) => app.move_selection(-1),
            (KeyCode::Down, _) | (KeyCode::Char('j'), true) => app.move_selection(1),
            (KeyCode::PageUp, _) => app.move_selection(-10),
            (KeyCode::PageDown, _) => app.move_selection(10),
            (KeyCode::Tab, _) => app.toggle_expand(),
            (KeyCode::Char('p'), true) => app.show_preview = !app.show_preview,
            (KeyCode::Char('o'), true) => {
                if let Some(p) = app.preview.as_mut() {
                    p.cycle_density();
                }
            }
            (KeyCode::Char('e'), true) => {
                if let Some(p) = app.preview.as_mut() {
                    p.toggle_all();
                }
            }
            (KeyCode::Backspace, _) => app.backspace(),
            (KeyCode::Delete, _) => app.delete(),
            (KeyCode::Left, _) => app.cursor = app.cursor.saturating_sub(1),
            (KeyCode::Right, _) => app.cursor = (app.cursor + 1).min(app.query.chars().count()),
            (KeyCode::Home, _) => app.cursor = 0,
            (KeyCode::End, _) => app.cursor = app.query.chars().count(),
            // Every unmodified key belongs to the search box, which is why each command above
            // costs a modifier or a named key (docs/TUI-DESIGN.md §8).
            (KeyCode::Char(ch), false) => app.insert_char(ch),
            _ => {}
        }
    }
    None
}

/// Drawn on **stderr**, deliberately.
///
/// Stdout is the return channel — `cs tui` prints the resume command there so a shell
/// wrapper can `eval "$(cs tui)"`. If the UI shared it, capturing that command would
/// swallow the entire interface and the user would stare at a blank terminal. fzf solves
/// the same problem by opening /dev/tty; stderr costs nothing extra and keeps working when
/// there is no controlling terminal to open.
/// Act on a left click, by where it landed.
fn click(app: &mut state::App, panes: &layout::AppLayout, col: u16, row: u16) {
    // Facet bar: add a source to the selection, or take it back out by clicking it again. The
    // click lands in the search box as `agent:` text, so the filter is visible where it was
    // applied and a second chip widens rather than replaces (§5).
    if row == panes.filters.y {
        for (source, x, w) in render::facet_boxes(app) {
            if col >= panes.filters.x + x && col < panes.filters.x + x + w {
                app.toggle_source(source);
                return;
            }
        }
        return;
    }

    // Preview before results, because with the preview stacked underneath (below
    // `SPLIT_MIN_WIDTH`) a row can satisfy the results pane's `row >= body_top` test while
    // sitting in the preview.
    if let Some(pane) = panes.main.preview() {
        if pane.contains(ratatui::layout::Position::new(col, row)) {
            preview_click(app, pane, row);
            return;
        }
    }

    let results = panes.main.results();
    // +1 for the border, +1 for the column headings.
    let body_top = results.y + 2;
    let body_height = results.height.saturating_sub(3) as usize;
    if col >= results.x && col < results.right() && row >= body_top {
        let offset = (row - body_top) as usize;
        let index = render::first_visible(app, body_height) + offset;
        if index < app.rows.len() {
            app.select(index);
        }
    }
}

/// Focus the message under the pointer, and fold it if the pointer is on its header line.
///
/// Clicking the header — the collapsed one-liner, or the speaker line of an expanded block —
/// toggles, so a collapsed message opens where you clicked it and its speaker line closes it
/// again. Clicking body text only moves focus: a block that collapsed out from under the
/// pointer would take the text being read with it.
fn preview_click(app: &mut state::App, pane: ratatui::layout::Rect, row: u16) {
    let rows = render::preview_body_rows(pane.height);
    // The same width the renderer laid the text out at. Resolve a click against any other and
    // the lines wrap differently, so the row counted is not the row clicked.
    let width = render::preview_body_width(pane.width);
    let theme = app.theme;
    let Some(p) = app.preview.as_mut() else { return };
    // The border and the header lines are above the body; the pane is scrolled under them.
    let Some(offset) = row.checked_sub(pane.y + 1 + render::PREVIEW_HEADER_LINES) else { return };
    let at = offset as usize + p.scroll as usize;
    let Some((block, on_header)) = p.block_at_row(&theme, width, at) else { return };
    if on_header {
        p.toggle_at(block);
    } else {
        p.focus = block;
    }
    // A fold changes what is above the focused message, so the viewport has to be re-checked
    // against the new heights rather than left where the click found it.
    p.follow_focus(&theme, width, rows);
}

type Term = Terminal<CrosstermBackend<std::io::Stderr>>;

/// Raw mode and the alternate screen, restored on drop however the loop ends.
struct Screen(Option<Term>);

impl Screen {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut out = std::io::stderr();
        // Mouse capture is a real trade: the terminal stops handling drag-to-select, so
        // copying text out of the pane needs Shift held. Worth it because a mouse event
        // carries a position, which is what lets a wheel scroll whichever pane it is over
        // without the app needing any notion of focus at all.
        execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Screen(Some(Terminal::new(CrosstermBackend::new(out))?)))
    }

    fn terminal(&mut self) -> &mut Term {
        self.0.as_mut().expect("terminal is present until drop")
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // Best-effort, and deliberately silent: this runs while unwinding from whatever went
        // wrong, and a restore error would replace the real one.
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stderr(), DisableMouseCapture, LeaveAlternateScreen);
        if let Some(mut t) = self.0.take() {
            let _ = t.show_cursor();
        }
    }
}
