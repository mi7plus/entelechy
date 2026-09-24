//! Contamination detection across splits (PRD 9.5, Q18).
//!
//! Detection combines normalized-text hashing, n-gram Jaccard and embedding
//! similarity (Q18). Thresholds (recalibrated on the Phase 0 benchmark):
//! - Jaccard ≥ 0.8 **or** cosine ≥ 0.92 → **blocked** from crossing splits.
//! - cosine in [0.85, 0.92) → **flagged** for human review.
//! - otherwise → **clear**.
//!
//! Embeddings are supplied by the caller (an `entelechy-capability`/model
//! concern); this crate computes the text-only signals itself and folds in a
//! cosine value when provided.

use std::collections::HashSet;

/// Q18 thresholds.
const JACCARD_BLOCK: f64 = 0.8;
const COSINE_BLOCK: f64 = 0.92;
const COSINE_FLAG_LOW: f64 = 0.85;

/// The verdict for a candidate task relative to an existing one (Q18).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contamination {
    /// Safe to place in a different split.
    Clear,
    /// Borderline; requires human review before crossing splits.
    Flagged,
    /// Must not cross splits (near-duplicate).
    Blocked,
}

/// Normalize text for hashing/similarity: lowercase, collapse whitespace.
pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A stable normalized-text hash (FNV-1a) for exact-duplicate detection.
pub fn normalized_hash(text: &str) -> u64 {
    let norm = normalize(text);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in norm.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    h
}

/// Word n-gram Jaccard similarity in `[0,1]`.
pub fn ngram_jaccard(a: &str, b: &str, n: usize) -> f64 {
    let ga = ngrams(a, n);
    let gb = ngrams(b, n);
    if ga.is_empty() && gb.is_empty() {
        return 1.0;
    }
    let inter = ga.intersection(&gb).count() as f64;
    let union = ga.union(&gb).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

fn ngrams(text: &str, n: usize) -> HashSet<String> {
    let words: Vec<String> = normalize(text).split(' ').map(|s| s.to_string()).collect();
    let n = n.max(1);
    if words.len() < n {
        // Fall back to the whole (normalized) string as a single gram.
        let joined = words.join(" ");
        return if joined.is_empty() {
            HashSet::new()
        } else {
            HashSet::from([joined])
        };
    }
    words.windows(n).map(|w| w.join(" ")).collect()
}

/// Cosine similarity of two equal-length embedding vectors.
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        (dot / (na * nb)).clamp(-1.0, 1.0)
    }
}

/// Assess contamination between two task texts, optionally using embeddings
/// (Q18). Uses word trigrams for the Jaccard signal.
pub fn assess(a: &str, b: &str, embeddings: Option<(&[f64], &[f64])>) -> Contamination {
    // Exact normalized duplicate → blocked.
    if normalized_hash(a) == normalized_hash(b) {
        return Contamination::Blocked;
    }
    let jac = ngram_jaccard(a, b, 3);
    let cos = embeddings.map(|(x, y)| cosine(x, y));

    if jac >= JACCARD_BLOCK || cos.is_some_and(|c| c >= COSINE_BLOCK) {
        return Contamination::Blocked;
    }
    if cos.is_some_and(|c| (COSINE_FLAG_LOW..COSINE_BLOCK).contains(&c)) {
        return Contamination::Flagged;
    }
    Contamination::Clear
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_duplicate_is_blocked() {
        assert_eq!(
            assess("Refund my order please", "refund my   order please", None),
            Contamination::Blocked
        );
    }

    #[test]
    fn paraphrase_high_jaccard_is_blocked() {
        // 13-word sentences differing only in the final word: word-trigram
        // Jaccard = 10/12 ≈ 0.83, above the 0.8 block threshold.
        let a = "the customer wants a full refund for a broken item received last tuesday";
        let b = "the customer wants a full refund for a broken item received last friday";
        assert!(
            ngram_jaccard(a, b, 3) >= 0.8,
            "jac={}",
            ngram_jaccard(a, b, 3)
        );
        assert_eq!(assess(a, b, None), Contamination::Blocked);
    }

    #[test]
    fn unrelated_text_is_clear() {
        assert_eq!(
            assess("reset my password", "where is my invoice pdf", None),
            Contamination::Clear
        );
    }

    #[test]
    fn cosine_band_flags() {
        // Low text overlap but embeddings say ~0.88 → flagged.
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.88, 0.475, 0.0]; // cos ~0.88
        let c = cosine(&a, &b);
        assert!((COSINE_FLAG_LOW..COSINE_BLOCK).contains(&c), "cos={c}");
        assert_eq!(
            assess("alpha bravo charlie", "delta echo foxtrot", Some((&a, &b))),
            Contamination::Flagged
        );
    }
}
