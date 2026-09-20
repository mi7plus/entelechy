//! Memory tiers and context assembly (PRD 12: MK-1, MK-2, MK-7).
//!
//! Provides the five memory tiers (MK-2), context assembly with a token budget
//! and progressive disclosure (MK-1), and preserves each value's provenance and
//! taint metadata across storage and retrieval (MK-7) by storing the full
//! [`Value`] envelope.

use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_ir::{Taint, Value, ValueMeta};

    fn labeled(data: serde_json::Value, taint: Taint) -> Value {
        Value {
            data,
            meta: ValueMeta { taint, ..Default::default() },
        }
    }

    #[test]
    fn tiers_are_isolated_and_preserve_taint() {
        let mut m = TieredMemory::new();
        m.write(MemoryTier::Working, "k", labeled(serde_json::json!("w"), Taint::Tainted));
        m.write(MemoryTier::Semantic, "k", labeled(serde_json::json!("s"), Taint::Trusted));
        // Same key, different tiers -> distinct entries.
        assert_eq!(m.read(MemoryTier::Working, "k").unwrap().data, serde_json::json!("w"));
        assert_eq!(m.read(MemoryTier::Semantic, "k").unwrap().data, serde_json::json!("s"));
        // Taint (MK-7) is preserved.
        assert_eq!(m.read(MemoryTier::Working, "k").unwrap().meta.taint, Taint::Tainted);
        assert!(m.read(MemoryTier::Episodic, "k").is_none());
    }

    #[test]
    fn context_assembly_respects_budget() {
        let mut m = TieredMemory::new();
        // Each ~10-char payload ≈ 3-4 tokens.
        for i in 0..10 {
            m.write(MemoryTier::Episodic, format!("k{i}"), Value::trusted(serde_json::json!(format!("item-{i:04}"))));
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
