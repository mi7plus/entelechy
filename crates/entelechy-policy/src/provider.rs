//! Provider approval by data classification (PRD 7.5, Q19).
//!
//! Model calls are egress: a value with a confidentiality label may enter a
//! prompt only if the selected provider and region are approved for that label
//! (PRD 7.5, IR-I9). A signed ProviderApproval policy per data classification
//! lists the approved providers; above the internal classification, no provider
//! is approved until listed (PRD Q19).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Approved providers per data classification (PRD Q19). Maintained by the data
/// owner and a security approver together; versioned inside the PolicySnapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderApproval {
    /// classification -> approved provider identifiers.
    by_classification: BTreeMap<String, BTreeSet<String>>,
}

impl ProviderApproval {
    /// Empty policy: nothing approved (fail closed).
    pub fn new() -> Self {
        Self::default()
    }

    /// Approve a provider for a classification.
    pub fn approve(&mut self, classification: impl Into<String>, provider: impl Into<String>) {
        self.by_classification
            .entry(classification.into())
            .or_default()
            .insert(provider.into());
    }

    /// Whether `provider` is approved for `classification` (PRD Q19). Fails closed
    /// for any classification with no listed providers.
    pub fn is_approved(&self, classification: &str, provider: &str) -> bool {
        self.by_classification
            .get(classification)
            .is_some_and(|set| set.contains(provider))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlisted_classification_fails_closed() {
        let mut p = ProviderApproval::new();
        p.approve("internal", "mock");
        assert!(p.is_approved("internal", "mock"));
        // Above internal (e.g. pii): nothing approved until listed (Q19).
        assert!(!p.is_approved("pii", "mock"));
        assert!(!p.is_approved("internal", "other-provider"));
    }
}
