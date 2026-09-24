//! AuthorityEnvelope — first-class authority shared by compiler, runtime, policy
//! and assurance (PRD 5.4).

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The maximum authority available to a synthesized system (PRD 5.4).
///
/// Invariants (PRD 5.4):
/// - **A1**: no design node may exercise authority outside this envelope.
/// - **A2**: `Delegate` nodes can only narrow authority ([`AuthorityEnvelope::is_subset_of`]).
/// - **A3**: authority changes require a new approved artifact (enforced at the
///   artifact/approval layer, not here).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityEnvelope {
    /// Capability names the system may exercise (e.g. `read_crm`, `create_draft`).
    pub capabilities: BTreeSet<String>,
    /// Explicitly forbidden capabilities (e.g. `refund`, `delete_account`). A
    /// forbidden capability can never be granted, even if also listed as allowed.
    pub forbidden_capabilities: BTreeSet<String>,
    /// Data scopes (tenant / project / document / row / field).
    pub data_scopes: BTreeSet<String>,
    /// Allowed network egress targets (hosts / egress classes).
    pub network_scopes: BTreeSet<String>,
    /// Named secret handles the system may reference (never prompt-visible).
    pub secret_scopes: BTreeSet<String>,
    /// Financial limits, in minor currency units, per accounting scope.
    pub financial_limit_minor: Option<u64>,
    /// Whether human approval is required before external/write/irreversible effects.
    pub approval_required_for_writes: bool,
}

impl AuthorityEnvelope {
    /// An empty envelope: no authority at all.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether `capability` is grantable under this envelope: listed as allowed
    /// and not forbidden. Forbidden always wins (PRD 5.4).
    pub fn allows_capability(&self, capability: &str) -> bool {
        !self.forbidden_capabilities.contains(capability) && self.capabilities.contains(capability)
    }

    /// Whether `self` is a subset of `parent`: every grant in `self` is present
    /// in `parent`, and `self` forbids at least everything `parent` forbids.
    /// This is the delegation-narrowing check for invariant A2 / IR-I6.
    pub fn is_subset_of(&self, parent: &Self) -> bool {
        self.capabilities.is_subset(&parent.capabilities)
            && self.data_scopes.is_subset(&parent.data_scopes)
            && self.network_scopes.is_subset(&parent.network_scopes)
            && self.secret_scopes.is_subset(&parent.secret_scopes)
            && parent
                .forbidden_capabilities
                .is_subset(&self.forbidden_capabilities)
            && match (self.financial_limit_minor, parent.financial_limit_minor) {
                // A child may not exceed the parent's financial ceiling.
                (Some(c), Some(p)) => c <= p,
                (Some(_), None) => true, // parent unbounded, child bounded: narrower
                (None, Some(_)) => false, // child unbounded under bounded parent: wider
                (None, None) => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(caps: &[&str]) -> AuthorityEnvelope {
        AuthorityEnvelope {
            capabilities: caps.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn forbidden_beats_allowed() {
        let mut e = env(&["refund"]);
        e.forbidden_capabilities.insert("refund".into());
        assert!(!e.allows_capability("refund"));
    }

    #[test]
    fn delegation_narrows() {
        let parent = env(&["read_crm", "create_draft"]);
        let child = env(&["read_crm"]);
        assert!(child.is_subset_of(&parent));
        assert!(!parent.is_subset_of(&child)); // widening is rejected
    }

    #[test]
    fn child_cannot_drop_parent_prohibitions() {
        let mut parent = env(&["read_crm"]);
        parent.forbidden_capabilities.insert("refund".into());
        let child = env(&["read_crm"]); // forgot to carry the prohibition
        assert!(!child.is_subset_of(&parent));
    }

    #[test]
    fn financial_ceiling_narrows() {
        let mut parent = env(&["pay"]);
        parent.financial_limit_minor = Some(10_000);
        let mut child = env(&["pay"]);
        child.financial_limit_minor = Some(5_000);
        assert!(child.is_subset_of(&parent));
        child.financial_limit_minor = None; // unbounded under a bounded parent
        assert!(!child.is_subset_of(&parent));
    }
}
