//! Whether the ranking is any good, as a number.
//!
//! Everything else in the index is tuning with no target: `REPEAT_WEIGHT` and [`DECAY`] can
//! be moved in either direction and nothing says which way was better. This module is the
//! target — graded relevance judgements folded into metrics, so a change to the ranking is
//! an experiment rather than a preference.
//!
//! Only the arithmetic lives here. Judgements are a client concern: where the query set is
//! stored, how a human is prompted, and how answers are persisted all belong to whichever
//! client is doing the asking, and none of it should be linkable into a search path.
//!
//! [`DECAY`]: crate::search::DECAY

use serde::Serialize;
use std::collections::HashMap;

/// How relevant one conversation is to one query, as a human judged it.
///
/// Graded rather than binary because "is this the one I meant" and "would I have been happy
/// to see this" are different questions, and a ranking that puts a [`Grade::Related`] first
/// is not failing the way one that puts a [`Grade::Irrelevant`] first is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(into = "u8")]
pub enum Grade {
    /// Nothing to do with the query.
    Irrelevant = 0,
    /// Shares the subject but would not have answered the question.
    Related = 1,
    /// Would have been a useful answer, though not the one in mind.
    Useful = 2,
    /// This is the conversation the query was reaching for.
    Exact = 3,
}

impl Grade {
    pub const MAX: Grade = Grade::Exact;

    /// The threshold a result has to clear to count as a hit rather than noise.
    ///
    /// [`Grade::Related`] sits below it deliberately: a search that returns things merely
    /// *about* the right subject, and never the conversation itself, has failed at the job
    /// even though every result looks defensible.
    pub const USEFUL: Grade = Grade::Useful;

    pub fn from_u8(n: u8) -> Option<Grade> {
        match n {
            0 => Some(Grade::Irrelevant),
            1 => Some(Grade::Related),
            2 => Some(Grade::Useful),
            3 => Some(Grade::Exact),
            _ => None,
        }
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Whether a result at this grade counts toward MRR and success@k.
    pub fn is_hit(self) -> bool {
        self >= Grade::USEFUL
    }

    /// The single word for this grade, for prompts too narrow to carry [`Grade::label`].
    pub fn name(self) -> &'static str {
        match self {
            Grade::Irrelevant => "irrelevant",
            Grade::Related => "related",
            Grade::Useful => "useful",
            Grade::Exact => "exact",
        }
    }

    /// One-line gloss, shown next to the number wherever a human picks one.
    pub fn label(self) -> &'static str {
        match self {
            Grade::Irrelevant => "irrelevant — nothing to do with the query",
            Grade::Related => "related — same subject, would not have answered it",
            Grade::Useful => "useful — would have been a good answer",
            Grade::Exact => "exact — this is the one the query was reaching for",
        }
    }
}

impl From<Grade> for u8 {
    fn from(g: Grade) -> u8 {
        g.as_u8()
    }
}

/// Discounted cumulative gain over a ranked list of grades.
///
/// Exponential gain (`2^g - 1`) rather than linear, so the distance from *useful* to *exact*
/// is worth more than the distance from *irrelevant* to *related*. That matches what the
/// tool is for: surfacing the conversation someone meant, not a plausible neighbour.
pub fn dcg(ranked: &[Grade]) -> f64 {
    ranked
        .iter()
        .enumerate()
        .map(|(i, g)| (2f64.powi(g.as_u8() as i32) - 1.0) / ((i + 2) as f64).log2())
        .sum()
}

/// nDCG@k: the ranking's gain as a fraction of the best gain those judgements allow.
///
/// `None` when no positive judgement exists for the query, which is not a score of zero —
/// it means the query cannot be scored at all, and averaging a zero in would quietly punish
/// the ranking for a gap in the judgements. Callers report those separately.
pub fn ndcg_at(ranked: &[Grade], all_known: &[Grade], k: usize) -> Option<f64> {
    let actual = dcg(&ranked[..k.min(ranked.len())]);
    let mut ideal: Vec<Grade> = all_known.to_vec();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    ideal.truncate(k);
    let best = dcg(&ideal);
    (best > 0.0).then(|| actual / best)
}

/// Everything known about one query in the set.
pub struct Judged<'a> {
    pub id: &'a str,
    pub text: &'a str,
    /// Grades for this query, by conversation id. Conversations absent from the map are
    /// unjudged, which is not the same as [`Grade::Irrelevant`] — see [`QueryScore::unjudged`].
    pub grades: &'a HashMap<String, Grade>,
    /// Conversation ids the ranking returned, best first.
    pub returned: &'a [String],
}

/// How one query scored.
#[derive(Debug, Clone, Serialize)]
pub struct QueryScore {
    pub id: String,
    pub query: String,
    /// `None` when every known judgement for this query is [`Grade::Irrelevant`], so there
    /// is no better order to compare against and the query cannot be scored.
    pub ndcg: Option<f64>,
    /// `1/rank` of the first result at [`Grade::USEFUL`] or better; `None` if there is none
    /// in the returned list.
    pub reciprocal_rank: Option<f64>,
    /// 1-based rank of that first hit.
    pub first_hit_rank: Option<usize>,
    /// Whether rank 1 is the conversation the query was reaching for.
    pub exact_at_1: bool,
    pub hit_at_1: bool,
    pub hit_at_5: bool,
    /// Returned inside the scored depth with no judgement. High counts mean the score is
    /// built on an incomplete pool and should not be trusted (see [`Report::coverage`]).
    pub unjudged: usize,
    pub returned: usize,
    /// Judged conversations for this query that the ranking never returned at all. A recall
    /// failure rather than a ranking one, and the two want opposite fixes.
    pub missed: Vec<String>,
}

pub fn score_query(j: &Judged, k: usize) -> QueryScore {
    let depth = k.min(j.returned.len());
    let ranked: Vec<Grade> = j.returned[..depth]
        .iter()
        .map(|id| j.grades.get(id).copied().unwrap_or(Grade::Irrelevant))
        .collect();
    let unjudged = j.returned[..depth].iter().filter(|id| !j.grades.contains_key(*id)).count();

    let all_known: Vec<Grade> = j.grades.values().copied().collect();
    let first_hit = ranked.iter().position(|g| g.is_hit());

    let returned_set: std::collections::HashSet<&str> =
        j.returned.iter().map(String::as_str).collect();
    let mut missed: Vec<String> = j
        .grades
        .iter()
        .filter(|(id, g)| g.is_hit() && !returned_set.contains(id.as_str()))
        .map(|(id, _)| id.clone())
        .collect();
    missed.sort();

    QueryScore {
        id: j.id.to_string(),
        query: j.text.to_string(),
        ndcg: ndcg_at(&ranked, &all_known, k),
        reciprocal_rank: first_hit.map(|i| 1.0 / (i + 1) as f64),
        first_hit_rank: first_hit.map(|i| i + 1),
        exact_at_1: ranked.first() == Some(&Grade::Exact),
        hit_at_1: ranked.first().is_some_and(|g| g.is_hit()),
        hit_at_5: ranked.iter().take(5).any(|g| g.is_hit()),
        unjudged,
        returned: j.returned.len(),
        missed,
    }
}

/// The set's verdict.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub depth: usize,
    /// Mean nDCG@`depth` over queries that could be scored.
    pub ndcg: f64,
    /// Mean reciprocal rank. Queries with no hit in the returned list contribute 0, which is
    /// the right answer here — the ranking genuinely failed to surface anything useful.
    pub mrr: f64,
    pub success_at_1: f64,
    pub success_at_5: f64,
    pub exact_at_1: f64,
    pub scored: usize,
    /// Queries whose judgements are all [`Grade::Irrelevant`], excluded from `ndcg`. Judge
    /// these before reading anything into the score.
    pub unscorable: usize,
    /// Fraction of the returned results inside `depth` that carry a judgement. Below ~0.8
    /// the pool is too thin for the numbers to mean much.
    pub coverage: f64,
    pub queries: Vec<QueryScore>,
}

pub fn report(scores: Vec<QueryScore>, depth: usize) -> Report {
    let n = scores.len().max(1) as f64;
    let scorable: Vec<f64> = scores.iter().filter_map(|s| s.ndcg).collect();
    let mean = |xs: &[f64]| if xs.is_empty() { 0.0 } else { xs.iter().sum::<f64>() / xs.len() as f64 };

    let shown: usize = scores.iter().map(|s| s.returned.min(depth)).sum();
    let unjudged: usize = scores.iter().map(|s| s.unjudged).sum();

    Report {
        depth,
        ndcg: mean(&scorable),
        mrr: scores.iter().map(|s| s.reciprocal_rank.unwrap_or(0.0)).sum::<f64>() / n,
        success_at_1: scores.iter().filter(|s| s.hit_at_1).count() as f64 / n,
        success_at_5: scores.iter().filter(|s| s.hit_at_5).count() as f64 / n,
        exact_at_1: scores.iter().filter(|s| s.exact_at_1).count() as f64 / n,
        scored: scorable.len(),
        unscorable: scores.len() - scorable.len(),
        coverage: if shown == 0 { 1.0 } else { 1.0 - unjudged as f64 / shown as f64 },
        queries: scores,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Grade::*;

    fn grades(pairs: &[(&str, Grade)]) -> HashMap<String, Grade> {
        pairs.iter().map(|(id, g)| (id.to_string(), *g)).collect()
    }

    fn ids(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_perfect_ranking_scores_one_and_a_reversed_one_scores_less() {
        let g = grades(&[("a", Exact), ("b", Useful), ("c", Irrelevant)]);
        let perfect = ids(&["a", "b", "c"]);
        let reversed = ids(&["c", "b", "a"]);

        let s = |returned: &Vec<String>| {
            score_query(&Judged { id: "q", text: "q", grades: &g, returned }, 10).ndcg.unwrap()
        };
        assert!((s(&perfect) - 1.0).abs() < 1e-9, "best possible order is 1.0");
        assert!(s(&reversed) < s(&perfect));
        assert!(s(&reversed) > 0.0, "the relevant ones are still in the list, just late");
    }

    #[test]
    fn misplacing_the_exact_conversation_costs_more_gain_than_misordering_two_mediocre_ones() {
        // What the exponential gain buys, stated in the unit it applies to. Linear gain
        // would make both swaps cost the same amount of DCG; they are not worth the same to
        // someone looking for one particular conversation.
        //
        // Deliberately measured on raw DCG rather than nDCG. nDCG divides by the ideal, so
        // the *normalised* cost of a swap is actually larger among low grades — a small
        // ideal makes every mistake a bigger fraction of it. That is a property of the
        // normalisation, not of the gain function, and asserting it here would pin the
        // wrong thing.
        let exact_swap = dcg(&[Exact, Useful]) - dcg(&[Useful, Exact]);
        let mediocre_swap = dcg(&[Useful, Related]) - dcg(&[Related, Useful]);
        assert!(exact_swap > mediocre_swap, "{exact_swap} should exceed {mediocre_swap}");

        // The same thing at the source: one step up near the top is worth more than one
        // step up near the bottom.
        let gain = |g: Grade| 2f64.powi(g.as_u8() as i32) - 1.0;
        assert!(gain(Exact) - gain(Useful) > gain(Useful) - gain(Related));
    }

    #[test]
    fn a_query_with_nothing_positive_known_is_unscorable_rather_than_zero() {
        // Averaging a 0 in here would blame the ranking for a hole in the judgements, and
        // the score would sink every time a new unjudged query was added to the set.
        let g = grades(&[("a", Irrelevant), ("b", Irrelevant)]);
        let s = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["a", "b"]) }, 10,
        );
        assert_eq!(s.ndcg, None);
        assert_eq!(s.reciprocal_rank, None, "nothing at or above Useful was returned");

        let r = report(vec![s], 10);
        assert_eq!(r.unscorable, 1);
        assert_eq!(r.scored, 0);
        assert_eq!(r.ndcg, 0.0, "no scorable query means no mean to report");
    }

    #[test]
    fn a_merely_related_judgement_is_still_enough_to_order_against() {
        // nDCG needs a better-and-worse to compare, not a hit: if one result is known to be
        // more on-subject than another, putting the wrong one first is measurable. MRR and
        // success@k are what refuse to give credit here, and they are why the two are
        // reported side by side rather than one standing in for the other.
        let g = grades(&[("a", Related), ("b", Irrelevant)]);
        let right = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["a", "b"]) }, 10,
        );
        let wrong = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["b", "a"]) }, 10,
        );
        assert_eq!(right.ndcg, Some(1.0));
        assert!(wrong.ndcg.unwrap() < 1.0);
        assert!(!right.hit_at_5, "related is not a hit however well it is ranked");
        assert_eq!(report(vec![right], 10).success_at_5, 0.0);
    }

    #[test]
    fn related_does_not_count_as_a_hit() {
        // A search that reliably returns things about the right subject, and never the
        // conversation itself, looks defensible result by result and has still failed.
        let g = grades(&[("a", Related), ("b", Useful)]);
        let s = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["a", "b"]) }, 10,
        );
        assert!(!s.hit_at_1);
        assert!(s.hit_at_5);
        assert_eq!(s.first_hit_rank, Some(2));
        assert_eq!(s.reciprocal_rank, Some(0.5));
    }

    #[test]
    fn an_unjudged_result_is_counted_as_irrelevant_but_reported_as_a_gap() {
        // It has to score as zero — there is nothing else to score it as — but a set where
        // most of the top ten is unjudged is measuring the judgements, not the ranking.
        let g = grades(&[("a", Exact)]);
        let s = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["x", "y", "a"]) }, 10,
        );
        assert_eq!(s.unjudged, 2);
        assert!(s.ndcg.unwrap() < 1.0, "the exact hit sat at rank 3");

        let r = report(vec![s], 10);
        assert!((r.coverage - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_relevant_conversation_never_returned_is_a_recall_failure_not_a_ranking_one() {
        let g = grades(&[("a", Exact), ("gone", Useful)]);
        let s = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["a"]) }, 10,
        );
        assert_eq!(s.missed, vec!["gone"]);
        assert!(s.ndcg.unwrap() < 1.0, "the ideal list still contains the one that never came back");
    }

    #[test]
    fn depth_truncates_the_ideal_list_too() {
        // Otherwise nDCG@1 could never reach 1.0 for a query with two relevant results:
        // the ranking would be measured against an ideal it was never allowed to return.
        let g = grades(&[("a", Exact), ("b", Exact)]);
        let s = score_query(
            &Judged { id: "q", text: "q", grades: &g, returned: &ids(&["a", "b"]) }, 1,
        );
        assert!((s.ndcg.unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn an_empty_result_list_scores_zero_rather_than_panicking() {
        let g = grades(&[("a", Exact)]);
        let s = score_query(&Judged { id: "q", text: "q", grades: &g, returned: &[] }, 10);
        assert_eq!(s.ndcg, Some(0.0));
        assert_eq!(s.reciprocal_rank, None);
        assert_eq!(s.missed, vec!["a"]);

        let r = report(vec![s], 10);
        assert_eq!(r.mrr, 0.0);
        assert_eq!(r.coverage, 1.0, "nothing was shown, so nothing is unjudged");
    }

    #[test]
    fn mrr_counts_a_total_miss_as_zero_while_ndcg_declines_to_score_it() {
        // The two are answering different questions. MRR asks "how far down before
        // something useful", and "never" is a real, bad answer. nDCG asks "how close to the
        // best achievable order", which is undefined when nothing good is known to exist.
        let hit = score_query(
            &Judged {
                id: "a", text: "a",
                grades: &grades(&[("x", Exact)]),
                returned: &ids(&["x"]),
            }, 10,
        );
        let miss = score_query(
            &Judged {
                id: "b", text: "b",
                grades: &grades(&[("y", Exact)]),
                returned: &ids(&["z"]),
            }, 10,
        );
        let r = report(vec![hit, miss], 10);
        assert!((r.mrr - 0.5).abs() < 1e-9, "one perfect, one nothing");
        assert_eq!(r.scored, 2, "both had a known relevant conversation");
        assert!((r.success_at_1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn grades_round_trip_through_the_number_a_human_types() {
        for g in [Irrelevant, Related, Useful, Exact] {
            assert_eq!(Grade::from_u8(g.as_u8()), Some(g));
        }
        assert_eq!(Grade::from_u8(4), None);
        assert!(Grade::Exact > Grade::Useful && Grade::Useful > Grade::Related);
    }
}
