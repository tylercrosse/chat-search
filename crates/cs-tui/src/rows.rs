//! Flattening the result tree into a cursor (docs/TUI-DESIGN.md §3).
//!
//! `search_grouped` returns conversations with their hits nested beneath. A cursor is flat.
//! Materialising the flat vector once per search — rather than computing offsets from the
//! tree during render — is what keeps scrolling, `PageDown` and mouse hit-testing uniform.

use std::collections::HashSet;

use cs_core::search::Group;

/// One navigable line. Indices point into the `&[Group]` the vector was built from, so a row
/// is meaningless without it; rebuild both together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// A conversation. Carries the density strip, aggregates and title.
    Header { group: usize },
    /// One matching message beneath its conversation.
    Hit { group: usize, hit: usize },
}

impl Row {
    /// The conversation this row acts on. A hit row resolves to its parent — no terminal
    /// resume can open at a message, and the `Pick` event records the conversation's rank,
    /// not the hit's.
    pub fn group(self) -> usize {
        match self {
            Row::Header { group } | Row::Hit { group, .. } => group,
        }
    }

    pub fn is_header(self) -> bool {
        matches!(self, Row::Header { .. })
    }
}

/// Hit rows shown under one expanded conversation. Uncapped, a single verbose conversation
/// fills the viewport; the header shows `+N more` beyond this and the preview carries the
/// rest.
pub const MAX_HITS: usize = 3;

/// Build the flat row vector.
///
/// - `blank_query` — the recent-conversations fallback (6eb.5). Headers only: there is no
///   match evidence to show, and density is the only thing worth having in browse mode.
///   `expanded` is ignored in this mode.
/// - otherwise — one header per group, plus up to [`MAX_HITS`] hit rows for each group whose
///   `conv_id` is in `expanded`. A group with no hits contributes only its header.
///
/// Order follows `groups`, and hits follow their header in `Group::hits` order.
pub fn build(groups: &[Group], expanded: &HashSet<String>, blank_query: bool) -> Vec<Row> {
    // Headers alone are the common case — most conversations stay collapsed — so size for
    // that and let the expanded few grow the vector.
    let mut rows = Vec::with_capacity(groups.len());
    for (group, g) in groups.iter().enumerate() {
        rows.push(Row::Header { group });
        // Expansion is keyed by `conv_id`, not by index, for the same reason `reselect` is:
        // the index means nothing once the next search reorders the groups.
        if blank_query || !expanded.contains(&g.conv_id) {
            continue;
        }
        rows.extend((0..g.hits.len().min(MAX_HITS)).map(|hit| Row::Hit { group, hit }));
    }
    rows
}

/// Row index to select after a rebuild, preferring the conversation that was selected
/// before.
///
/// Returns the index of `prev_conv`'s header if it is still present, else `fallback` clamped
/// into range, else 0. Selection follows the *conversation*, not the row number: an index
/// survives a rebuild by accident, a conversation survives it on purpose.
pub fn reselect(rows: &[Row], groups: &[Group], prev_conv: Option<&str>, fallback: usize) -> usize {
    if rows.is_empty() {
        return 0;
    }
    prev_conv
        .and_then(|conv| {
            // Headers only: the conversation may have been expanded before the rebuild and
            // collapsed after (or the other way round), so its hit rows are not a stable
            // landing place even when the conversation itself survived.
            rows.iter().position(|r| {
                r.is_header() && groups.get(r.group()).is_some_and(|g| g.conv_id == conv)
            })
        })
        .unwrap_or_else(|| fallback.min(rows.len() - 1))
}

/// Toggle a conversation's expansion, returning whether it is now expanded.
pub fn toggle(expanded: &mut HashSet<String>, conv_id: &str) -> bool {
    // `remove` borrows the key, so only the collapsed -> expanded direction allocates.
    if expanded.remove(conv_id) {
        false
    } else {
        expanded.insert(conv_id.to_string());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs_core::search::Hit;

    fn hit(conv_id: &str, i: usize) -> Hit {
        Hit {
            conv_id: conv_id.into(),
            msg_id: format!("{conv_id}:m{i}"),
            source: "codex".into(),
            title: None,
            role: "user".into(),
            kind: "prose".into(),
            ts: None,
            score: 0.0,
            snippet: String::new(),
            snippet_spans: Vec::new(),
            resume_cmd: None,
            on_head_path: true,
            is_sidechain: false,
            thread_key: "main".into(),
            deleted_upstream: false,
        }
    }

    /// A group carrying `n` nested hits. Every other field is inert: flattening reads only
    /// `conv_id` and `hits.len()`, and hard-coding the rest keeps the cases readable.
    fn group(conv_id: &str, n: usize) -> Group {
        Group {
            conv_id: conv_id.into(),
            source: "codex".into(),
            title: None,
            cwd: None,
            ended_at: None,
            ended_date: None,
            user_turns: 0,
            msg_count: 0,
            prose_count: 0,
            score: 0.0,
            match_count: n,
            match_seqs: Vec::new(),
            resume_cmd: None,
            deleted_upstream: false,
            hits: (0..n).map(|i| hit(conv_id, i)).collect(),
        }
    }

    fn expanded(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_blank_query_draws_headers_only_whatever_is_expanded() {
        // Browse mode has no match evidence to expand into, so a set left over from the
        // previous query must not leak rows into it (§3, 6eb.5).
        let groups = [group("c:a", 3), group("c:b", 2)];
        let rows = build(&groups, &expanded(&["c:a", "c:b"]), true);
        assert_eq!(rows, [Row::Header { group: 0 }, Row::Header { group: 1 }]);
    }

    #[test]
    fn a_group_with_no_hits_contributes_only_its_header() {
        // `recent` returns groups with empty `hits`, and a hit-less group under a real query
        // is reachable too — expansion must not invent a row to fill the space.
        let groups = [group("c:a", 0)];
        assert_eq!(build(&groups, &expanded(&["c:a"]), false), [Row::Header { group: 0 }]);
    }

    #[test]
    fn hit_rows_stop_at_the_cap() {
        // Uncapped, one verbose conversation fills the viewport; the surplus is the header's
        // `+N more` and the preview's problem.
        let groups = [group("c:a", MAX_HITS + 4)];
        let rows = build(&groups, &expanded(&["c:a"]), false);
        assert_eq!(rows.len(), 1 + MAX_HITS);
        assert_eq!(rows[1], Row::Hit { group: 0, hit: 0 });
        assert_eq!(rows[MAX_HITS], Row::Hit { group: 0, hit: MAX_HITS - 1 });
        // The cap truncates rather than sampling: hits arrive best-first from `search_grouped`.
        assert!(rows.iter().all(|r| r.group() == 0));
    }

    #[test]
    fn expanding_one_conversation_leaves_its_neighbours_collapsed() {
        let groups = [group("c:a", 2), group("c:b", 2), group("c:c", 2)];
        let rows = build(&groups, &expanded(&["c:b"]), false);
        assert_eq!(
            rows,
            [
                Row::Header { group: 0 },
                Row::Header { group: 1 },
                Row::Hit { group: 1, hit: 0 },
                Row::Hit { group: 1, hit: 1 },
                Row::Header { group: 2 },
            ],
            "hits interleave under their own header, not at the end"
        );
    }

    #[test]
    fn an_expanded_id_that_matches_nothing_is_ignored() {
        // Expansion outlives the result set it was set on, so stale ids are normal input.
        let groups = [group("c:a", 2)];
        assert_eq!(build(&groups, &expanded(&["c:gone"]), false), [Row::Header { group: 0 }]);
    }

    #[test]
    fn a_hit_row_acts_on_its_conversation() {
        // Enter on a hit resumes the conversation — no terminal opens at a message — and the
        // `Pick` event records the conversation's rank, so both rows resolve the same way.
        let groups = [group("c:a", 1), group("c:b", 2)];
        let rows = build(&groups, &expanded(&["c:b"]), false);
        assert_eq!(rows[2], Row::Hit { group: 1, hit: 0 });
        assert_eq!(rows[2].group(), 1);
        assert!(!rows[2].is_header());
        assert_eq!(groups[rows[3].group()].conv_id, "c:b");
    }

    #[test]
    fn reselect_follows_the_conversation_when_the_ranking_moves_it() {
        // The point of keying on conv_id: one more keystroke re-ranks the list, and holding
        // row 0 would silently walk the cursor onto a different conversation.
        let before = [group("c:a", 1), group("c:b", 1)];
        let after = [group("c:x", 1), group("c:y", 1), group("c:b", 1)];
        let rows = build(&after, &HashSet::new(), false);
        assert_eq!(reselect(&rows, &after, Some("c:b"), 0), 2);
        assert_eq!(before[0].conv_id, "c:a", "the previous list is only the cursor's origin");
    }

    #[test]
    fn reselect_lands_on_the_header_not_a_hit_row() {
        // A surviving conversation preceded by an expanded one: the naive answer is the row
        // index of the first row mentioning it, which for the collapsed-then-expanded case is
        // still the header, but the guard is what keeps it so.
        let groups = [group("c:a", 3), group("c:b", 3)];
        let rows = build(&groups, &expanded(&["c:a", "c:b"]), false);
        assert_eq!(rows.len(), 2 + 2 * MAX_HITS);
        let at = reselect(&rows, &groups, Some("c:b"), 0);
        assert_eq!(rows[at], Row::Header { group: 1 });
    }

    #[test]
    fn reselect_falls_back_when_the_conversation_dropped_out() {
        let groups = [group("c:a", 1), group("c:b", 1), group("c:c", 1)];
        let rows = build(&groups, &HashSet::new(), false);
        assert_eq!(reselect(&rows, &groups, Some("c:gone"), 1), 1);
        // No previous conversation at all — startup — takes the same path.
        assert_eq!(reselect(&rows, &groups, None, 2), 2);
    }

    #[test]
    fn reselect_clamps_a_fallback_past_the_end() {
        // The fallback is usually the old cursor position, and a narrowing query routinely
        // returns fewer rows than that index.
        let groups = [group("c:a", 1), group("c:b", 1)];
        let rows = build(&groups, &HashSet::new(), false);
        assert_eq!(rows.len(), 2);
        assert_eq!(reselect(&rows, &groups, Some("c:gone"), 99), 1);
        assert_eq!(reselect(&rows, &groups, None, usize::MAX), 1);
    }

    #[test]
    fn reselect_on_no_rows_is_zero_rather_than_a_panic() {
        // A query matching nothing is one keystroke away at all times, and `rows.len() - 1`
        // underflows on it.
        assert_eq!(reselect(&[], &[], Some("c:a"), 7), 0);
        assert_eq!(reselect(&[], &[], None, 0), 0);
    }

    #[test]
    fn toggle_reports_the_state_it_just_set() {
        let mut set = HashSet::new();
        assert!(toggle(&mut set, "c:a"), "first press expands");
        assert!(set.contains("c:a"));
        assert!(!toggle(&mut set, "c:a"), "second press collapses");
        assert!(set.is_empty());
        // Independent per conversation: expansion is not a single selected-row flag.
        toggle(&mut set, "c:a");
        toggle(&mut set, "c:b");
        assert_eq!(set.len(), 2);
        assert!(!toggle(&mut set, "c:a"));
        assert_eq!(set.iter().map(String::as_str).collect::<Vec<_>>(), ["c:b"]);
    }

    #[test]
    fn toggling_then_rebuilding_adds_exactly_that_conversations_hits() {
        // The round trip the space key actually performs.
        let groups = [group("c:a", 5), group("c:b", 1)];
        let mut set = HashSet::new();
        let collapsed = build(&groups, &set, false);
        toggle(&mut set, "c:a");
        let open = build(&groups, &set, false);
        assert_eq!(open.len() - collapsed.len(), MAX_HITS);
        toggle(&mut set, "c:a");
        assert_eq!(build(&groups, &set, false), collapsed);
    }
}
