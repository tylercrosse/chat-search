//! Fitting text into a fixed cell (docs/TUI-DESIGN.md §2).
//!
//! Everything here is display-width aware, not char-count aware: CJK and emoji occupy two
//! columns, and a byte- or char-based truncation overflows the cell and corrupts every
//! column to its right.

use unicode_width::UnicodeWidthStr;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two columns each, so a char-count truncation overshoots by exactly its char count.
    const JP: &str = "日本語ドキュメント";
    /// `e` + U+0301: two chars, one column, and splittable into a mark with nothing to sit on.
    const COMBINING: &str = "e\u{0301}cole normale";
    const EMOJI: &str = "👍👍👍 ok";

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
