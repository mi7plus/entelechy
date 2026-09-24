//! Append-only execution journal (PRD 5.2 EffectRecord, 8.4, RK-2).
//!
//! The runtime journals every model call, tool call, human decision, policy
//! decision, random draw and external effect (RK-2). Replay follows the recorded
//! order rather than racing (PRD 8.4). Entries are append-only; a run's terminal
//! status is derived from journal state so restart cannot invent a different
//! outcome (PRD 8.6).

use entelechy_gateway::{ModelRequest, ModelResponse};
use serde::{Deserialize, Serialize};

/// Commit status of a consequential effect (PRD 7.6). Only `Unknown` enters
/// reconciliation (PRD 8.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitStatus {
    /// The effect is known to have committed.
    Committed,
    /// The effect is known not to have committed.
    NotCommitted,
    /// Commit state is unknown; must be reconciled before the run completes.
    Unknown,
}

/// One journaled event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum JournalEvent {
    /// A model call and its response (RK-2). Model calls are egress (PRD 7.5).
    ModelCall {
        /// The request issued.
        request: ModelRequest,
        /// The response received.
        response: ModelResponse,
    },
    /// A tool effect: write-ahead intent, request hash and commit status
    /// (PRD 7.6, 8.5).
    ToolEffect {
        /// Capability invoked.
        capability: String,
        /// Operation key (idempotency — PRD 7.6).
        operation_key: String,
        /// Hash of the canonical request (PRD 7.6 journal entry).
        request_hash: String,
        /// The tool's response payload.
        response: serde_json::Value,
        /// Commit status (PRD 7.6).
        commit: CommitStatus,
    },
    /// A policy/gate decision (RK-2).
    PolicyDecision {
        /// Policy consulted.
        policy: String,
        /// Whether the gate opened.
        allowed: bool,
    },
}

/// A single append-only journal entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Monotonic sequence number within the run (recorded scheduling order).
    pub seq: u64,
    /// The node path that produced the event (includes Map/Loop indices).
    pub node_path: String,
    /// The event.
    pub event: JournalEvent,
}

/// An append-only journal for one run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    /// Run identifier.
    pub run_id: String,
    /// Ordered entries.
    pub entries: Vec<JournalEntry>,
}

impl Journal {
    /// Create an empty journal for a run.
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            entries: Vec::new(),
        }
    }

    /// Append an event, assigning the next sequence number.
    pub fn append(&mut self, node_path: impl Into<String>, event: JournalEvent) {
        let seq = self.entries.len() as u64;
        self.entries.push(JournalEntry {
            seq,
            node_path: node_path.into(),
            event,
        });
    }

    /// Whether any consequential effect has an unknown commit status, which
    /// forces the run into `reconciliation_required` (PRD 8.5, 8.6).
    pub fn has_unknown_commit(&self) -> bool {
        self.entries.iter().any(|e| {
            matches!(
                &e.event,
                JournalEvent::ToolEffect {
                    commit: CommitStatus::Unknown,
                    ..
                }
            )
        })
    }
}

/// A cursor for reading a journal back during replay, in recorded order
/// (PRD 8.4).
pub struct JournalReader<'a> {
    entries: &'a [JournalEntry],
    pos: usize,
}

impl<'a> JournalReader<'a> {
    /// Start reading at the first entry.
    pub fn new(journal: &'a Journal) -> Self {
        Self {
            entries: &journal.entries,
            pos: 0,
        }
    }

    /// Take the next entry if its node path matches, else error. Enforces that
    /// replay follows the recorded order (PRD 8.4).
    pub fn next_for(&mut self, node_path: &str) -> Result<&'a JournalEvent, ReplayMismatch> {
        let entry = self
            .entries
            .get(self.pos)
            .ok_or(ReplayMismatch::Exhausted)?;
        if entry.node_path != node_path {
            return Err(ReplayMismatch::Divergence {
                expected: entry.node_path.clone(),
                actual: node_path.to_string(),
                seq: entry.seq,
            });
        }
        self.pos += 1;
        Ok(&entry.event)
    }
}

/// A replay divergence: the point where re-execution differed from the record
/// (PRD 8.4, RK-8).
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReplayMismatch {
    /// The journal ran out of entries before the program finished.
    #[error("journal exhausted during replay")]
    Exhausted,
    /// The node reached during replay differs from the recorded node.
    #[error("replay diverged at seq {seq}: expected node '{expected}', got '{actual}'")]
    Divergence {
        /// Recorded node path.
        expected: String,
        /// Node path reached during replay.
        actual: String,
        /// Sequence number of the divergence.
        seq: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_monotonic_seq() {
        let mut j = Journal::new("run-1");
        j.append(
            "root/pay",
            JournalEvent::PolicyDecision {
                policy: "p".into(),
                allowed: true,
            },
        );
        j.append(
            "root/pay2",
            JournalEvent::PolicyDecision {
                policy: "p".into(),
                allowed: false,
            },
        );
        assert_eq!(j.entries[0].seq, 0);
        assert_eq!(j.entries[1].seq, 1);
    }

    #[test]
    fn reader_detects_divergence() {
        let mut j = Journal::new("run-1");
        j.append(
            "root/a",
            JournalEvent::PolicyDecision {
                policy: "p".into(),
                allowed: true,
            },
        );
        let mut r = JournalReader::new(&j);
        let err = r.next_for("root/b").unwrap_err();
        assert!(matches!(err, ReplayMismatch::Divergence { .. }));
    }
}
