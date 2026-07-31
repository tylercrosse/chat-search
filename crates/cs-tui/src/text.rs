//! Fitting text into a fixed cell (docs/TUI-DESIGN.md §2).
//!
//! Everything here is display-width aware, not char-count aware: CJK and emoji occupy two
//! columns, and a byte- or char-based truncation overflows the cell and corrupts every
//! column to its right.
//!
//! Highlighted text is a *list* of styled runs rather than a string, so it gets its own family —
//! [`marked_runs`] to split, [`truncate_runs`] to fit one line and [`wrap_runs`] to fill several.
//! Both the results rows and the preview body draw one, and neither gets to answer the width
//! question its own way.

use cs_core::highlight::Span as Mark;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The cut marker, one column wide: U+2026 is East Asian Ambiguous, which `unicode-width`'s
/// non-CJK tables resolve to 1 — the same resolution a terminal in a non-CJK locale makes.
/// Every budget below subtracts that 1 literally, so `the_marker_is_one_column` pins the
/// assumption against a tables upgrade.
const ELLIPSIS: char = '…';

/// Display columns `s` occupies.
pub fn width(s: &str) -> usize {
    s.width()
}

/// Trim from the end, marking the cut with `…`.
///
/// For titles: the front of a title is the discriminating part. Returns `s` unchanged when
/// it already fits. Never exceeds `w` columns, including the marker; `w == 0` gives `""`,
/// and `w == 1` gives `"…"` rather than a partial glyph.
pub fn truncate_end(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    // `w == 1` falls out of this: the budget is 0, no char fits, and the marker alone is
    // returned.
    let mut out = String::from(&s[..prefix_end(s, w - 1)]);
    out.push(ELLIPSIS);
    out
}

/// Trim from the middle, preserving both ends, marking the cut with `…`.
///
/// For paths: `~/dev/projects/personal-site` tail-truncated to 20 becomes
/// `~/dev/projects/pers…`, which discards exactly the discriminating token. This keeps the
/// leaf, so the same input gives `~/dev/pro…sonal-site`.
///
/// Bias the cut so the *tail* keeps at least as many columns as the head — the leaf
/// directory identifies the conversation, the ancestors rarely do.
pub fn elide_middle(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    // The odd column goes to the tail, so `tail >= head` at every width rather than only at
    // the odd ones. `w = 20` gives 9 + marker + 10, reproducing the head of the doc example
    // above; that example's literal tail is one column longer than its own stated width, and
    // the invariant wins over the transcription.
    let body = w - 1;
    let head_w = body / 2;
    let end = prefix_end(s, head_w);
    let start = suffix_start(s, body - head_w);
    // Unreachable while `width(s) > w`: the two slices are budgeted to `w - 1` columns
    // together, so they cannot between them span a string wider than that.
    debug_assert!(start >= end, "elided ends overlap: {s:?} at {w}");

    // A wide char straddling either budget forfeits its column instead of borrowing across
    // the marker. Borrowing would let the head outgrow the tail on exactly the CJK paths the
    // tail bias exists for.
    let mut out = String::from(&s[..end]);
    out.push(ELLIPSIS);
    out.push_str(&s[start..]);
    out
}

/// `text` split at `marks`, matched runs styled and everything else left alone.
///
/// The two `Span` types this sits between mean opposite things, so the one that is not a run is
/// renamed: `marks` are byte offsets (`cs_core::highlight::Span`, aliased `Mark` here) and the
/// result is styled runs (`ratatui::text::Span`). Nothing here is measured in columns; that is
/// [`truncate_runs`]'s half of the job.
///
/// `mark` is patched *over* `base` rather than replacing it, because [`crate::theme::Theme`]'s
/// match highlight is modifiers only: a match inside the selected row has to keep that row's
/// fill or it reads as a hole punched in the band.
///
/// Marks that are empty, inverted, out of order, overlapping or past the end of `text` are
/// repaired rather than trusted. The offsets and the string arrive by different routes — spans
/// are computed against the snippet a search returned, the row draws whatever it holds now — so
/// a disagreement is a boundary condition rather than a bug to assert on, and a panic inside
/// the draw loop takes the whole TUI down with it.
pub fn marked_runs(text: &str, marks: &[Mark], base: Style, mark: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut at = 0;
    for (start, end) in cleaned(text, marks) {
        if start > at {
            out.push(Span::styled(text[at..start].to_string(), base));
        }
        out.push(Span::styled(text[start..end].to_string(), base.patch(mark)));
        at = end;
    }
    if at < text.len() {
        out.push(Span::styled(text[at..].to_string(), base));
    }
    out
}

/// Trim a run list from the end, marking the cut with `…`.
///
/// [`truncate_end`] for styled text, and the same convention throughout: the list comes back
/// untouched when it already fits, `w == 0` gives nothing, and `w == 1` gives the marker alone.
/// Callers assemble first and truncate second — the results row prepends its `" ↳ "` tag before
/// calling — so a prefix is spent out of the budget instead of on top of it.
///
/// Measures the assembled text rather than summing the runs' widths, for the reason
/// `prefix_end` gives: width belongs to the sequence, not to its pieces. `☺` and U+FE0F measure
/// 1 and 0 apart, and 2 together. [`marked_runs`] keeps its boundaries off those seams so that
/// the two accountings agree — which they must, because ratatui places cells run by run.
///
/// The marker inherits the style of the last surviving run, so the selected row's fill stays
/// one continuous band out to the edge of the cell.
pub fn truncate_runs(runs: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let text: String = runs.iter().map(|r| &*r.content).collect();
    if width(&text) <= w {
        return runs;
    }
    if w == 0 {
        return Vec::new();
    }

    let end = prefix_end(&text, w - 1);
    let mut out = Vec::with_capacity(runs.len() + 1);
    // Nothing survives at `w == 1`, and the marker still has to carry a style; the run the line
    // opens with is the one the cell would have been filled with.
    let mut style = runs.first().map(|r| r.style).unwrap_or_default();
    let mut at = 0;
    for run in runs {
        if at == end {
            break;
        }
        style = run.style;
        let next = at + run.content.len();
        if next <= end {
            out.push(run);
            at = next;
            continue;
        }
        // `end` is a char boundary of the assembly, and a run boundary is one too, so a cut
        // landing inside this run lands between two of its own chars.
        out.push(Span::styled(run.content[..end - at].to_string(), run.style));
        break;
    }
    out.push(Span::styled(ELLIPSIS.to_string(), style));
    out
}

/// Wrap an assembled run list to `w` display columns, returning one run list per terminal row.
///
/// [`truncate_runs`] is what a one-line cell does with text that does not fit; this is what a
/// pane with rows to spend does instead. Greedy and word-first: a row ends at the last whitespace
/// that fits, and a token is broken only when it is wider than the pane on its own — in this
/// corpus a path, a URL or a base64 blob rather than a word.
///
/// How many rows come back is geometry, not decoration: the preview places clicks and counts
/// scroll from it, so a row that draws wider than `w` folds the wrong message rather than merely
/// looking wrong. Every row therefore fits both as its concatenation and as the sum of its runs —
/// ratatui places cells run by run, which is why [`marked_runs`] keeps its boundaries off glyph
/// seams — and rows break only where a run may begin.
///
/// A run cut by a break keeps its style on both halves, so a match that straddles one is still a
/// match on the row below.
///
/// `w == 0` gives no rows at all, since none of them could be drawn. Every other width gives at
/// least one, empty input included: a blank line still occupies a row.
///
/// A glyph wider than the whole pane is dropped, the same trade [`truncate_end`] makes when a
/// wide char straddles its budget. Half a glyph is not drawable, and the alternative — one row
/// that overflows — would lie to the caller about the height it just drew.
pub fn wrap_runs(runs: Vec<Span<'static>>, w: usize) -> Vec<Vec<Span<'static>>> {
    if w == 0 {
        return Vec::new();
    }
    let text: String = runs.iter().map(|r| &*r.content).collect();
    // Measures the assembly rather than the runs, for the reason `truncate_runs` gives.
    if width(&text) <= w {
        return vec![runs];
    }
    let rows = row_ranges(&text, w);
    if rows.is_empty() {
        // Every glyph was wider than the pane. The line is undrawable but it is still a line,
        // and a caller counting rows to find it has to be told so.
        return vec![Vec::new()];
    }
    slice_runs(runs, &rows)
}

/// `$HOME` collapsed to `~`, for display only.
///
/// Returns `"—"` for an empty path: a conversation with no working directory (every ChatGPT
/// one, 6eb.26) must read as "this source has no such thing", not as missing data.
pub fn display_dir(path: &str, home: Option<&str>) -> String {
    if path.is_empty() {
        return "—".to_string();
    }
    let Some(home) = home.map(|h| h.trim_end_matches('/')).filter(|h| !h.is_empty()) else {
        return path.to_string();
    };
    if path == home {
        return "~".to_string();
    }
    // The rest must start at a separator. A plain `starts_with` turns
    // `/Users/tylercrosse` under home `/Users/tyler` into `~crosse`, which reads as a real
    // path and is not one.
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;
/// The mean Gregorian month, 365.2425 / 12 days. Calendar-exact months are unavailable here —
/// `age` takes a duration, which has no start date and therefore no calendar — and the mean
/// buys the property a round 30 days does not: `12 * MONTH_MS == YEAR_MS`, so the last month
/// bucket is `11mo` and no `12mo` ever renders one column from `1y`.
const MONTH_MS: i64 = 2_629_746_000;
const YEAR_MS: i64 = 12 * MONTH_MS;

/// Coarse age for the Age column: `now`, `5m`, `3h`, `12d`, `4mo`, `2y`.
///
/// Relative rather than absolute on purpose — it is scannable, and it is structurally immune
/// to the UTC bug 6eb.8 fixed, because a duration has no timezone. The absolute local date
/// appears once, in the preview header, where it can be verified.
///
/// `ms_ago` is a duration in milliseconds, not a timestamp. Negative input (a clock that
/// moved backwards) renders `now` rather than a negative age.
///
/// Buckets, each truncating toward zero: `now` below a minute (a conversation touched this
/// minute is the one you just left, and `0m` says less than `now`), then minutes below an
/// hour, hours below a day, days below a month — so `30d` is the widest day form — months
/// below a year, years above. Widest output is `11mo` at 4 columns, inside the 7-column Age
/// cell the narrowest layout keeps (layout.rs).
pub fn age(ms_ago: i64) -> String {
    match ms_ago {
        ms if ms < MINUTE_MS => "now".to_string(),
        ms if ms < HOUR_MS => format!("{}m", ms / MINUTE_MS),
        ms if ms < DAY_MS => format!("{}h", ms / HOUR_MS),
        ms if ms < MONTH_MS => format!("{}d", ms / DAY_MS),
        ms if ms < YEAR_MS => format!("{}mo", ms / MONTH_MS),
        ms => format!("{}y", ms / YEAR_MS),
    }
}

/// Byte length of the longest prefix of `s` fitting in `w` columns.
///
/// Measures each candidate prefix rather than summing per-char widths: width is a property of
/// the sequence, not of its chars. A variation selector or a ZWJ joiner is zero columns alone
/// while changing what the run containing it measures, so a per-char sum is an estimate of
/// the wrong string. The scan stops at the first over-budget prefix, which bounds the work by
/// `w` rather than by the length of `s`.
fn prefix_end(s: &str, w: usize) -> usize {
    // Printable ASCII is one column per byte, cannot combine with what follows, and cannot put
    // a char boundary anywhere but a byte boundary — so the answer is arithmetic and the scan
    // below can be skipped entirely.
    //
    // Worth a special case because that scan is quadratic in `w`: it re-measures the whole
    // prefix for every character, so a row costs about w²/2 width lookups. At a 94-column pane
    // over the 9,261 rows of this corpus's longest conversation that is ~42M lookups, and it
    // measured as essentially the entire cost of laying the pane out.
    //
    // Printable rather than merely ASCII: tab and the C0 controls measure zero columns, not one,
    // and a line containing them has to take the general path.
    //
    // The byte just past the cut is checked too, and that is not belt and braces. Anything of
    // zero width attaches to the character before it and so extends the prefix without widening
    // it — `width("e\u{301}") == width("e")` — so stopping at `w` bytes would cut a combining
    // mark off the letter it belongs to. Requiring the next byte to be printable ASCII means it
    // is a standalone column that genuinely does not fit.
    let n = w.min(s.len());
    let printable = |b: &u8| b.is_ascii_graphic() || *b == b' ';
    if s.as_bytes()[..n].iter().all(printable)
        && s.as_bytes().get(n).map_or(true, |b| printable(b))
    {
        return n;
    }
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        let next = i + ch.len_utf8();
        if width(&s[..next]) > w {
            break;
        }
        end = next;
    }
    end
}

/// Byte offset of the longest suffix of `s` fitting in `w` columns. See [`prefix_end`].
fn suffix_start(s: &str, w: usize) -> usize {
    let mut start = s.len();
    for (i, _) in s.char_indices().rev() {
        if width(&s[i..]) > w {
            break;
        }
        start = i;
    }
    start
}

/// `marks` as ascending, non-overlapping, sliceable byte ranges of `text`.
///
/// Each mark is widened to the boundaries [`glyph_start`] and [`glyph_end`] allow and then
/// merged with its neighbours, so what comes back can be sliced out of `text` in one pass.
/// Widening rather than narrowing: a mark that half-covers something still says the row matched
/// there, and dropping it would be the one outcome that misleads.
fn cleaned(text: &str, marks: &[Mark]) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = marks
        .iter()
        .map(|m| (glyph_start(text, m.start), glyph_end(text, m.end)))
        .filter(|&(s, e)| s < e)
        .collect();
    spans.sort_unstable();

    let mut out: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (s, e) in spans {
        match out.last_mut() {
            // Touching counts as overlapping: two marks that meet are one run, and a boundary
            // between them would be invisible anyway.
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// The nearest boundary at or before `i` that a run may start on.
fn glyph_start(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    // The char-boundary test comes first and short-circuits: `splits_a_glyph` slices `s` at `i`.
    while i > 0 && (!s.is_char_boundary(i) || splits_a_glyph(s, i)) {
        i -= 1;
    }
    i
}

/// The nearest boundary at or after `i` that a run may start on. See [`glyph_start`].
fn glyph_end(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i < s.len() && (!s.is_char_boundary(i) || splits_a_glyph(s, i)) {
        i += 1;
    }
    i
}

/// Whether a run starting at `i` would begin mid-glyph, `i` being a char boundary.
///
/// Splitting one is not a crash, it is a miscount, and in both directions: `☺` + U+FE0F is 2
/// columns whole and measures 1 in pieces, while `👨`+ZWJ+`👩` is 2 whole and measures 4. So
/// the cell either overflows or is left short, depending on which glyph it was — and neither
/// shows up as a panic anyone can find later.
fn splits_a_glyph(s: &str, i: usize) -> bool {
    // A zero-width char modifies what precedes it, so it can never open a run. Control chars
    // report no width at all and are treated the same way; tool output does contain them
    // (`cs_core::highlight`), and attaching one to its neighbour is as good an answer as any.
    if s[i..].chars().next().is_some_and(|c| c.width().unwrap_or(0) == 0) {
        return true;
    }
    // U+200D is the joiner that binds *forward* — the emoji after it does not stand alone —
    // so the seam is behind the boundary rather than in front of it.
    s[..i].chars().next_back() == Some('\u{200D}')
}

/// The byte range each row of `text` draws, wrapped greedily to `w` columns.
///
/// Ranges ascend and never overlap, but they need not meet: the whitespace a break was taken at
/// belongs to neither of the rows it separates. Every endpoint bar the end of `text` itself is a
/// boundary [`glyph_start`] allows, so a row measures the same in pieces as it does whole.
///
/// Terminates because each pass moves `at` strictly forward — past a row, or past one glyph no
/// row could hold. A pass that left it standing still would hang the draw loop, not the wrap.
fn row_ranges(text: &str, w: usize) -> Vec<(usize, usize)> {
    let mut rows = Vec::new();
    let mut at = 0;
    while at < text.len() {
        // The widest prefix that fits. Asking for it rather than first measuring the remainder
        // is what keeps a wrap linear in the length of the line: `prefix_end` stops one glyph
        // past the pane, where measuring the remainder rereads a 100KB tool-output line per row.
        let fit = at + prefix_end(&text[at..], w);
        if fit == text.len() {
            rows.push((at, fit));
            break;
        }
        // Pulled back to a boundary a run may start on.
        let limit = glyph_start(text, fit);
        if limit <= at {
            at = glyph_end(text, at + 1);
            continue;
        }
        let (end, next) = break_at(text, at, limit);
        rows.push((at, end));
        at = next;
    }
    rows
}

/// Where the row starting at `at` ends and where the next one starts, `limit` being the furthest
/// this one may reach.
///
/// The two differ whenever the break lands on whitespace: those columns are why the break is
/// there, and drawing them again down the left of the next row would give a wrapped paragraph a
/// ragged edge that moves as the pane resizes.
///
/// A break that would leave this row empty — an indent as wide as the pane — or that would not
/// advance is refused in favour of splitting at `limit`. That costs a word, which is the only
/// alternative that both draws every byte and finishes.
fn break_at(text: &str, at: usize, limit: usize) -> (usize, usize) {
    let cut = if text[limit..].starts_with(char::is_whitespace) {
        // The token ends at or before the edge, so the row keeps all of it.
        limit
    } else {
        match text[at..limit].rfind(char::is_whitespace) {
            // Hand the half-drawn token back to the next row, starting it after the whitespace —
            // nothing between there and `limit` is whitespace, or `rfind` would have found it.
            Some(i) => text.len() - text[at + i..].trim_start().len(),
            // One token wider than the pane: a path, a URL, a base64 blob. Breaking it is the
            // only alternative to overflowing.
            None => limit,
        }
    };
    let end = glyph_start(text, at + text[at..cut].trim_end().len());
    let next = glyph_start(text, text.len() - text[cut..].trim_start().len());
    if end <= at || next <= at {
        return (limit, limit);
    }
    (end, next)
}

/// `runs` re-cut to `ranges`, one run list per range.
///
/// A run straddling a range boundary is split and both halves keep its style. Bytes between two
/// ranges belong to no row and are dropped, which is how the whitespace at a break stops being
/// drawn. Both lists ascend, so this walks each of them once.
fn slice_runs(runs: Vec<Span<'static>>, ranges: &[(usize, usize)]) -> Vec<Vec<Span<'static>>> {
    let mut out = Vec::with_capacity(ranges.len());
    let mut i = 0;
    // Byte offset of `runs[i]` in the assembled text.
    let mut at = 0;
    for &(start, end) in ranges {
        let mut row = Vec::new();
        while i < runs.len() && at < end {
            let run = &runs[i];
            // Both ends are char boundaries of the assembly and a run boundary is one too, so a
            // range landing inside this run lands between two of its own chars.
            let lo = start.saturating_sub(at);
            let hi = (end - at).min(run.content.len());
            if lo < hi {
                row.push(Span::styled(run.content[lo..hi].to_string(), run.style));
            }
            if at + run.content.len() > end {
                break;
            }
            at += run.content.len();
            i += 1;
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    /// Two columns each, so a char-count truncation overshoots by exactly its char count.
    const JP: &str = "日本語ドキュメント";
    /// `e` + U+0301: two chars, one column, and splittable into a mark with nothing to sit on.
    const COMBINING: &str = "e\u{0301}cole normale";
    const EMOJI: &str = "👍👍👍 ok";

    /// Distinguishable from `hl()` under `==`, which is all the run tests ask of either.
    fn base() -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    fn hl() -> Style {
        Style::new().add_modifier(Modifier::REVERSED)
    }

    fn mark(start: usize, end: usize) -> Mark {
        Mark { start, end }
    }

    /// What the run list puts on screen, with the styles dropped.
    fn drawn(runs: &[Span<'static>]) -> String {
        runs.iter().map(|r| &*r.content).collect()
    }

    /// Unmarked text as the one run a caller would hand a wrap.
    fn plain(s: &str) -> Vec<Span<'static>> {
        marked_runs(s, &[], base(), hl())
    }

    /// What a wrap puts on screen, one string per row. See [`drawn`].
    fn rows(runs: Vec<Span<'static>>, w: usize) -> Vec<String> {
        wrap_runs(runs, w).iter().map(|r| drawn(r)).collect()
    }

    #[test]
    fn the_marker_is_one_column() {
        // Every budget in this module is `w - 1`. A tables upgrade that resolved ambiguous
        // width to 2 would make each of them off by one, silently, in the direction that
        // overflows the cell.
        assert_eq!(width(&ELLIPSIS.to_string()), 1);
    }

    #[test]
    fn width_counts_columns_not_chars() {
        assert_eq!(width("hello"), 5);
        assert_eq!(width(JP), 18); // 9 chars
        assert_eq!(width("👍"), 2); // 1 char
        assert_eq!(width("e\u{0301}"), 1); // 2 chars
        assert_eq!(width(""), 0);
    }

    #[test]
    fn a_string_that_fits_comes_back_unchanged_with_no_marker() {
        assert_eq!(truncate_end("hello", 5), "hello");
        assert_eq!(truncate_end("hello", 99), "hello");
        assert_eq!(truncate_end(JP, 18), JP);
        assert_eq!(elide_middle("~/dev", 5), "~/dev");
        assert_eq!(elide_middle(JP, 18), JP);
        // Exactly-fits is the common case in a laid-out table; a marker here would spend a
        // column to say nothing was cut.
        assert!(!truncate_end(JP, 18).contains(ELLIPSIS));
    }

    #[test]
    fn zero_columns_is_empty_and_one_column_is_the_marker_alone() {
        assert_eq!(truncate_end("hello", 0), "");
        assert_eq!(truncate_end(JP, 0), "");
        assert_eq!(truncate_end("hello", 1), "…");
        // Not a half-drawn `日`: the cell has room for one column and the glyph needs two.
        assert_eq!(truncate_end(JP, 1), "…");
        assert_eq!(elide_middle("hello", 0), "");
        assert_eq!(elide_middle("hello", 1), "…");
        // An empty input fits every width, marker included.
        assert_eq!(truncate_end("", 0), "");
        assert_eq!(elide_middle("", 0), "");
    }

    #[test]
    fn a_wide_char_straddling_the_budget_is_dropped_not_split() {
        // Budget 4 after the marker: two kana fit exactly.
        assert_eq!(truncate_end(JP, 5), "日本…");
        // Budget 5: the third kana would make 6, so the row gives up a column rather than
        // painting half a glyph into the next cell. A char-based truncate takes 5 chars here
        // and writes 11 columns into a 6-column cell.
        assert_eq!(truncate_end(JP, 6), "日本…");
        assert_eq!(width(truncate_end(JP, 6).as_str()), 5);
        assert_eq!(truncate_end(EMOJI, 3), "👍…");
        assert!(width(truncate_end(EMOJI, 3).as_str()) <= 3);
    }

    #[test]
    fn truncation_never_exceeds_its_width_for_any_input() {
        // The property that matters: an over-wide cell corrupts every column to its right,
        // so this is asserted over the shapes that break a char-count implementation rather
        // than over one example of each.
        let inputs = [
            "",
            "a",
            "hello world, a plain ascii title",
            JP,
            COMBINING,
            EMOJI,
            "日本 mixed 語 ascii 混在 text",
            "👨‍👩‍👧 family zwj sequence",
            "a\u{0301}\u{0302}\u{0303} stacked marks",
            "~/dev/projects/personal-site",
            "☺\u{FE0F} variation selector",
        ];
        for s in inputs {
            for w in 0..=40 {
                let cut = truncate_end(s, w);
                assert!(
                    width(&cut) <= w,
                    "truncate_end({s:?}, {w}) = {cut:?} is {} columns",
                    width(&cut)
                );
                let mid = elide_middle(s, w);
                assert!(
                    width(&mid) <= w,
                    "elide_middle({s:?}, {w}) = {mid:?} is {} columns",
                    width(&mid)
                );
                // Fitting inputs are passed through by both, at every width above their own.
                if width(s) <= w {
                    assert_eq!(cut, s);
                    assert_eq!(mid, s);
                }
            }
        }
    }

    #[test]
    fn elide_middle_keeps_the_leaf_that_tail_elision_throws_away() {
        let path = "~/dev/projects/personal-site";
        // The failure mode from docs/TUI-DESIGN.md §2: 20 columns of a path whose last token
        // is the only discriminating one, and tail elision spends all 20 on the ancestors.
        assert_eq!(truncate_end(path, 20), "~/dev/projects/pers…");

        let mid = elide_middle(path, 20);
        assert!(width(&mid) <= 20, "{mid:?}");
        assert!(mid.starts_with("~/dev/pro"), "{mid:?}");
        assert!(mid.ends_with("sonal-site"), "{mid:?}");
        assert!(path.ends_with(mid.split(ELLIPSIS).nth(1).unwrap()), "tail is a real suffix");
    }

    #[test]
    fn the_tail_keeps_at_least_as_many_columns_as_the_head() {
        let path = "~/dev/projects/personal-site";
        for w in 2..width(path) {
            let mid = elide_middle(path, w);
            let (head, tail) = mid.split_once(ELLIPSIS).unwrap();
            assert!(
                width(tail) >= width(head),
                "elide_middle({path:?}, {w}) = {mid:?} favours the head"
            );
        }
        // Spelled out at one odd and one even width, since the rule is about where the
        // leftover column lands: 21 splits 10/10, 20 splits 9/10 rather than 10/9.
        assert_eq!(elide_middle(path, 21), "~/dev/proj…sonal-site");
        assert_eq!(elide_middle(path, 20), "~/dev/pro…sonal-site");
    }

    #[test]
    fn text_with_no_marks_is_one_run_in_the_base_style() {
        let runs = marked_runs("hello", &[], base(), hl());
        assert_eq!(runs, vec![Span::styled("hello".to_string(), base())]);
        // Nothing to draw is no runs, not one empty one.
        assert!(marked_runs("", &[mark(0, 4)], base(), hl()).is_empty());
    }

    #[test]
    fn only_the_marked_bytes_carry_the_highlight() {
        let s = "commit the commits";
        let runs = marked_runs(s, &[mark(0, 6), mark(11, 18)], base(), hl());
        assert_eq!(drawn(&runs), s, "the split is lossless");
        assert_eq!(
            runs,
            vec![
                Span::styled("commit".to_string(), base().patch(hl())),
                Span::styled(" the ".to_string(), base()),
                Span::styled("commits".to_string(), base().patch(hl())),
            ]
        );
    }

    #[test]
    fn the_highlight_is_layered_over_the_row_style_not_swapped_for_it() {
        // `Theme::match_hl` is modifiers alone, so a match on the selected row that took the
        // highlight style *instead of* the row's would drop the fill and read as a gap in it.
        let selected = Style::new().fg(Color::Black).bg(Color::Gray);
        let runs = marked_runs("borrow checker", &[mark(0, 6)], selected, hl());
        assert_eq!(runs[0].style.bg, Some(Color::Gray));
        assert!(runs[0].style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn a_mark_never_cuts_a_character_in_half() {
        // `é` is two bytes, so a mark that ends at 4 ends inside it. Slicing there panics, and
        // the honest repair is to take the whole char: half of one is not drawable.
        let s = "café au lait";
        assert_eq!(marked_runs(s, &[mark(0, 4)], base(), hl())[0].content, "café");
        assert_eq!(marked_runs(s, &[mark(4, 8)], base(), hl())[1].content, "é au");
        for m in [mark(0, 4), mark(4, 8), mark(4, 5)] {
            assert_eq!(drawn(&marked_runs(s, &[m], base(), hl())), s, "{m:?}");
        }
    }

    #[test]
    fn marks_out_of_order_or_overlapping_become_one_ascending_set_of_runs() {
        let s = "borrow checker borrow";
        let runs = marked_runs(s, &[mark(15, 21), mark(0, 6), mark(3, 10)], base(), hl());
        assert_eq!(
            runs.iter().map(|r| &*r.content).collect::<Vec<_>>(),
            ["borrow che", "cker ", "borrow"]
        );
        assert_eq!(drawn(&runs), s);
    }

    #[test]
    fn a_mark_outside_the_text_is_repaired_rather_than_panicked_on() {
        let s = "short";
        assert_eq!(drawn(&marked_runs(s, &[mark(0, 900)], base(), hl())), s);
        assert_eq!(drawn(&marked_runs(s, &[mark(900, 901)], base(), hl())), s);
        assert_eq!(drawn(&marked_runs(s, &[mark(usize::MAX, usize::MAX)], base(), hl())), s);
        // Empty and inverted marks say nothing, so they mark nothing.
        let runs = marked_runs(s, &[mark(4, 1), mark(2, 2)], base(), hl());
        assert_eq!(runs, [Span::styled("short".to_string(), base())]);
    }

    #[test]
    fn a_run_never_begins_with_something_that_belongs_to_the_glyph_before_it() {
        // `☺` is one column and `☺\u{FE0F}` is two, so a run boundary between them measures 1
        // where the terminal draws 2 — the split is the bug, not the width table.
        let vs = "☺\u{FE0F} ok";
        let runs = marked_runs(vs, &[mark(0, 3)], base(), hl());
        assert_eq!(runs[0].content, "☺\u{FE0F}");
        // ZWJ binds forwards, so a boundary just after one is the same seam seen from behind:
        // `👨` and `\u{200D}👩` measure 2 + 2 against the 2 columns the family draws.
        let zwj = "👨\u{200D}👩!";
        assert_eq!(marked_runs(zwj, &[mark(1, 8)], base(), hl())[0].content, "👨\u{200D}👩");
        for s in [vs, zwj, COMBINING] {
            let runs = marked_runs(s, &[mark(1, 4)], base(), hl());
            let pieces: usize = runs.iter().map(|r| width(&r.content)).sum();
            assert_eq!(pieces, width(s), "{s:?} measures differently in pieces: {runs:?}");
        }
    }

    #[test]
    fn a_run_list_that_fits_comes_back_unchanged_with_no_marker() {
        let runs = marked_runs("hello", &[mark(0, 2)], base(), hl());
        assert_eq!(truncate_runs(runs.clone(), 5), runs);
        assert_eq!(truncate_runs(runs.clone(), 99), runs);
        assert!(!drawn(&truncate_runs(runs, 5)).contains(ELLIPSIS));
        assert!(truncate_runs(Vec::new(), 0).is_empty());
    }

    #[test]
    fn a_run_cut_by_truncation_keeps_its_style_on_what_survives() {
        let runs = marked_runs("borrow checker", &[mark(0, 6)], base(), hl());
        let cut = truncate_runs(runs, 4);
        assert_eq!(drawn(&cut), "bor…");
        assert_eq!(cut[0].style, base().patch(hl()), "the surviving half is still a match");
        assert_eq!(cut[1].style, base().patch(hl()), "and the marker continues it");
        // A cut past the match leaves the match whole and the marker on the run it landed in.
        let runs = marked_runs("borrow checker", &[mark(0, 6)], base(), hl());
        let cut = truncate_runs(runs, 10);
        assert_eq!(drawn(&cut), "borrow ch…");
        assert_eq!(cut[0].style, base().patch(hl()));
        assert_eq!(cut[2].style, base());
    }

    #[test]
    fn truncating_runs_counts_columns_not_bytes_or_chars() {
        // Each kana is 3 bytes and 2 columns, so a byte cut keeps two of nine and a char cut
        // writes 10 columns into a 5-column cell.
        let runs = marked_runs(JP, &[mark(0, 6)], base(), hl());
        assert_eq!(drawn(&truncate_runs(runs.clone(), 5)), "日本…");
        // Budget 5 after the marker: the third kana would make 6, so the cell gives up a
        // column rather than painting half a glyph into its neighbour.
        assert_eq!(drawn(&truncate_runs(runs.clone(), 6)), "日本…");
        assert_eq!(width(&drawn(&truncate_runs(runs, 6))), 5);
        let runs = marked_runs(EMOJI, &[mark(0, 4)], base(), hl());
        assert_eq!(drawn(&truncate_runs(runs, 3)), "👍…");
    }

    #[test]
    fn zero_columns_drops_every_run_and_one_column_is_the_marker_alone() {
        let runs = marked_runs("hello", &[mark(0, 5)], base(), hl());
        assert!(truncate_runs(runs.clone(), 0).is_empty());
        let one = truncate_runs(runs, 1);
        assert_eq!(drawn(&one), "…");
        // Still styled: an unstyled marker on the selected row is a hole in the fill.
        assert_eq!(one[0].style, base().patch(hl()));
        // Not a half-drawn `日` either.
        assert_eq!(drawn(&truncate_runs(marked_runs(JP, &[], base(), hl()), 1)), "…");
    }

    #[test]
    fn a_prefix_the_caller_prepends_is_spent_out_of_the_same_budget() {
        // The results row leads its snippet with a `" ↳ "` tag. Truncating the snippet first
        // and prepending afterwards spends three columns the cell was never given, which is
        // why truncation takes the assembled list rather than the text.
        let mut runs = vec![Span::styled(" ↳ ".to_string(), base())];
        runs.extend(marked_runs("borrow checker", &[mark(0, 6)], base(), hl()));
        let cut = truncate_runs(runs, 10);
        assert_eq!(drawn(&cut), " ↳ borrow…");
        assert_eq!(width(&drawn(&cut)), 10);
        assert_eq!(cut[0].style, base(), "the tag is not part of the match");
        assert_eq!(cut[1].style, base().patch(hl()));
    }

    #[test]
    fn a_run_list_never_exceeds_its_width_for_any_input() {
        // Same property as `truncation_never_exceeds_its_width_for_any_input`, over the axis
        // this pair adds: the marks decide where the run boundaries fall, and a boundary is
        // exactly where a column count goes wrong. The mark lists are the shapes a caller can
        // hand over when its offsets were computed against some other string.
        let marks = [
            vec![],
            vec![mark(0, 1)],
            vec![mark(1, 4)],
            vec![mark(0, usize::MAX)],
            vec![mark(3, 2)],
            vec![mark(2, 2)],
            vec![mark(5, 9), mark(0, 7)],
            vec![mark(usize::MAX - 1, usize::MAX)],
        ];
        let inputs = [
            "",
            "a",
            "hello world, a plain ascii title",
            JP,
            COMBINING,
            EMOJI,
            "日本 mixed 語 ascii 混在 text",
            "👨‍👩‍👧 family zwj sequence",
            "a\u{0301}\u{0302}\u{0303} stacked marks",
            "☺\u{FE0F} variation selector",
        ];
        for s in inputs {
            for ms in &marks {
                let runs = marked_runs(s, ms, base(), hl());
                assert_eq!(drawn(&runs), s, "{s:?} with {ms:?} lost or duplicated text");
                for w in 0..=40 {
                    let cut = truncate_runs(runs.clone(), w);
                    let out = drawn(&cut);
                    assert!(
                        width(&out) <= w,
                        "truncate_runs({s:?} marked {ms:?}, {w}) = {out:?} is {} columns",
                        width(&out)
                    );
                    // Ratatui lays the runs out one after another, so the pieces have to fit
                    // on their own terms and not only once concatenated.
                    let pieces: usize = cut.iter().map(|r| width(&r.content)).sum();
                    assert!(pieces <= w, "{s:?} marked {ms:?} at {w}: pieces measure {pieces}");
                    if width(s) <= w {
                        assert_eq!(cut, runs, "a list that fits is passed through");
                    }
                }
            }
        }
    }

    #[test]
    fn a_run_list_that_already_fits_comes_back_as_a_single_row() {
        let runs = marked_runs("hello", &[mark(0, 2)], base(), hl());
        assert_eq!(wrap_runs(runs.clone(), 5), vec![runs.clone()]);
        assert_eq!(wrap_runs(runs, 99).len(), 1);
        // A blank line still occupies a row: the preview counts rows to decide which message a
        // click landed in, and a line that reported none would shift every message below it.
        assert_eq!(wrap_runs(Vec::new(), 10), vec![Vec::new()]);
    }

    #[test]
    fn zero_columns_gives_no_rows_at_all() {
        // Not one empty row: a pane no columns wide has nowhere to draw it, and a row the
        // caller counts but never sees is exactly the disagreement its geometry cannot survive.
        assert!(wrap_runs(plain("hello"), 0).is_empty());
        assert!(wrap_runs(Vec::new(), 0).is_empty());
    }

    #[test]
    fn a_row_breaks_at_whitespace_rather_than_inside_a_word() {
        let s = "the borrow checker rejects this";
        assert_eq!(rows(plain(s), 12), ["the borrow", "checker", "rejects this"]);
        // Narrow enough that the break moves, to pin that it is the *last* whitespace that fits
        // rather than the first one found.
        assert_eq!(rows(plain(s), 18), ["the borrow checker", "rejects this"]);
    }

    #[test]
    fn a_token_wider_than_the_pane_is_broken_rather_than_left_to_overflow() {
        // Paths, URLs and base64 blobs are everywhere in this corpus and none of them offers a
        // space to break at. Overflowing would be silent; spinning looking for a break would not.
        let s = "see /Users/t/dev/projects/chat-search/crates/cs-tui/src/text.rs now";
        let out = rows(plain(s), 20);
        assert_eq!(out[0], "see", "the word before the token still wraps as a word");
        assert!(out.iter().all(|r| width(r) <= 20), "{out:?}");
        assert_eq!(out.concat().replace(' ', ""), s.replace(' ', ""), "no byte lost");
    }

    #[test]
    fn a_run_broken_across_rows_keeps_its_style_on_both_halves() {
        // A match wider than the pane. If the highlight stopped at the break, the second row
        // would read as ordinary text and the eye would lose the hit it came for.
        let runs = marked_runs("supercalifragilistic", &[mark(0, 20)], base(), hl());
        let out = wrap_runs(runs, 8);
        let lines: Vec<String> = out.iter().map(|r| drawn(r)).collect();
        assert_eq!(lines, ["supercal", "ifragili", "stic"]);
        let styles: Vec<Style> = out.iter().flatten().map(|r| r.style).collect();
        assert!(styles.iter().all(|s| *s == base().patch(hl())), "{styles:?}");
        // And a row that holds both keeps them apart.
        let runs = marked_runs("the borrow checker", &[mark(4, 10)], base(), hl());
        let out = wrap_runs(runs, 10);
        assert_eq!(drawn(&out[0]), "the borrow");
        assert_eq!(out[0][0].style, base());
        assert_eq!(out[0][1].style, base().patch(hl()));
    }

    #[test]
    fn whitespace_at_a_break_is_drawn_by_neither_row() {
        assert_eq!(rows(plain("alpha    beta"), 6), ["alpha", "beta"]);
        // Whitespace inside a row is content, though, and an indent is not a break: a wrapped
        // code line that lost its leading columns would stop reading as code.
        assert_eq!(rows(plain("    indented tail"), 12), ["    indented", "tail"]);
    }

    #[test]
    fn wrapping_counts_columns_not_bytes_or_chars() {
        // Three kana to the row at six columns: nine bytes and three chars, and neither of those
        // is the number the pane has.
        assert_eq!(rows(plain(JP), 6), ["日本語", "ドキュ", "メント"]);
        // An odd width leaves the last column empty rather than painting half a kana into it.
        assert_eq!(rows(plain(JP), 7), ["日本語", "ドキュ", "メント"]);
        assert_eq!(rows(plain(EMOJI), 4), ["👍👍", "👍", "ok"]);
    }

    #[test]
    fn a_glyph_wider_than_the_pane_is_dropped_because_no_row_could_draw_it() {
        // One column against two-column kana: breaking draws half a glyph and overflowing
        // corrupts the column to its right, so the honest answer is the empty row — and it is
        // still a row, because the caller's arithmetic counts it.
        assert_eq!(rows(plain(JP), 1), [""]);
        // Only the undrawable glyphs go; what fits is still drawn.
        assert_eq!(rows(plain("a日b"), 1), ["a", "b"]);
        assert_eq!(rows(plain("hello"), 1), ["h", "e", "l", "l", "o"]);
    }

    #[test]
    fn every_row_of_a_wrap_fits_its_width_for_any_input() {
        // The property `a_run_list_never_exceeds_its_width_for_any_input` asserts, over the
        // second axis this adds: a wrap answers with a *height* the caller then places clicks
        // and scroll offsets against, so losing text or drawing past `w` misfolds a message
        // rather than merely looking wrong. Reaching the end of the loop is half the assertion —
        // a token with no break in it is the shape that spins.
        let marks = [
            vec![],
            vec![mark(0, 4)],
            vec![mark(3, 12)],
            vec![mark(0, usize::MAX)],
            vec![mark(9, 4)],
            vec![mark(5, 9), mark(0, 7)],
        ];
        let inputs = [
            "",
            "a",
            "hello world, a plain ascii title",
            "    indented, then    doubled   spaces   ",
            JP,
            COMBINING,
            EMOJI,
            "日本 mixed 語 ascii 混在 text",
            "👨‍👩‍👧 family zwj sequence",
            "a\u{0301}\u{0302}\u{0303} stacked marks",
            "☺\u{FE0F} variation selector",
            "/Users/t/dev/projects/chat-search/crates/cs-tui/src/text.rs:128",
            "aGVsbG8gd29ybGQgdGhpcyBpcyBhIGJhc2U2NCBibG9iIHdpdGggbm8gc3BhY2Vz",
        ];
        for s in inputs {
            for ms in &marks {
                let runs = marked_runs(s, ms, base(), hl());
                for w in 1..=40 {
                    let out = wrap_runs(runs.clone(), w);
                    assert!(!out.is_empty(), "wrapping {s:?} at {w} reported no rows at all");
                    for row in &out {
                        let line = drawn(row);
                        assert!(width(&line) <= w, "{s:?} at {w}: row {line:?} overflows");
                        // Ratatui lays the runs out one after another, so the pieces have to fit
                        // on their own terms and not only once concatenated.
                        let pieces: usize = row.iter().map(|r| width(&r.content)).sum();
                        assert!(pieces <= w, "{s:?} at {w}: row {line:?} measures {pieces} apart");
                    }
                    let drawn_rows: String = out.iter().map(|r| drawn(r)).collect();
                    assert_recovers(s, &drawn_rows, w);
                }
            }
        }
    }

    /// Assert `out` is `s` with only the two things a wrap may drop missing.
    ///
    /// Those are the whitespace a break was taken at, and a glyph too wide for any row — which
    /// no input here is above `w == 1`, none of them being more than two columns. Anything else,
    /// a lost word or a duplicated or reordered one, surfaces as a skipped run that is neither
    /// or as text left over at the end.
    fn assert_recovers(s: &str, out: &str, w: usize) {
        let legal = |run: &str| run.chars().all(char::is_whitespace) || (w == 1 && width(run) > 1);
        let mut rest = out;
        let mut skipped = String::new();
        for c in s.chars() {
            match rest.strip_prefix(c) {
                Some(r) => {
                    assert!(legal(&skipped), "wrapping {s:?} at {w} lost {skipped:?}: {out:?}");
                    skipped.clear();
                    rest = r;
                }
                None => skipped.push(c),
            }
        }
        assert!(legal(&skipped), "wrapping {s:?} at {w} lost {skipped:?}: {out:?}");
        assert!(rest.is_empty(), "wrapping {s:?} at {w} invented {rest:?}");
    }

    #[test]
    fn display_dir_collapses_home_only_at_a_separator() {
        let home = Some("/Users/tylercrosse");
        assert_eq!(
            display_dir("/Users/tylercrosse/dev/projects/chat-search", home),
            "~/dev/projects/chat-search"
        );
        assert_eq!(display_dir("/Users/tylercrosse", home), "~");
        assert_eq!(display_dir("/Users/tylercrosse/", home), "~/");
        // Another user whose name extends this one: `~crosse/...` would read as a path and
        // point somewhere that does not exist.
        assert_eq!(
            display_dir("/Users/tylercrosse2/dev", Some("/Users/tylercrosse")),
            "/Users/tylercrosse2/dev"
        );
        assert_eq!(display_dir("/opt/homebrew/bin", home), "/opt/homebrew/bin");
        assert_eq!(display_dir("/Users/tylercrosse/dev", None), "/Users/tylercrosse/dev");
        // A trailing separator on $HOME is legal in the environment and must not defeat the
        // match, nor produce `~//dev`.
        assert_eq!(display_dir("/Users/tylercrosse/dev", Some("/Users/tylercrosse/")), "~/dev");
        // `HOME=` unset-but-present, and `HOME=/`, would otherwise collapse every path.
        assert_eq!(display_dir("/Users/tylercrosse/dev", Some("")), "/Users/tylercrosse/dev");
        assert_eq!(display_dir("/Users/tylercrosse/dev", Some("/")), "/Users/tylercrosse/dev");
    }

    #[test]
    fn an_absent_working_directory_reads_as_absent_not_missing() {
        assert_eq!(display_dir("", Some("/Users/tylercrosse")), "—");
        assert_eq!(display_dir("", None), "—");
    }

    #[test]
    fn each_age_bucket_changes_at_its_own_boundary() {
        assert_eq!(age(0), "now");
        assert_eq!(age(MINUTE_MS - 1), "now");
        assert_eq!(age(MINUTE_MS), "1m");
        assert_eq!(age(HOUR_MS - 1), "59m");
        assert_eq!(age(HOUR_MS), "1h");
        assert_eq!(age(DAY_MS - 1), "23h");
        assert_eq!(age(DAY_MS), "1d");
        assert_eq!(age(MONTH_MS - 1), "30d");
        assert_eq!(age(MONTH_MS), "1mo");
        assert_eq!(age(YEAR_MS - 1), "11mo");
        assert_eq!(age(YEAR_MS), "1y");
        // The samples the doc comment advertises.
        assert_eq!(age(5 * MINUTE_MS), "5m");
        assert_eq!(age(3 * HOUR_MS), "3h");
        assert_eq!(age(12 * DAY_MS), "12d");
        assert_eq!(age(4 * MONTH_MS), "4mo");
        assert_eq!(age(2 * YEAR_MS), "2y");
    }

    #[test]
    fn a_backwards_clock_renders_now_rather_than_a_negative_age() {
        // Import stamps come from other machines (cs-archive scans a synced tree), so a
        // future `ended_at` is a skew artefact, not corruption — `-3m` in the column would
        // read as a bug in the search.
        assert_eq!(age(-1), "now");
        assert_eq!(age(-DAY_MS), "now");
        assert_eq!(age(i64::MIN), "now");
    }

    #[test]
    fn age_never_outgrows_the_narrowest_age_column() {
        // 7 columns at inner width < 72 (layout.rs), and the Age header shares the cell.
        for ms in [0, MINUTE_MS, HOUR_MS, DAY_MS, 30 * DAY_MS, YEAR_MS - 1, 99 * YEAR_MS] {
            assert!(width(&age(ms)) <= 5, "{ms} renders {:?}", age(ms));
        }
    }
}

