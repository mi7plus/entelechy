//! Policy engine: authority checks and Gate evaluation at effect boundaries.
//!
//! PRD v11 references: section 8.3 (PolicyEngine), 5.4 (AuthorityEnvelope), 7.3
//! (Gate semantics), 7.5 (model egress & confidentiality), 17.4 (fallbacks), Q6
//! (a small native policy core defines the semantics and the adapter interface),
//! Q19 (provider approval).
//!
//! The PolicyEngine evaluates policy independently at effect boundaries and
//! records decisions (PRD 8.3). Enforcement happens outside model output
//! (principle 8). A small native core defines the semantics; Cedar/OPA are later
//! adapters behind [`PolicyEngine`] and never define the semantics (Q6).
#![forbid(unsafe_code)]

pub mod provider;

use serde::{Deserialize, Serialize};

use entelechy_ir::{AuthorityEnvelope, EffectClass, Taint};

pub use provider::ProviderApproval;

/// An obligation the caller must satisfy for an otherwise-allowed effect (PRD
/// 5.4 approval rules, 7.3 Gate semantics).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Obligation {
    /// Human approval is required before the effect (PRD 5.4).
    RequireApproval,
    /// A Gate must have sanitized the tainted input first (IR-I3).
    RequireGate,
    /// Two-person approval for an irreversible effect (PRD 5.4 irreversibility).
    RequireTwoPersonApproval,
}

/// A recorded policy decision (PRD 8.3: record decisions; 5.2 PolicySnapshot).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Whether the effect is authorized (subject to obligations).
    pub allowed: bool,
    /// Human-readable rationale.
    pub reason: String,
    /// Prerequisites the runtime must satisfy before the effect (PRD 5.4, 7.3).
    pub obligations: Vec<Obligation>,
    /// Policy snapshot version in force (PRD 5.2).
    pub snapshot_version: u32,
}

impl Decision {
    fn deny(reason: impl Into<String>, version: u32) -> Self {
        Self {
            allowed: false,
            reason: reason.into(),
            obligations: vec![],
            snapshot_version: version,
        }
    }
}

/// A request to authorize one effect at its boundary (PRD 8.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyRequest {
    /// Principal id attributed to the effect (PRD 5.8).
    pub principal: String,
    /// Capability being exercised.
    pub capability: String,
    /// Effect class of the capability (PRD 7.6).
    pub effect_class: EffectClass,
    /// Integrity label of the value driving the effect (IR-I3).
    pub taint: Taint,
    /// Whether an applicable Gate has already sanitized the input (PRD 7.3).
    pub gate_satisfied: bool,
    /// Egress host for network-scope checks, if this effect is network egress.
    pub egress_host: Option<String>,
    /// Data classification of the value entering an egress/model call (PRD 7.5).
    pub data_classification: Option<String>,
    /// The provider selected for an egress/model call (PRD 7.5, Q19).
    pub provider: Option<String>,
}

/// A versioned policy configuration in force for a run or release (PRD 5.2).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySnapshot {
    /// Snapshot version; any change creates a new version (PRD 5.2).
    pub version: u32,
    /// Provider approval policy (Q19).
    pub provider_approval: ProviderApproval,
}

/// The policy engine abstraction (PRD 8.3, Q6). Cedar/OPA adapters implement this
/// without redefining the semantics.
pub trait PolicyEngine {
    /// Evaluate an effect request against the authority and snapshot.
    fn evaluate(
        &self,
        authority: &AuthorityEnvelope,
        snapshot: &PolicySnapshot,
        req: &PolicyRequest,
    ) -> Decision;
}

/// The native policy core that defines the 1.x semantics (Q6).
#[derive(Clone, Debug, Default)]
pub struct NativePolicy;

impl NativePolicy {
    /// Create the native policy core.
    pub fn new() -> Self {
        Self
    }
}

impl PolicyEngine for NativePolicy {
    fn evaluate(
        &self,
        authority: &AuthorityEnvelope,
        snapshot: &PolicySnapshot,
        req: &PolicyRequest,
    ) -> Decision {
        let v = snapshot.version;

        // 1. Forbidden capability always loses (PRD 5.4).
        if authority.forbidden_capabilities.contains(&req.capability) {
            return Decision::deny(format!("capability '{}' is forbidden", req.capability), v);
        }
        // 2. Capability must be granted (A1 / IR-I2).
        if !authority.allows_capability(&req.capability) {
            return Decision::deny(
                format!("capability '{}' not in AuthorityEnvelope", req.capability),
                v,
            );
        }
        // 3. Network scope for egress (PRD 5.4 network scopes).
        if let Some(host) = &req.egress_host {
            if !authority.network_scopes.contains(host) {
                return Decision::deny(format!("egress host '{host}' not in network scopes"), v);
            }
        }
        // 4. Model/egress confidentiality routing (PRD 7.5, IR-I9, Q19).
        if matches!(req.effect_class, EffectClass::External) {
            if let Some(class) = &req.data_classification {
                let provider = req.provider.as_deref().unwrap_or("");
                if !snapshot.provider_approval.is_approved(class, provider) {
                    return Decision::deny(
                        format!(
                            "provider '{provider}' not approved for classification '{class}' (Q19)"
                        ),
                        v,
                    );
                }
            }
        }

        let mut obligations = Vec::new();

        // 5. Tainted input must have crossed a Gate before a privileged effect
        //    (IR-I3). Deny if not satisfied.
        if req.taint == Taint::Tainted && req.effect_class.is_consequential() && !req.gate_satisfied
        {
            return Decision::deny(
                "tainted value reaches a consequential effect without a Gate (IR-I3)",
                v,
            );
        }

        // 6. Approval obligations for consequential effects (PRD 5.4).
        if req.effect_class.is_consequential() && authority.approval_required_for_writes {
            obligations.push(Obligation::RequireApproval);
        }
        // 7. Irreversible effects escalate to two-person approval (PRD 5.4).
        if matches!(req.effect_class, EffectClass::Irreversible) {
            obligations.push(Obligation::RequireTwoPersonApproval);
        }

        Decision {
            allowed: true,
            reason: "authorized".into(),
            obligations,
            snapshot_version: v,
        }
    }
}

/// An append-only log of policy decisions recorded at effect boundaries (PRD 8.3).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DecisionLog {
    /// Recorded decisions, in order.
    pub entries: Vec<(String, Decision)>,
}

impl DecisionLog {
    /// Create an empty log.
    pub fn new() -> Self {
        Self::default()
    }
    /// Record a decision for a capability/effect.
    pub fn record(&mut self, capability: impl Into<String>, decision: Decision) {
        self.entries.push((capability.into(), decision));
    }
    /// Whether any recorded decision was a denial.
    pub fn any_denied(&self) -> bool {
        self.entries.iter().any(|(_, d)| !d.allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn authority() -> AuthorityEnvelope {
        AuthorityEnvelope {
            capabilities: BTreeSet::from(["draft_reply".to_string(), "send_email".to_string()]),
            forbidden_capabilities: BTreeSet::from(["refund".to_string()]),
            network_scopes: BTreeSet::from(["api.example.com".to_string()]),
            approval_required_for_writes: true,
            ..Default::default()
        }
    }

    fn req(cap: &str, class: EffectClass) -> PolicyRequest {
        PolicyRequest {
            principal: "svc".into(),
            capability: cap.into(),
            effect_class: class,
            taint: Taint::Trusted,
            gate_satisfied: false,
            egress_host: None,
            data_classification: None,
            provider: None,
        }
    }

    #[test]
    fn forbidden_capability_denied() {
        let d = NativePolicy.evaluate(&authority(), &PolicySnapshot::default(), &req("refund", EffectClass::Irreversible));
        assert!(!d.allowed);
    }

    #[test]
    fn ungranted_capability_denied() {
        let d = NativePolicy.evaluate(&authority(), &PolicySnapshot::default(), &req("delete_account", EffectClass::Write));
        assert!(!d.allowed);
    }

    #[test]
    fn write_requires_approval() {
        let d = NativePolicy.evaluate(&authority(), &PolicySnapshot::default(), &req("draft_reply", EffectClass::Write));
        assert!(d.allowed);
        assert!(d.obligations.contains(&Obligation::RequireApproval));
    }

    #[test]
    fn tainted_privileged_effect_without_gate_denied() {
        let mut r = req("draft_reply", EffectClass::Write);
        r.taint = Taint::Tainted;
        let d = NativePolicy.evaluate(&authority(), &PolicySnapshot::default(), &r);
        assert!(!d.allowed, "{d:?}");
        // With a gate satisfied it is allowed.
        r.gate_satisfied = true;
        assert!(NativePolicy.evaluate(&authority(), &PolicySnapshot::default(), &r).allowed);
    }

    #[test]
    fn egress_provider_must_be_approved() {
        let mut snap = PolicySnapshot { version: 2, provider_approval: ProviderApproval::new() };
        snap.provider_approval.approve("internal", "mock");
        let mut r = req("send_email", EffectClass::External);
        r.data_classification = Some("pii".into());
        r.provider = Some("mock".into());
        // pii is not approved for mock -> denied (Q19).
        assert!(!NativePolicy.evaluate(&authority(), &snap, &r).allowed);
        // internal is approved.
        r.data_classification = Some("internal".into());
        assert!(NativePolicy.evaluate(&authority(), &snap, &r).allowed);
    }

    #[test]
    fn egress_host_scope_enforced() {
        let mut r = req("send_email", EffectClass::External);
        r.egress_host = Some("evil.example.net".into());
        assert!(!NativePolicy.evaluate(&authority(), &PolicySnapshot::default(), &r).allowed);
    }
}
