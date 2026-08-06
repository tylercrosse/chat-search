//! Draw frames to text instead of to a terminal, so a change to the TUI can be shown rather
//! than described.
//!
//! The Swift app has had `--shot` since `chat-search-me9.8.2` and the TUI has not, which is why
//! every TUI pull request so far has carried a *hand-typed* approximation of the screen. A
//! hand-typed screen is an assertion about the render, made by the person least able to check
//! it — and it goes stale the first time the layout moves under it.
//!
//! What comes out is text, not an image, and that is the point rather than a limitation. A
//! terminal frame is already characters, so the honest capture is the buffer itself: it diffs
//! line by line in a review, it costs a few kilobytes, it needs no display, and it is identical
//! whether it was taken by a person, by a background session, or by a sandboxed Codex worker
//! that has no window server to ask.
//!
//! Colour is deliberately dropped. `Buffer` holds a style per cell and none of it survives here,
//! because the reason to look at two frames side by side is nearly always that something moved,
//! wrapped or truncated. Theme work is the exception, and it is the one thing this cannot show —
//! `--verify-theme` on the Swift side is the instrument for that.

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::{render, state, theme, Opts};

/// One captured frame: a name for the state it was taken in, and the screen as text.
pub struct Frame {
    /// Names the state rather than the file — `scripts/shot.sh` turns it into a path, and a PR
    /// body quotes it as the caption. Kept short and lower-case for both.
    pub name: &'static str,
    pub text: String,
}

/// Every frame worth taking of one query, in the order a reader should meet them.
///
/// The list is short on purpose. The Swift app takes eleven because it has two views, a drawer,
/// three grouping axes and a fold state; the TUI's states that actually *render differently* are
/// these three, and a fourth frame nobody looks at is a fourth frame to keep passing.
pub fn frames(db_path: PathBuf, opts: Opts, width: u16, height: u16) -> anyhow::Result<Vec<Frame>> {
    let reader = cs_core::open_for_read(&db_path)?;
    // `App::new` runs the opening search itself, so the app is answerable the moment it exists.
    let mut app = state::App::new(reader, &opts, theme::Theme::detect(theme::no_color_env()))?;

    let mut out = Vec::new();
    out.push(Frame { name: "rest", text: draw(&app, width, height)? });

    // The preview pane is half the screen, so the list's own column plan is only visible with it
    // shut — and that is the half a row change is usually about.
    app.show_preview = false;
    out.push(Frame { name: "no-preview", text: draw(&app, width, height)? });
    app.show_preview = true;

    // An expanded row draws its matched lines where a collapsed one draws a single snippet, which
    // is a different render rather than a taller one.
    app.toggle_expand();
    out.push(Frame { name: "expanded", text: draw(&app, width, height)? });

    Ok(out)
}

/// Render one frame at a stated size and return it as lines of text.
fn draw(app: &state::App, width: u16, height: u16) -> anyhow::Result<String> {
    let mut term = Terminal::new(TestBackend::new(width, height))?;
    term.draw(|frame| render::draw(frame, app))?;
    Ok(dump(term.backend().buffer()))
}

/// The buffer as text, one line per row.
///
/// Trailing blanks are stripped per line. A frame is padded to the full width with spaces, so
/// keeping them would make every line the same length and every diff carry invisible churn on
/// rows where nothing visible changed.
fn dump(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell((x, y)) {
                line.push_str(cell.symbol());
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dumped_frame_keeps_its_shape_and_loses_its_padding() {
        // The two properties a reviewer reads a frame for: one line per terminal row, and no
        // trailing run of spaces to make an unchanged row look changed.
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 8, 2));
        buffer.set_string(0, 0, "hi", ratatui::style::Style::default());
        let text = super::dump(&buffer);
        assert_eq!(text, "hi\n\n", "padding is stripped, and every row still ends a line");
    }
}
