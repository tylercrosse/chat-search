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
use ratatui::crossterm::event::{self, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers};
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
    /// Restricts to one source id, as `--source` does on the CLI. Desugared into query state
    /// at startup so there is exactly one source of truth for filters (§5).
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
        resume_cmd: Option<String>,
        cwd: Option<String>,
    },
}

pub fn run(db_path: PathBuf, log: LogSink<'_>, opts: Opts) -> anyhow::Result<Exit> {
    let conn = rusqlite::Connection::open(&db_path)
        .with_context(|| format!("opening {} (run `cs index` first?)", db_path.display()))?;
    // Checked before the terminal is taken over: an error printed into the alternate screen
    // is an error nobody reads.
    cs_core::ensure_current(&conn).map_err(anyhow::Error::msg)?;
    let mut app = state::App::new(conn, &opts, theme::Theme::detect(theme::no_color_env()))?;

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

fn event_loop(term: &mut Term, app: &mut state::App) -> anyhow::Result<Exit> {
    loop {
        term.draw(|f| render::draw(f, app))?;

        // Blocking read: with no background work there is nothing to poll for, and a timeout
        // would just wake the process to redraw an unchanged screen.
        let CEvent::Key(key) = event::read()? else { continue };
        // Windows reports press and release; acting on both double-types every character.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Alt is the preview's modifier and Ctrl the application's, so the two cursors —
        // the result list and the message inside it — never fight over a key.
        if key.modifiers.contains(KeyModifiers::ALT) {
            if let Some(p) = app.preview.as_mut() {
                match key.code {
                    KeyCode::Up => p.move_focus(-1),
                    KeyCode::Down => p.move_focus(1),
                    KeyCode::Enter => p.toggle_focused(),
                    _ => {}
                }
            }
            continue;
        }
        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => return Ok(Exit::Quit),
            (KeyCode::Enter, _) => {
                let Some(g) = app.selected_group() else { continue };
                return Ok(Exit::Open {
                    conv_id: g.conv_id.clone(),
                    resume_cmd: g.resume_cmd.clone(),
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
}

/// Drawn on **stderr**, deliberately.
///
/// Stdout is the return channel — `cs tui` prints the resume command there so a shell
/// wrapper can `eval "$(cs tui)"`. If the UI shared it, capturing that command would
/// swallow the entire interface and the user would stare at a blank terminal. fzf solves
/// the same problem by opening /dev/tty; stderr costs nothing extra and keeps working when
/// there is no controlling terminal to open.
type Term = Terminal<CrosstermBackend<std::io::Stderr>>;

/// Raw mode and the alternate screen, restored on drop however the loop ends.
struct Screen(Option<Term>);

impl Screen {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut out = std::io::stderr();
        execute!(out, EnterAlternateScreen)?;
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
        let _ = execute!(std::io::stderr(), LeaveAlternateScreen);
        if let Some(mut t) = self.0.take() {
            let _ = t.show_cursor();
        }
    }
}
