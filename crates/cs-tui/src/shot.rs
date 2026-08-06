//! Draw frames to a file instead of to a terminal, so a change to the TUI can be shown rather
//! than described.
//!
//! The Swift app has had `--shot` since `chat-search-me9.8.2` and the TUI has not, which is why
//! every TUI pull request so far has carried a *hand-typed* approximation of the screen. A
//! hand-typed screen is an assertion about the render, made by the person least able to check
//! it — and it goes stale the first time the layout moves under it.
//!
//! Three renderings of the same buffer, because no one of them is enough:
//!
//! - [`Frame::text`] is characters and nothing else. It diffs line by line, costs a few
//!   kilobytes, and answers *what moved, wrapped or truncated* — which is most review questions
//!   about a small change, and none of the ones about a large one.
//! - [`Frame::ansi`] is the same buffer with its styles as escape codes. This is the honest
//!   capture: `cat` it and the terminal resolves it exactly as the running app would, because it
//!   is the same bytes the app would have written.
//! - [`Frame::svg`] renders the ANSI against a stated palette so it can go in a pull request
//!   body, where an escape code renders as mojibake.
//!
//! **The SVG is *a* rendering, not *the* rendering, and that is a property of the TUI rather
//! than of this code.** `theme.rs` styles in named ANSI colours — `Color::LightBlue`,
//! `Modifier::DIM` — which resolve against whatever palette the reader's terminal carries. The
//! Swift app has concrete tokens and one true set of pixels; this does not. So the SVG names the
//! palette it used, and a disagreement about a *colour* has to be settled in a terminal against
//! the `.ans`, not in the picture.

use std::fmt::Write as _;

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::{render, state, theme, Opts};

/// The palette the SVG resolves named colours against: xterm's defaults, which is what an
/// unconfigured terminal carries and therefore the least surprising choice. Named in the SVG
/// itself so a reader never has to guess which one they are looking at.
const PALETTE_NAME: &str = "xterm default (16-colour)";
const PALETTE: [(u8, u8, u8); 16] = [
    (0x00, 0x00, 0x00), // black
    (0xcd, 0x00, 0x00), // red
    (0x00, 0xcd, 0x00), // green
    (0xcd, 0xcd, 0x00), // yellow
    (0x00, 0x00, 0xee), // blue
    (0xcd, 0x00, 0xcd), // magenta
    (0x00, 0xcd, 0xcd), // cyan
    (0xe5, 0xe5, 0xe5), // white / gray
    (0x7f, 0x7f, 0x7f), // bright black / dark gray
    (0xff, 0x00, 0x00), // bright red
    (0x00, 0xff, 0x00), // bright green
    (0xff, 0xff, 0x00), // bright yellow
    (0x5c, 0x5c, 0xff), // bright blue
    (0xff, 0x00, 0xff), // bright magenta
    (0x00, 0xff, 0xff), // bright cyan
    (0xff, 0xff, 0xff), // bright white
];

/// What `Color::Reset` becomes. A buffer says "the terminal's default" and cannot say what that
/// is, so the SVG picks one and says so.
const DEFAULT_BG: (u8, u8, u8) = (0x1b, 0x21, 0x23);
const DEFAULT_FG: (u8, u8, u8) = (0xc5, 0xc8, 0xc6);

/// Cell geometry. Runs are drawn with `textLength`, so these hold the grid exactly whatever font
/// the renderer actually resolves — without it, a font half a pixel off per glyph walks a
/// 140-column row several characters out of alignment by its right edge.
const CELL_W: f32 = 8.4;
const CELL_H: f32 = 18.0;
const BASELINE: f32 = 13.5;

/// One captured frame, in each of the three renderings.
pub struct Frame {
    /// Names the state rather than the file — `scripts/shot.sh` turns it into a path, and a PR
    /// body quotes it as the caption. Kept short and lower-case for both.
    pub name: &'static str,
    pub text: String,
    pub ansi: String,
    pub svg: String,
}

/// Every frame worth taking of one query, in the order a reader should meet them.
///
/// The list is short on purpose. The Swift app takes eleven because it has two views, a drawer,
/// three grouping axes and a fold state; the TUI's states that actually *render differently* are
/// these four, and a fifth frame nobody looks at is a fifth frame to keep passing.
pub fn frames(db_path: std::path::PathBuf, opts: Opts, width: u16, height: u16) -> anyhow::Result<Vec<Frame>> {
    let reader = cs_core::open_for_read(&db_path)?;
    // Styled deliberately, even when the calling shell has NO_COLOR set: a shot taken with the
    // styles stripped would record the plain theme as though it were the themed one, and the
    // `.ans` and the SVG would both be quietly wrong.
    let mut app = state::App::new(reader, &opts, theme::Theme::indexed())?;

    let mut out = Vec::new();
    out.push(capture("rest", &app, width, height)?);

    // The preview pane is half the screen, so the list's own column plan is only visible with it
    // shut — and that is the half a row change is usually about.
    app.show_preview = false;
    out.push(capture("no-preview", &app, width, height)?);
    app.show_preview = true;

    // An expanded row draws its matched lines where a collapsed one draws a single snippet, which
    // is a different render rather than a taller one.
    app.toggle_expand();
    out.push(capture("expanded", &app, width, height)?);

    // Mid-word, which since `j1n` is a state of its own: a search defers reading the conversation
    // that ranked first until the keyboard goes quiet, so between the keystroke and the quarter
    // second after it the pane says what it is doing rather than drawing a conversation nobody
    // asked to see. Nobody can photograph this by pausing a running TUI.
    //
    // Taken by replaying the query's last keystroke on a second `App`, so the frame is a real
    // one rather than a state poked into place — and so it does not also carry the expansion the
    // frame above it left set. A one-character query has no last keystroke to replay and is the
    // recent list either way, so there is nothing here to show.
    let mut partial = opts.clone();
    if let Some((cut, last)) = opts.query.char_indices().next_back() {
        partial.query = opts.query[..cut].to_string();
        let reader = cs_core::open_for_read(&db_path)?;
        let mut mid = state::App::new(reader, &partial, theme::Theme::indexed())?;
        mid.insert_char(last);
        out.push(capture("typing", &mid, width, height)?);
    }

    Ok(out)
}

/// Render one frame at a stated size, in all three forms.
fn capture(name: &'static str, app: &state::App, width: u16, height: u16) -> anyhow::Result<Frame> {
    let mut term = Terminal::new(TestBackend::new(width, height))?;
    term.draw(|frame| render::draw(frame, app))?;
    let buffer = term.backend().buffer();
    Ok(Frame {
        name,
        text: dump(buffer),
        ansi: ansi(buffer),
        svg: svg(buffer),
    })
}

/// The buffer as text, one line per row.
///
/// Trailing blanks are stripped per line. A frame is padded to the full width with spaces, so
/// keeping them would make every line the same length and every diff carry invisible churn on
/// rows where nothing visible changed.
fn dump(buffer: &Buffer) -> String {
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

/// The buffer as the app would have written it: SGR sequences and text.
///
/// Emitted per *run* rather than per cell, and reset at the end of every line. The reset is what
/// makes the file safe to `cat` — a frame that ended mid-style would leave the reader's shell
/// wearing it.
fn ansi(buffer: &Buffer) -> String {
    let area = buffer.area();
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        let mut current: Option<(Option<Color>, Option<Color>, Modifier)> = None;
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell((x, y)) else { continue };
            let key = (cell.fg.into(), cell.bg.into(), cell.modifier);
            if current != Some(key) {
                out.push_str(&sgr(cell.fg, cell.bg, cell.modifier));
                current = Some(key);
            }
            out.push_str(cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// One SGR sequence for a style. Always leads with a reset, so a run never inherits the last
/// one's modifiers — the alternative is tracking which bits to turn *off*, and getting that
/// subtly wrong shows up as a frame that looks bold from the first bold cell onward.
fn sgr(fg: Color, bg: Color, modifier: Modifier) -> String {
    let mut codes = vec!["0".to_string()];
    if modifier.contains(Modifier::BOLD) { codes.push("1".into()); }
    if modifier.contains(Modifier::DIM) { codes.push("2".into()); }
    if modifier.contains(Modifier::ITALIC) { codes.push("3".into()); }
    if modifier.contains(Modifier::UNDERLINED) { codes.push("4".into()); }
    if modifier.contains(Modifier::REVERSED) { codes.push("7".into()); }
    if modifier.contains(Modifier::CROSSED_OUT) { codes.push("9".into()); }
    if let Some(code) = sgr_color(fg, false) { codes.push(code); }
    if let Some(code) = sgr_color(bg, true) { codes.push(code); }
    format!("\x1b[{}m", codes.join(";"))
}

fn sgr_color(color: Color, background: bool) -> Option<String> {
    let base = if background { 10 } else { 0 };
    let n = match color {
        Color::Reset => return None,
        Color::Black => 30, Color::Red => 31, Color::Green => 32, Color::Yellow => 33,
        Color::Blue => 34, Color::Magenta => 35, Color::Cyan => 36, Color::Gray => 37,
        Color::DarkGray => 90, Color::LightRed => 91, Color::LightGreen => 92,
        Color::LightYellow => 93, Color::LightBlue => 94, Color::LightMagenta => 95,
        Color::LightCyan => 96, Color::White => 97,
        Color::Rgb(r, g, b) => return Some(format!("{};2;{r};{g};{b}", 38 + base)),
        Color::Indexed(i) => return Some(format!("{};5;{i}", 38 + base)),
    };
    Some((n + base).to_string())
}

/// The buffer as an SVG, against [`PALETTE`].
///
/// Background rects and text are both emitted per run of identical style, which keeps a 140x40
/// frame to a few tens of kilobytes rather than the ~5,600 elements a per-cell render would take.
fn svg(buffer: &Buffer) -> String {
    let area = buffer.area();
    let (w, h) = (area.width as f32 * CELL_W, area.height as f32 * CELL_H);
    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w:.1} {h:.1}\" width=\"{w:.0}\" height=\"{h:.0}\">\n\
         <!-- {} cols x {} rows, named colours resolved against the {PALETTE_NAME} palette.\n\
              The TUI styles in named ANSI colours, so this is one resolution of the frame and\n\
              not the only one; settle a colour question against the .ans in a terminal. -->\n\
         <rect width=\"100%\" height=\"100%\" fill=\"{}\"/>\n\
         <g font-family=\"ui-monospace, SFMono-Regular, Menlo, Consolas, monospace\" font-size=\"14\">\n",
        area.width, area.height, hex(DEFAULT_BG)
    );

    for y in area.top()..area.bottom() {
        let mut run: Option<(usize, String, Color, Color, Modifier)> = None;
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell((x, y)) else { continue };
            match &mut run {
                Some((_, text, fg, bg, m)) if *fg == cell.fg && *bg == cell.bg && *m == cell.modifier => {
                    text.push_str(cell.symbol());
                }
                _ => {
                    if let Some(r) = run.take() { emit_run(&mut out, r, y - area.top()); }
                    run = Some((
                        (x - area.left()) as usize,
                        cell.symbol().to_string(),
                        cell.fg,
                        cell.bg,
                        cell.modifier,
                    ));
                }
            }
        }
        if let Some(r) = run.take() { emit_run(&mut out, r, y - area.top()); }
    }

    out.push_str("</g>\n</svg>\n");
    out
}

fn emit_run(out: &mut String, run: (usize, String, Color, Color, Modifier), row: u16) {
    let (col, text, fg, bg, modifier) = run;
    let chars = text.chars().count();
    if chars == 0 { return; }

    // REVERSED is applied here rather than left to the renderer, which has no notion of it.
    let (mut fg, mut bg) = (rgb(fg, DEFAULT_FG), rgb(bg, DEFAULT_BG));
    if modifier.contains(Modifier::REVERSED) { std::mem::swap(&mut fg, &mut bg); }

    let x = col as f32 * CELL_W;
    let width = chars as f32 * CELL_W;
    if bg != DEFAULT_BG {
        let _ = write!(
            out, "<rect x=\"{x:.1}\" y=\"{:.1}\" width=\"{width:.1}\" height=\"{CELL_H:.1}\" fill=\"{}\"/>\n",
            row as f32 * CELL_H, hex(bg)
        );
    }
    if text.trim().is_empty() { return; }

    // DIM has no SVG equivalent; opacity is the closest honest analogue, and the theme leans on
    // it heavily (`theme.rs` prefers DIM over DarkGray precisely so it composes with any palette).
    let mut attrs = String::new();
    if modifier.contains(Modifier::BOLD) { attrs.push_str(" font-weight=\"bold\""); }
    if modifier.contains(Modifier::ITALIC) { attrs.push_str(" font-style=\"italic\""); }
    if modifier.contains(Modifier::DIM) { attrs.push_str(" fill-opacity=\"0.55\""); }
    if modifier.contains(Modifier::UNDERLINED) { attrs.push_str(" text-decoration=\"underline\""); }

    let _ = write!(
        out,
        "<text x=\"{x:.1}\" y=\"{:.1}\" fill=\"{}\" textLength=\"{width:.1}\" \
         lengthAdjust=\"spacingAndGlyphs\" xml:space=\"preserve\"{attrs}>{}</text>\n",
        row as f32 * CELL_H + BASELINE,
        hex(fg),
        escape(&text)
    );
}

/// A named or indexed colour against [`PALETTE`]. 216-cube and grayscale entries are computed by
/// xterm's own formulas rather than tabulated.
fn rgb(color: Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        Color::Reset => default,
        Color::Black => PALETTE[0], Color::Red => PALETTE[1], Color::Green => PALETTE[2],
        Color::Yellow => PALETTE[3], Color::Blue => PALETTE[4], Color::Magenta => PALETTE[5],
        Color::Cyan => PALETTE[6], Color::Gray => PALETTE[7], Color::DarkGray => PALETTE[8],
        Color::LightRed => PALETTE[9], Color::LightGreen => PALETTE[10],
        Color::LightYellow => PALETTE[11], Color::LightBlue => PALETTE[12],
        Color::LightMagenta => PALETTE[13], Color::LightCyan => PALETTE[14],
        Color::White => PALETTE[15],
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => match i {
            0..=15 => PALETTE[i as usize],
            16..=231 => {
                let i = i - 16;
                let step = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                (step(i / 36), step((i % 36) / 6), step(i % 6))
            }
            _ => {
                let v = (i - 232) * 10 + 8;
                (v, v, v)
            }
        },
    }
}

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};

    fn cell_buffer() -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 2));
        buffer.set_string(0, 0, "hi", Style::default().fg(Color::Yellow));
        buffer
    }

    #[test]
    fn a_dumped_frame_keeps_its_shape_and_loses_its_padding() {
        // The two properties a reviewer reads a frame for: one line per terminal row, and no
        // trailing run of spaces to make an unchanged row look changed.
        let text = super::dump(&cell_buffer());
        assert_eq!(text, "hi\n\n", "padding is stripped, and every row still ends a line");
    }

    #[test]
    fn every_ansi_line_ends_reset_so_the_file_is_safe_to_cat() {
        // A frame that ended mid-style would leave the shell that printed it wearing the last
        // cell's colour, which is the failure people blame on their terminal rather than on us.
        let ansi = super::ansi(&cell_buffer());
        assert_eq!(ansi.lines().count(), 2);
        for line in ansi.lines() {
            assert!(line.ends_with("\x1b[0m"), "line did not reset: {line:?}");
        }
        assert!(ansi.contains("\x1b[0;33m"), "the yellow run carries its own SGR");
    }

    #[test]
    fn a_run_is_pinned_to_the_grid_whatever_font_resolves() {
        // Without textLength a font half a pixel off per glyph walks a 140-column row several
        // characters out of true by its right edge, and the frame stops being a grid.
        let svg = super::svg(&cell_buffer());
        assert!(svg.contains("textLength=\"16.8\""), "two cells at 8.4 wide");
        assert!(svg.contains("lengthAdjust=\"spacingAndGlyphs\""));
        assert!(svg.contains(super::PALETTE_NAME), "the SVG names the palette it resolved against");
    }

    #[test]
    fn reversed_swaps_the_pair_rather_than_asking_the_renderer_to() {
        // SVG has no notion of reverse video, so a cell that relied on it would render as
        // ordinary text on the default background — invisible when that is the point of it.
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer.set_string(0, 0, "x", Style::default()
            .fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::REVERSED));
        let svg = super::svg(&buffer);
        assert!(svg.contains(&super::hex(super::PALETTE[3])), "yellow became the foreground");
        assert!(svg.contains("<rect"), "and black became a drawn background");
    }
}
