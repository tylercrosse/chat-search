//! Where everything sits (docs/TUI-DESIGN.md §2).
//!
//! Pure geometry: `Rect` in, rectangles out, no state and no rendering. Two independent
//! levels of responsiveness — panes reflow before columns shed, and the preview is never
//! dropped for width, only on the explicit toggle.

use ratatui::layout::{Constraint, Layout, Position, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    pub header: Rect,
    pub search: Rect,
    pub filters: Rect,
    pub main: MainLayout,
    pub footer: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainLayout {
    ResultsOnly { results: Rect },
    Split { results: Rect, preview: Rect },
}

impl MainLayout {
    pub fn results(self) -> Rect {
        match self {
            Self::ResultsOnly { results } | Self::Split { results, .. } => results,
        }
    }

    pub fn preview(self) -> Option<Rect> {
        match self {
            Self::ResultsOnly { .. } => None,
            Self::Split { preview, .. } => Some(preview),
        }
    }
}

/// Five bands, one flexible:
/// `Length(1)` header, `Length(3)` search, `Length(1)` filters, `Min(3)` main, `Length(1)` footer.
pub fn app(area: Rect, show_preview: bool) -> AppLayout {
    // `Min` outranks `Length` in the solver, so a terminal shorter than the nine rows these
    // constraints ask for gives the results band its three rows and collapses the fixed ones,
    // instead of overflowing. Every band stays inside `area` at any size; some arrive empty.
    let [header, search, filters, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area);

    AppLayout { header, search, filters, main: split_main(main, show_preview), footer }
}

/// The pane level of §2's two levels of responsiveness.
///
/// Width changes only the *axis* of the split, never its existence: a user who narrows a
/// window has not asked to lose the preview, and hiding it on width is the naive behaviour
/// §2 rejects. Stacked, the preview is a fixed 12 rows because a preview that shrank with the
/// window would stop repaying the rows it costs, while `Min(8)` keeps enough list to triage.
fn split_main(area: Rect, show_preview: bool) -> MainLayout {
    if !show_preview {
        return MainLayout::ResultsOnly { results: area };
    }
    let [results, preview] = if area.width >= SPLIT_MIN_WIDTH {
        Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area)
    } else {
        Layout::vertical([Constraint::Min(8), Constraint::Length(12)]).areas(area)
    };
    MainLayout::Split { results, preview }
}

/// Split at `width >= SPLIT_MIN_WIDTH`: horizontal 62/38. Below it, reflow to vertical with
/// results `Min(8)` over preview `Length(12)`. `show_preview == false` gives the whole area
/// to results at any width.
pub const SPLIT_MIN_WIDTH: u16 = 116;

/// One column's horizontal extent within the results pane. `w == 0` means hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Col {
    pub x: u16,
    pub w: u16,
}

impl Col {
    pub fn is_hidden(self) -> bool {
        self.w == 0
    }
}

/// Column plan for the results pane, in order: agent, title, dir, density, msgs, age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Columns {
    pub agent: Col,
    pub title: Col,
    pub dir: Col,
    pub density: Col,
    pub msgs: Col,
    pub age: Col,
}

/// Progressive shedding against the results pane's *inner* width, with a floor on Title.
///
/// Msgs sheds before Directory, and the density strip before both: the directory is a
/// stronger retrieval cue for this corpus than either — 6eb.26 measured date and project as
/// the strongest recognition cues, "stronger than anything in the text".
///
/// | inner width | agent | dir | density | msgs | age |
/// | ---: | ---: | ---: | ---: | ---: | ---: |
/// | >= 116 | 13 | 30 | 10 | 6 | 9 |
/// | >= 100 | 13 | 28 | 10 | 0 | 9 |
/// | >= 72 | 12 | 22 | 0 | 0 | 8 |
/// | < 72 | 12 | 0 | 0 | 0 | 7 |
///
/// Title takes the remainder and never falls below [`TITLE_MIN_WIDTH`], even when that
/// overflows a very narrow pane — an unreadable title is worse than a clipped row. One
/// blank column separates each *visible* pair; a hidden column consumes no gap.
///
/// A shed column comes back as `Col::default()`, not as a zero-width slice sitting at the
/// cursor: nothing may be drawn there, and a plausible-looking `x` invites a renderer to use
/// one anyway.
pub fn columns(inner_width: u16) -> Columns {
    // The breakpoints measure the results pane's *inner* width, not the terminal's, so the
    // top row is reachable at ~118 columns with the preview toggled off but needs ~190 while
    // the 62/38 split is up. Written as a literal rather than [`SPLIT_MIN_WIDTH`] on purpose:
    // the two 116s measure different rectangles and are free to move apart.
    let (agent, dir, density, msgs, age) = match inner_width {
        w if w >= 116 => (13, 30, 10, 6, 9),
        w if w >= 100 => (13, 28, 10, 0, 9),
        w if w >= 72 => (12, 22, 0, 0, 8),
        _ => (12, 0, 0, 0, 7),
    };

    // Title is always visible, so the gap count is exactly the number of visible *fixed*
    // columns — one to the title's left or right for each — and a shed column contributes
    // neither width nor separator.
    let fixed = [agent, dir, density, msgs, age];
    let reserved: u16 =
        fixed.iter().sum::<u16>() + fixed.iter().filter(|w| **w > 0).count() as u16;
    let title = inner_width.saturating_sub(reserved).max(TITLE_MIN_WIDTH);

    let mut x = 0u16;
    let mut place = |w: u16| {
        if w == 0 {
            return Col::default();
        }
        let col = Col { x, w };
        // Saturating because the floor is allowed to push the plan past a 20-column pane;
        // the renderer clips, and a wrapped `x` would draw the age column at the left edge.
        x = x.saturating_add(w).saturating_add(GAP);
        col
    };

    Columns {
        agent: place(agent),
        title: place(title),
        dir: place(dir),
        density: place(density),
        msgs: place(msgs),
        age: place(age),
    }
}

/// Blank separator between two visible columns. Rows draw no vertical rules, so this one
/// space is the entire separation between a directory cell that filled its width and the
/// density strip beside it.
const GAP: u16 = 1;

pub const TITLE_MIN_WIDTH: u16 = 16;

/// Which pane a mouse event at `(column, row)` landed in, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    Results,
    Preview,
}

/// Hit-test a mouse position against the two scrollable panes.
///
/// Takes the full terminal `area` and re-derives the panes rather than accepting them: the
/// rects a caller is holding are one resize behind the event it is handling, and geometry
/// this cheap is not worth caching against that. Anything outside both panes — the header,
/// the search box, the footer, the gap past the split — returns `None` so the wheel does
/// nothing rather than scrolling whichever pane happens to be focused.
pub fn scroll_target(
    area: Rect,
    show_preview: bool,
    column: u16,
    row: u16,
) -> Option<ScrollTarget> {
    let main = app(area, show_preview).main;
    let at = Position::new(column, row);
    if main.results().contains(at) {
        Some(ScrollTarget::Results)
    } else if main.preview().is_some_and(|preview| preview.contains(at)) {
        Some(ScrollTarget::Preview)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six columns in layout order. Names exist only so a failure says which pair moved.
    fn ordered(c: Columns) -> [(&'static str, Col); 6] {
        [
            ("agent", c.agent),
            ("title", c.title),
            ("dir", c.dir),
            ("density", c.density),
            ("msgs", c.msgs),
            ("age", c.age),
        ]
    }

    fn visible(c: Columns) -> Vec<(&'static str, Col)> {
        ordered(c).into_iter().filter(|(_, col)| !col.is_hidden()).collect()
    }

    /// One past the last cell any column claims — the number that has to equal the pane width.
    fn right_edge(c: Columns) -> u16 {
        visible(c).last().map_or(0, |(_, col)| col.x + col.w)
    }

    #[test]
    fn the_five_bands_stack_in_order_and_consume_the_whole_area() {
        let area = Rect::new(0, 0, 120, 40);
        let l = app(area, false);
        assert_eq!((l.header.y, l.header.height), (0, 1));
        assert_eq!((l.search.y, l.search.height), (1, 3));
        assert_eq!((l.filters.y, l.filters.height), (4, 1));
        assert_eq!((l.main.results().y, l.main.results().height), (5, 40 - 6));
        assert_eq!((l.footer.y, l.footer.height), (39, 1));
        for band in [l.header, l.search, l.filters, l.main.results(), l.footer] {
            assert_eq!((band.x, band.width), (0, 120));
        }
    }

    #[test]
    fn the_bands_are_placed_relative_to_the_area_not_the_screen() {
        let area = Rect::new(4, 2, 80, 20);
        let l = app(area, false);
        assert_eq!((l.header.x, l.header.y), (4, 2));
        assert_eq!(l.footer.bottom(), area.bottom());
        assert_eq!(l.main.results().x, 4);
    }

    #[test]
    fn at_exactly_the_split_width_the_panes_sit_side_by_side() {
        let area = Rect::new(0, 0, SPLIT_MIN_WIDTH, 40);
        let MainLayout::Split { results, preview } = app(area, true).main else {
            panic!("the preview was requested and the width is at the threshold");
        };
        assert_eq!(results.y, preview.y);
        assert_eq!(results.height, preview.height);
        assert_eq!(preview.x, results.right(), "the panes must abut — no dead column between");
        assert_eq!(results.width + preview.width, area.width);
        // 62% of 116 is 71.9; the solver has to round it one way or the other.
        assert!((71..=72).contains(&results.width), "results took {}", results.width);
    }

    #[test]
    fn one_column_below_the_split_width_the_preview_reflows_underneath() {
        let area = Rect::new(0, 0, SPLIT_MIN_WIDTH - 1, 40);
        let MainLayout::Split { results, preview } = app(area, true).main else {
            panic!("width alone must never drop the preview");
        };
        assert_eq!((results.x, results.width), (0, area.width));
        assert_eq!((preview.x, preview.width), (0, area.width));
        assert_eq!(preview.y, results.bottom());
        assert_eq!(preview.height, 12, "the stacked preview is a fixed 12 rows");
        assert_eq!(results.height, 34 - 12, "the list absorbs the rest of the main band");
    }

    #[test]
    fn narrowing_a_window_never_costs_the_preview() {
        // The naive responsive move — hide the pane when it gets tight — is exactly what §2
        // rejects, because narrowing a window is not a request to lose the preview.
        for width in 20..=250u16 {
            let l = app(Rect::new(0, 0, width, 40), true);
            assert!(l.main.preview().is_some(), "preview dropped at width {width}");
        }
    }

    #[test]
    fn the_toggle_is_the_only_thing_that_removes_the_preview() {
        for width in [20u16, SPLIT_MIN_WIDTH - 1, SPLIT_MIN_WIDTH, 250] {
            let l = app(Rect::new(0, 0, width, 40), false);
            assert_eq!(l.main.preview(), None, "width {width}");
            assert_eq!(l.main.results(), Rect::new(0, 5, width, 34), "width {width}");
        }
    }

    #[test]
    fn a_terminal_too_small_for_the_bands_still_yields_rects_inside_it() {
        // Five rows against nine rows of constraints, and narrower than any column plan.
        let area = Rect::new(0, 0, 20, 5);
        let l = app(area, true);
        let panes =
            [l.header, l.search, l.filters, l.footer, l.main.results(), l.main.preview().unwrap()];
        for r in panes {
            assert!(r.right() <= area.right(), "{r:?} escaped {area:?} horizontally");
            assert!(r.bottom() <= area.bottom(), "{r:?} escaped {area:?} vertically");
        }
        for row in 0..area.height {
            for column in 0..area.width {
                let _ = scroll_target(area, true, column, row);
            }
        }
    }

    #[test]
    fn the_widest_plan_shows_all_six_columns_and_fills_the_pane_exactly() {
        let c = columns(116);
        assert_eq!(c.agent, Col { x: 0, w: 13 });
        assert_eq!(c.title, Col { x: 14, w: 43 });
        assert_eq!(c.dir, Col { x: 58, w: 30 });
        assert_eq!(c.density, Col { x: 89, w: 10 });
        assert_eq!(c.msgs, Col { x: 100, w: 6 });
        assert_eq!(c.age, Col { x: 107, w: 9 });
        assert_eq!(right_edge(c), 116);
    }

    #[test]
    fn msgs_sheds_first_one_column_under_116() {
        // Directory and the density strip both outrank the message count: 6eb.26 measured
        // date and project as the strongest recognition cues, "stronger than anything in the
        // text", which is the empirical form of §2's "you remember the thing in
        // `personal-site` more reliably than you remember it ran long".
        let c = columns(115);
        assert!(c.msgs.is_hidden());
        assert_eq!((c.agent.w, c.dir.w, c.density.w, c.age.w), (13, 28, 10, 9));
    }

    #[test]
    fn the_100_boundary_is_inclusive_and_the_density_strip_goes_one_below_it() {
        let widths = |w| {
            let c = columns(w);
            (c.agent.w, c.dir.w, c.density.w, c.msgs.w, c.age.w)
        };
        assert_eq!(widths(100), (13, 28, 10, 0, 9));
        assert_eq!(widths(99), (12, 22, 0, 0, 8));
    }

    #[test]
    fn the_72_boundary_is_inclusive_and_the_directory_goes_one_below_it() {
        let widths = |w| {
            let c = columns(w);
            (c.agent.w, c.dir.w, c.density.w, c.msgs.w, c.age.w)
        };
        assert_eq!(widths(72), (12, 22, 0, 0, 8));
        assert_eq!(widths(71), (12, 0, 0, 0, 7));
    }

    #[test]
    fn a_shed_column_gives_up_its_separator_too() {
        // At 71 only agent, title and age survive. Were the three hidden columns still
        // reserving a blank each, age would start three cells late and the title would be
        // three narrower than the pane can actually afford.
        let c = columns(71);
        assert_eq!(c.agent, Col { x: 0, w: 12 });
        assert_eq!(c.title, Col { x: 13, w: 50 });
        assert_eq!(c.age, Col { x: 64, w: 7 });
        assert_eq!(right_edge(c), 71);
        for (name, col) in [("dir", c.dir), ("density", c.density), ("msgs", c.msgs)] {
            assert_eq!(col, Col::default(), "{name} kept coordinates after being shed");
        }
    }

    #[test]
    fn the_title_takes_whatever_the_fixed_columns_leave() {
        // Reserved = fixed widths + one gap each, per plan: 21, 45, 64, 73.
        assert_eq!(columns(60).title.w, 60 - 21);
        assert_eq!(columns(90).title.w, 90 - 45);
        assert_eq!(columns(110).title.w, 110 - 64);
        assert_eq!(columns(200).title.w, 200 - 73);
    }

    #[test]
    fn the_title_floor_wins_over_fitting_the_pane() {
        // 12 agent + 7 age + 2 gaps + a 16-column title needs 37; below that the floor is
        // what refuses to give, because a clipped row beats an unreadable title.
        assert_eq!(columns(37).title.w, TITLE_MIN_WIDTH);
        assert_eq!(right_edge(columns(37)), 37);
        for width in [0u16, 1, 20, 36] {
            let c = columns(width);
            assert_eq!(c.title.w, TITLE_MIN_WIDTH, "width {width}");
            assert_eq!(right_edge(c), 37, "width {width} overran by more than the floor");
        }
    }

    #[test]
    fn visible_columns_never_overlap_and_only_the_floor_overruns_the_pane() {
        for width in 20..=250u16 {
            let c = columns(width);
            let cols = visible(c);
            for pair in cols.windows(2) {
                let ((left_name, left), (right_name, right)) = (pair[0], pair[1]);
                assert_eq!(
                    right.x,
                    left.x + left.w + GAP,
                    "width {width}: {left_name}..{right_name} are not exactly one blank apart",
                );
            }
            let edge = right_edge(c);
            assert!(
                edge == width || (edge > width && c.title.w == TITLE_MIN_WIDTH),
                "width {width}: right edge {edge} is neither an exact fill nor a floor overrun",
            );
        }
    }

    #[test]
    fn the_agent_badge_and_title_survive_every_width() {
        for width in 0..=250u16 {
            let c = columns(width);
            assert!(!c.agent.is_hidden(), "agent shed at width {width}");
            assert!(!c.title.is_hidden(), "title shed at width {width}");
        }
    }

    #[test]
    fn widening_the_pane_never_takes_a_column_away() {
        // Shedding is monotone in width; a plan that lost a column on the way up would make
        // a slow drag-resize flicker columns in and out.
        let count = |w: u16| visible(columns(w)).len();
        for width in 20..250u16 {
            assert!(count(width + 1) >= count(width), "column lost going {width} -> {}", width + 1);
        }
    }

    #[test]
    fn the_wheel_finds_both_panes_of_a_side_by_side_split() {
        let area = Rect::new(0, 0, 140, 40);
        let l = app(area, true);
        let panes = [
            (l.main.results(), ScrollTarget::Results),
            (l.main.preview().unwrap(), ScrollTarget::Preview),
        ];
        for (rect, want) in panes {
            // All four corners: an off-by-one on `right()`/`bottom()` shows up here and
            // nowhere else, and the inner corners are the ones that face the other pane.
            for at in [
                (rect.x, rect.y),
                (rect.right() - 1, rect.y),
                (rect.x, rect.bottom() - 1),
                (rect.right() - 1, rect.bottom() - 1),
            ] {
                assert_eq!(scroll_target(area, true, at.0, at.1), Some(want), "{at:?}");
            }
        }
    }

    #[test]
    fn the_wheel_finds_both_panes_after_the_vertical_reflow() {
        let area = Rect::new(0, 0, 100, 40);
        let l = app(area, true);
        let (results, preview) = (l.main.results(), l.main.preview().unwrap());
        assert_eq!(preview.y, results.bottom(), "expected the stacked layout at width 100");
        let last_list_row = results.bottom() - 1;
        assert_eq!(scroll_target(area, true, 50, last_list_row), Some(ScrollTarget::Results));
        assert_eq!(scroll_target(area, true, 50, preview.y), Some(ScrollTarget::Preview));
    }

    #[test]
    fn a_click_on_the_chrome_scrolls_nothing() {
        let area = Rect::new(0, 0, 140, 40);
        // Rows 0..5 are the header, the three-row search box and the filter bar.
        for row in 0..5 {
            assert_eq!(scroll_target(area, true, 10, row), None, "row {row}");
        }
        assert_eq!(scroll_target(area, true, 10, area.bottom() - 1), None, "footer");
    }

    #[test]
    fn a_position_outside_the_terminal_is_not_a_pane() {
        let area = Rect::new(0, 0, 140, 40);
        assert_eq!(scroll_target(area, true, area.width, 10), None);
        assert_eq!(scroll_target(area, true, 10, area.height), None);
        assert_eq!(scroll_target(area, true, u16::MAX, u16::MAX), None);
    }

    #[test]
    fn with_the_preview_off_the_results_own_the_cells_it_used_to_hold() {
        let area = Rect::new(0, 0, 140, 40);
        let preview = app(area, true).main.preview().unwrap();
        let (column, row) = (preview.x + 1, preview.y + 1);
        assert_eq!(scroll_target(area, true, column, row), Some(ScrollTarget::Preview));
        assert_eq!(scroll_target(area, false, column, row), Some(ScrollTarget::Results));
    }

    #[test]
    fn hit_testing_respects_an_offset_area() {
        // Mouse events carry absolute terminal coordinates, so a pane-relative comparison
        // would land in the wrong pane by exactly the offset.
        let area = Rect::new(10, 5, 140, 40);
        let results = app(area, true).main.results();
        assert_eq!(scroll_target(area, true, results.x, results.y), Some(ScrollTarget::Results));
        assert_eq!(scroll_target(area, true, 0, 0), None);
    }
}
