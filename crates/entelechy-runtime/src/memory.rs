//! Memory tiers and context assembly (PRD 12: MK-1, MK-2, MK-7).
//!
//! Provides the five memory tiers (MK-2), context assembly with a token budget
//! and progressive disclosure (MK-1), and preserves each value's provenance and
//! taint metadata across storage and retrieval (MK-7) by storing the full
//! [`Value`] envelope.

use std::collections::{BTreeMap, BTreeSet};

use entelechy_ir::{MemoryTier, Value};

/// A tiered key-value memory (PRD 12, MK-2). Each entry keeps its full value
/// envelope, so provenance and taint survive read/write (MK-7).
#[derive(Default)]
pub struct TieredMemory {
    entries: BTreeMap<(MemoryTier, String), Value>,
}

impl TieredMemory {
    /// Create an empty tiered memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a value to a tier under a key (MK-7: the envelope is preserved).
    pub fn write(&mut self, tier: MemoryTier, key: impl Into<String>, value: Value) {
        self.entries.insert((tier, key.into()), value);
    }

    /// Read a value from a tier.
    pub fn read(&self, tier: MemoryTier, key: &str) -> Option<&Value> {
        self.entries.get(&(tier, key.to_string()))
    }

    /// Number of entries across all tiers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the memory is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Retrieve the `top_k` entries of `tier` most relevant to `query`, ranked by
    /// lexical token overlap (PRD 12, 11.4 level 2). This is the deterministic
    /// in-process knowledge source a [`entelechy_ir::RetrievalNode`] reads:
    ///
    /// - Relevance is the number of distinct query tokens the entry contains;
    ///   entries with no overlap are omitted (an honest empty result when nothing
    ///   matches, rather than padding with noise).
    /// - Ties break by key ascending, so the order is stable and replay-exact
    ///   (RK-2) — no map iteration order or scoring float leaks in.
    /// - Returned values keep their full envelope, so provenance, taint and
    ///   confidentiality survive retrieval (MK-7).
    ///
    /// Being deterministic and in-process, retrieval is not a journaled effect:
    /// the same memory and query always yield the same result, so replay and
    /// counterfactual re-execution reproduce it without a recorded event.
    pub fn retrieve(&self, tier: MemoryTier, query: &str, top_k: u32) -> Vec<Value> {
        let q = tokenize(query);
        if q.is_empty() || top_k == 0 {
            return Vec::new();
        }
        // (score, key, value): BTreeMap iteration is already (tier, key) ascending,
        // so pushing in order and using a stable sort keeps ties in key order.
        let mut scored: Vec<(usize, &str, &Value)> = Vec::new();
        for ((t, key), value) in &self.entries {
            if *t != tier {
                continue;
            }
            let doc = tokenize(&value.data.to_string());
            let score = q.iter().filter(|tok| doc.contains(*tok)).count();
            if score > 0 {
                scored.push((score, key.as_str(), value));
            }
        }
        // Highest score first; equal scores stay in key-ascending order (stable
        // sort over key-ordered input).
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored
            .into_iter()
            .take(top_k as usize)
            .map(|(_, _, v)| v.clone())
            .collect()
    }

    /// Assemble a context from the given tiers within a token budget (MK-1).
    /// Greedy progressive disclosure: entries are included in tier/key order until
    /// the budget is reached; entries that do not fit are skipped rather than
    /// truncated, so no value is silently corrupted. Returned values keep their
    /// provenance and taint (MK-7).
    pub fn assemble_context(&self, tiers: &[MemoryTier], budget_tokens: usize) -> Vec<Value> {
        let mut out = Vec::new();
        let mut used = 0usize;
        for ((tier, _key), value) in &self.entries {
            if !tiers.contains(tier) {
                continue;
            }
            let cost = estimate_tokens(value);
            if used + cost <= budget_tokens {
                out.push(value.clone());
                used += cost;
            }
        }
        out
    }
}

/// A cheap, deterministic token estimate for budgeting (~4 chars/token).
pub fn estimate_tokens(value: &Value) -> usize {
    value.data.to_string().len() / 4 + 1
}

/// Lowercase, alphanumeric-delimited token set used for lexical retrieval
/// scoring. Deterministic and allocation-light; a set, so repeated words don't
/// inflate overlap.
fn tokenize(s: &str) -> BTreeSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_ir::{Taint, Value, ValueMeta};

    fn labeled(data: serde_json::Value, taint: Taint) -> Value {
        Value {
            data,
            meta: ValueMeta {
                taint,
                ..Default::default()
            },
        }
    }

    #[test]
    fn tiers_are_isolated_and_preserve_taint() {
        let mut m = TieredMemory::new();
        m.write(
            MemoryTier::Working,
            "k",
            labeled(serde_json::json!("w"), Taint::Tainted),
        );
        m.write(
            MemoryTier::Semantic,
            "k",
            labeled(serde_json::json!("s"), Taint::Trusted),
        );
        // Same key, different tiers -> distinct entries.
        assert_eq!(
            m.read(MemoryTier::Working, "k").unwrap().data,
            serde_json::json!("w")
        );
        assert_eq!(
            m.read(MemoryTier::Semantic, "k").unwrap().data,
            serde_json::json!("s")
        );
        // Taint (MK-7) is preserved.
        assert_eq!(
            m.read(MemoryTier::Working, "k").unwrap().meta.taint,
            Taint::Tainted
        );
        assert!(m.read(MemoryTier::Episodic, "k").is_none());
    }

    #[test]
    fn retrieve_ranks_by_overlap_and_is_stable() {
        let mut m = TieredMemory::new();
        m.write(
            MemoryTier::Semantic,
            "a",
            Value::trusted(serde_json::json!("reset your password from the login page")),
        );
        m.write(
            MemoryTier::Semantic,
            "b",
            Value::trusted(serde_json::json!(
                "password reset requires email verification"
            )),
        );
        m.write(
            MemoryTier::Semantic,
            "c",
            Value::trusted(serde_json::json!("our refund policy covers 30 days")),
        );

        // Query overlaps "a" and "b" on {password, reset}; "c" not at all.
        let hits = m.retrieve(MemoryTier::Semantic, "how do I reset my password", 10);
        assert_eq!(hits.len(), 2, "only the two matching entries are returned");
        assert!(hits.iter().all(|v| v
            .data
            .as_str()
            .map(|s| s.contains("password"))
            .unwrap_or(false)));

        // top_k bounds the result.
        let one = m.retrieve(MemoryTier::Semantic, "password reset", 1);
        assert_eq!(one.len(), 1);

        // A different tier contributes nothing (isolation).
        assert!(m
            .retrieve(MemoryTier::Episodic, "password reset", 10)
            .is_empty());

        // An empty query (or no overlap) yields an honest empty result.
        assert!(m.retrieve(MemoryTier::Semantic, "", 10).is_empty());
        assert!(m
            .retrieve(MemoryTier::Semantic, "quantum chromodynamics", 10)
            .is_empty());
    }

    #[test]
    fn retrieve_preserves_taint_and_confidentiality() {
        // MK-7: the retrieved value keeps its full envelope.
        let mut m = TieredMemory::new();
        let mut secret = labeled(
            serde_json::json!("the vault code is swordfish"),
            Taint::Tainted,
        );
        secret.meta.confidentiality.insert("secret".into());
        m.write(MemoryTier::Semantic, "k", secret);

        let hits = m.retrieve(MemoryTier::Semantic, "vault code", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].meta.taint, Taint::Tainted);
        assert!(hits[0].meta.confidentiality.contains("secret"));
    }

    #[test]
    fn context_assembly_respects_budget() {
        let mut m = TieredMemory::new();
        // Each ~10-char payload ≈ 3-4 tokens.
        for i in 0..10 {
            m.write(
                MemoryTier::Episodic,
                format!("k{i}"),
                Value::trusted(serde_json::json!(format!("item-{i:04}"))),
            );
        }
        let all = m.assemble_context(&[MemoryTier::Episodic], 1_000);
        assert_eq!(all.len(), 10);
        // A tight budget includes only some entries (progressive disclosure).
        let few = m.assemble_context(&[MemoryTier::Episodic], 5);
        assert!(few.len() < 10 && !few.is_empty(), "got {}", few.len());
        // A tier not requested contributes nothing.
        assert!(m.assemble_context(&[MemoryTier::Working], 1_000).is_empty());
    }
}
