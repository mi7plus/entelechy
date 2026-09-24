//! LLM-free retrieval metrics (PRD MK-4).
//!
//! "Optimize retrieval first with LLM-free metrics such as recall@k, MRR and
//! nDCG" (PRD 12, MK-4). These are cheap, deterministic proxies used before any
//! model-in-the-loop evaluation, so retrieval configuration can be tuned without
//! spending model budget.
//!
//! Each function takes a ranked list of retrieved document ids (best first) and
//! the set of relevant ids for the query.

use std::collections::BTreeSet;

/// Recall@k: fraction of the relevant documents that appear in the top `k`
/// (PRD MK-4). Returns 1.0 when there are no relevant documents (nothing to miss).
pub fn recall_at_k(ranked: &[String], relevant: &BTreeSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 1.0;
    }
    let hits = ranked
        .iter()
        .take(k)
        .filter(|id| relevant.contains(*id))
        .count();
    hits as f64 / relevant.len() as f64
}

/// Precision@k: fraction of the top `k` that are relevant (PRD MK-4).
pub fn precision_at_k(ranked: &[String], relevant: &BTreeSet<String>, k: usize) -> f64 {
    if k == 0 {
        return 0.0;
    }
    let denom = k.min(ranked.len().max(1));
    let hits = ranked
        .iter()
        .take(k)
        .filter(|id| relevant.contains(*id))
        .count();
    hits as f64 / denom as f64
}

/// Mean reciprocal rank contribution for a single query: `1 / rank` of the first
/// relevant result (rank is 1-based), or 0 if none is retrieved (PRD MK-4).
pub fn reciprocal_rank(ranked: &[String], relevant: &BTreeSet<String>) -> f64 {
    for (i, id) in ranked.iter().enumerate() {
        if relevant.contains(id) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Mean reciprocal rank over many queries (PRD MK-4).
pub fn mean_reciprocal_rank(queries: &[(Vec<String>, BTreeSet<String>)]) -> f64 {
    if queries.is_empty() {
        return 0.0;
    }
    let sum: f64 = queries
        .iter()
        .map(|(ranked, relevant)| reciprocal_rank(ranked, relevant))
        .sum();
    sum / queries.len() as f64
}

/// nDCG@k with binary relevance (gain 1 for a relevant document), using the
/// standard `1 / log2(rank + 1)` discount (PRD MK-4). Returns 1.0 when there are
/// no relevant documents.
pub fn ndcg_at_k(ranked: &[String], relevant: &BTreeSet<String>, k: usize) -> f64 {
    if relevant.is_empty() {
        return 1.0;
    }
    let dcg: f64 = ranked
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, id)| {
            if relevant.contains(id) {
                1.0 / ((i as f64 + 2.0).log2())
            } else {
                0.0
            }
        })
        .sum();
    // Ideal DCG: all relevant docs ranked first, up to k.
    let ideal_hits = relevant.len().min(k);
    let idcg: f64 = (0..ideal_hits)
        .map(|i| 1.0 / ((i as f64 + 2.0).log2()))
        .sum();
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn ranked(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn recall_and_precision() {
        let r = ranked(&["a", "x", "b", "y"]);
        let rel = set(&["a", "b", "c"]);
        // top-4 contains a and b -> recall 2/3.
        assert!((recall_at_k(&r, &rel, 4) - 2.0 / 3.0).abs() < 1e-9);
        // top-2 has 1 relevant (a) -> precision 1/2.
        assert!((precision_at_k(&r, &rel, 2) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn reciprocal_rank_finds_first_relevant() {
        assert_eq!(
            reciprocal_rank(&ranked(&["x", "a", "b"]), &set(&["a"])),
            0.5
        );
        assert_eq!(reciprocal_rank(&ranked(&["a"]), &set(&["a"])), 1.0);
        assert_eq!(reciprocal_rank(&ranked(&["x", "y"]), &set(&["a"])), 0.0);
    }

    #[test]
    fn mrr_averages() {
        let queries = vec![
            (ranked(&["a", "x"]), set(&["a"])), // rr 1.0
            (ranked(&["x", "b"]), set(&["b"])), // rr 0.5
        ];
        assert!((mean_reciprocal_rank(&queries) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn ndcg_perfect_and_imperfect() {
        let rel = set(&["a", "b"]);
        // Perfect ranking -> 1.0.
        assert!((ndcg_at_k(&ranked(&["a", "b", "x"]), &rel, 3) - 1.0).abs() < 1e-9);
        // Relevant docs lower down -> < 1.0 but > 0.
        let n = ndcg_at_k(&ranked(&["x", "a", "b"]), &rel, 3);
        assert!(n < 1.0 && n > 0.0, "ndcg={n}");
    }

    #[test]
    fn empty_relevant_is_defined() {
        let rel = set(&[]);
        assert_eq!(recall_at_k(&ranked(&["a"]), &rel, 1), 1.0);
        assert_eq!(ndcg_at_k(&ranked(&["a"]), &rel, 1), 1.0);
    }
}
