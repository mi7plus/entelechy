//! Tamper-evident audit log (PRD 17.2).
//!
//! "The audit log is append-only and tamper-evident (hash-chained, with periodic
//! anchoring outside the operator's control), so approvals and policy decisions
//! cannot be rewritten after the fact" (PRD 17.2). Each entry's hash chains in the
//! previous entry's hash, so altering any past entry breaks every later hash
//! (detected by [`AuditLog::verify_chain`]). Periodic external anchors let a
//! holder detect a full rewrite even if the operator recomputes the chain
//! ([`AuditLog::verify_anchor`]).

use serde::{Deserialize, Serialize};

use entelechy_artifacts::ArtifactId;

/// One append-only audit entry (PRD 17.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Monotonic sequence number.
    pub seq: u64,
    /// Unix-seconds timestamp.
    pub timestamp: u64,
    /// The principal responsible (PRD 5.8).
    pub principal: String,
    /// The action recorded (e.g. an approval or policy decision).
    pub action: String,
    /// Hash of the previous entry (chain link).
    pub prev_hash: String,
    /// Hash of this entry (over its fields + `prev_hash`).
    pub entry_hash: String,
}

/// The genesis hash that anchors the head of an empty chain.
const GENESIS: &str = "genesis";

fn hash_entry(seq: u64, timestamp: u64, principal: &str, action: &str, prev_hash: &str) -> String {
    let doc = serde_json::json!({
        "seq": seq,
        "timestamp": timestamp,
        "principal": principal,
        "action": action,
        "prev_hash": prev_hash,
    });
    let bytes = entelechy_artifacts::to_canonical_bytes(&doc);
    ArtifactId::from_canonical_bytes(&bytes).to_string()
}

/// A hash-chained, append-only audit log (PRD 17.2).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    /// External anchors (a past head hash committed outside the operator's
    /// control at intervals).
    anchors: Vec<String>,
}

impl AuditLog {
    /// Create an empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// The current head hash (last entry's hash, or genesis).
    pub fn head(&self) -> String {
        self.entries
            .last()
            .map(|e| e.entry_hash.clone())
            .unwrap_or_else(|| GENESIS.to_string())
    }

    /// Append an entry, chaining in the current head (PRD 17.2). Returns the new
    /// head hash.
    pub fn append(
        &mut self,
        principal: impl Into<String>,
        action: impl Into<String>,
        timestamp: u64,
    ) -> String {
        let seq = self.entries.len() as u64;
        let prev_hash = self.head();
        let principal = principal.into();
        let action = action.into();
        let entry_hash = hash_entry(seq, timestamp, &principal, &action, &prev_hash);
        self.entries.push(AuditEntry {
            seq,
            timestamp,
            principal,
            action,
            prev_hash,
            entry_hash: entry_hash.clone(),
        });
        entry_hash
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Recompute the chain from genesis and verify internal consistency (PRD
    /// 17.2). Any post-hoc edit to a past entry breaks this.
    pub fn verify_chain(&self) -> bool {
        let mut prev = GENESIS.to_string();
        for (i, e) in self.entries.iter().enumerate() {
            if e.seq != i as u64 || e.prev_hash != prev {
                return false;
            }
            let expected = hash_entry(e.seq, e.timestamp, &e.principal, &e.action, &e.prev_hash);
            if expected != e.entry_hash {
                return false;
            }
            prev = e.entry_hash.clone();
        }
        true
    }

    /// Record an external anchor of the current head (periodic anchoring outside
    /// the operator's control — PRD 17.2). Returns the anchored hash.
    pub fn anchor(&mut self) -> String {
        let head = self.head();
        self.anchors.push(head.clone());
        head
    }

    /// Verify that a previously-held anchor is still part of this chain's history
    /// (PRD 17.2). A rewrite that recomputes the chain produces different hashes,
    /// so an externally-held anchor no longer matches — detecting the tamper.
    pub fn verify_anchor(&self, anchor_hash: &str) -> bool {
        anchor_hash == GENESIS || self.entries.iter().any(|e| e.entry_hash == anchor_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log3() -> AuditLog {
        let mut l = AuditLog::new();
        l.append("owner", "approve GoalSpec sha256:aa", 100);
        l.append("policy", "deny refund", 101);
        l.append("approver", "promote L1->L2", 102);
        l
    }

    #[test]
    fn valid_chain_verifies() {
        let l = log3();
        assert_eq!(l.len(), 3);
        assert!(l.verify_chain());
    }

    #[test]
    fn tampering_breaks_the_chain() {
        let mut l = log3();
        // Rewrite a past entry's action without recomputing hashes.
        l.entries[1].action = "allow refund".into();
        assert!(!l.verify_chain());
    }

    #[test]
    fn external_anchor_detects_full_rewrite() {
        let mut l = log3();
        let anchor = l.anchor(); // hold this externally
        l.append("owner", "approve release", 103);
        // The anchor is still in history.
        assert!(l.verify_anchor(&anchor));

        // A full rewrite (different content) recomputes to different hashes, so the
        // externally-held anchor no longer matches any entry.
        let mut rewritten = AuditLog::new();
        rewritten.append("owner", "approve GoalSpec sha256:EVIL", 100);
        rewritten.append("policy", "deny refund", 101);
        rewritten.append("approver", "promote L1->L2", 102);
        assert!(rewritten.verify_chain()); // internally consistent...
        assert!(!rewritten.verify_anchor(&anchor)); // ...but fails the external anchor
    }
}
