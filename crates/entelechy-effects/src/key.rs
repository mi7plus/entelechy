//! Operation keys (PRD 7.6).
//!
//! The key is derived from the run, the node path (including Map index and Loop
//! iteration) and the logical attempt. The logical attempt is stable across
//! crash recovery and advances only when policy retries after a confirmed
//! non-commit (PRD 7.6).

use serde::{Deserialize, Serialize};

/// A stable, idempotency-relevant operation key (PRD 7.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationKey {
    /// The run this effect belongs to.
    pub run_id: String,
    /// The node path, including Map index and Loop iteration (PRD 7.6).
    pub node_path: String,
    /// The logical attempt. Stable across crash recovery; only advances after a
    /// confirmed non-commit (PRD 7.6).
    pub logical_attempt: u32,
}

impl OperationKey {
    /// Create a first-attempt key.
    pub fn new(run_id: impl Into<String>, node_path: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            node_path: node_path.into(),
            logical_attempt: 0,
        }
    }

    /// The next attempt. Callers must only advance after a *confirmed*
    /// non-commit (PRD 7.6); an unknown commit goes to reconciliation instead.
    pub fn next_attempt(&self) -> Self {
        Self {
            logical_attempt: self.logical_attempt + 1,
            ..self.clone()
        }
    }

    /// The canonical string form embedded in provider requests where the API
    /// allows it (PRD 7.6 providers without idempotency support).
    pub fn as_string(&self) -> String {
        format!(
            "{}/{}/attempt{}",
            self.run_id, self.node_path, self.logical_attempt
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_string_includes_attempt() {
        let k = OperationKey::new("run-1", "root/2:map3/pay");
        assert_eq!(k.as_string(), "run-1/root/2:map3/pay/attempt0");
        assert_eq!(k.next_attempt().logical_attempt, 1);
    }
}
