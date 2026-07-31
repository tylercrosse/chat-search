//! Throwaway: does the new parser rank identically to the code it replaces?
use cs_core::query::Query;
use cs_core::search::to_match_expr_opts;

#[test]
fn new_parser_matches_the_old_expression_for_every_non_filter_query() {
    let corpus = [
        "borrow", "borrow ", "le", "lea", "deep le", "deep learn", "learn deep learn",
        "rust async", "a-b_c", "say \"hi\"", "*", "-", "??", "", "   ",
        "Borrow Checker", "the", "emb", "embe", "embed", "embedd", "rus", "rust",
        "café", "naïve résumé", "x", "xy", "xyz", "a b c d e", "trailing  spaces  ",
        "under_score", "123", "1 2", "-borrow", "--borrow", "a.b.c", "foo/bar",
    ];
    let mut diffs = vec![];
    for q in corpus {
        for prefix in [true, false] {
            let old = to_match_expr_opts(&q.to_lowercase(), prefix);
            let new = if prefix { Query::typeahead(q) } else { Query::exact(q) }.match_expr();
            if old != new {
                diffs.push(format!("  {q:?} prefix={prefix}\n    old={old}\n    new={new}"));
            }
        }
    }
    assert!(diffs.is_empty(), "{} divergences:\n{}", diffs.len(), diffs.join("\n"));
}

/// `App::holds` as it stands in cs-tui today, replicated here because that crate depends on
/// this one and cannot be imported from its tests.
fn old_holds(query: &str) -> bool {
    let trimmed = query.trim();
    let typed = trimmed.chars().count();
    let lone = !trimmed.contains(char::is_whitespace);
    lone && typed > 0 && typed < cs_core::search::MIN_PREFIX_LEN
}

#[test]
fn mode_reproduces_is_blank_and_holds_exactly() {
    let corpus = [
        "", "   ", "-", "??", "  ..  ", "a", "ab", "abc", "ab ", "a-", "a-b", "a b",
        "le", "lea", "deep le", "borrow", "x", "xy", "123", "1 2", "_", "__",
    ];
    let mut diffs = vec![];
    for q in corpus {
        let query = Query::typeahead(q);
        let blank = cs_core::search::is_blank(q);
        let holds = old_holds(q);
        // is_blank wins where both fire: "??" is held by the old rule but has no terms at all.
        let expected = if blank {
            cs_core::Mode::Empty
        } else if holds {
            cs_core::Mode::TooShort
        } else {
            cs_core::Mode::Searchable
        };
        if query.mode() != expected {
            diffs.push(format!("  {q:?} blank={blank} holds={holds} -> {:?}, want {expected:?}", query.mode()));
        }
    }
    assert!(diffs.is_empty(), "{} divergences:\n{}", diffs.len(), diffs.join("\n"));
}
