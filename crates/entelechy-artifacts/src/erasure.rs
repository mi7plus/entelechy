//! Erasure via crypto-shredding, and its survival across restore (PRD 17, 17.3,
//! Q14, DR-2).
//!
//! Sensitive payloads are encrypted under per-tenant/per-subject keys; erasure
//! destroys the key (crypto-shredding) and leaves a tombstone, while metadata and
//! lineage remain intact (PRD 17). Replay of an affected run is marked
//! replay-incomplete, never silently diverging (Q14). Backups honor
//! crypto-shredding: a restore replays the erasure ledger so erased data never
//! reappears (DR-2).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// An append-only ledger of erased subjects/keys (PRD 17, DR-2). Erasing records
/// a tombstone; the plaintext is unrecoverable because its key is destroyed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureLedger {
    tombstones: BTreeSet<String>,
}

impl ErasureLedger {
    /// Create an empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Erase a subject/key (crypto-shredding): record a tombstone (PRD 17).
    pub fn erase(&mut self, subject_key: impl Into<String>) {
        self.tombstones.insert(subject_key.into());
    }

    /// Whether a subject/key has been erased.
    pub fn is_erased(&self, subject_key: &str) -> bool {
        self.tombstones.contains(subject_key)
    }

    /// Number of tombstones.
    pub fn len(&self) -> usize {
        self.tombstones.len()
    }

    /// Whether the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.tombstones.is_empty()
    }

    /// Replay an older (restored) ledger into this one so erasures survive restore
    /// (DR-2): the result is the union, so a backup can never resurrect erased
    /// data, and a newer erasure not present in the backup also stays erased.
    pub fn replay_restore(&mut self, restored: &ErasureLedger) {
        self.tombstones.extend(restored.tombstones.iter().cloned());
    }

    /// Whether replaying a run that touched any of `run_subject_keys` must be
    /// marked replay-incomplete because required data was erased (Q14).
    pub fn replay_incomplete_for(&self, run_subject_keys: &[String]) -> bool {
        run_subject_keys.iter().any(|k| self.is_erased(k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erasure_leaves_a_tombstone() {
        let mut l = ErasureLedger::new();
        l.erase("tenant-a/subject-1");
        assert!(l.is_erased("tenant-a/subject-1"));
        assert!(!l.is_erased("tenant-a/subject-2"));
    }

    #[test]
    fn restore_never_resurrects_erased_data() {
        // Current state has an erasure the backup predates.
        let mut current = ErasureLedger::new();
        current.erase("subject-new");
        // The backup (older) had a different erasure.
        let mut backup = ErasureLedger::new();
        backup.erase("subject-old");
        // Restoring replays the backup ledger: both stay erased (DR-2).
        current.replay_restore(&backup);
        assert!(current.is_erased("subject-old"));
        assert!(current.is_erased("subject-new"));
    }

    #[test]
    fn replay_is_incomplete_when_run_data_erased() {
        let mut l = ErasureLedger::new();
        l.erase("subject-1");
        assert!(l.replay_incomplete_for(&["subject-1".into(), "subject-2".into()]));
        assert!(!l.replay_incomplete_for(&["subject-2".into()]));
    }
}
