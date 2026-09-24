//! A Cedar-style permit/forbid policy adapter (PRD Q6).
//!
//! PRD Q6: "A small native policy core defines the semantics and the adapter
//! interface. Cedar is the first shipped adapter." This module demonstrates the
//! adapter boundary with a self-contained permit/forbid rule engine that mirrors
//! Cedar's evaluation semantics (explicit `forbid` overrides any `permit`, and
//! the default is deny). It plugs in behind the same [`crate::PolicyEngine`] trait
//! as the native core and **reuses the core semantics** (taint, egress routing,
//! obligations) — it only supplies the capability decision.
//!
//! A production integration of the real `cedar-policy` crate would be another
//! `PolicyEngine` impl behind this same trait; nothing else in the runtime changes.

use serde::{Deserialize, Serialize};

use entelechy_ir::AuthorityEnvelope;

use crate::{finish_decision, Decision, PolicyEngine, PolicyRequest, PolicySnapshot};

/// The effect of a rule (Cedar semantics: `forbid` overrides `permit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    /// Permit matching requests.
    Permit,
    /// Forbid matching requests (overrides any permit).
    Forbid,
}

/// A matcher over one request component (principal / action / resource).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Match {
    /// Matches anything.
    Any,
    /// Matches an exact value.
    Is(String),
}

impl Match {
    fn matches(&self, value: &str) -> bool {
        match self {
            Match::Any => true,
            Match::Is(v) => v == value,
        }
    }
}

/// A Cedar-style rule over (principal, action, resource) (PRD Q6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// permit or forbid.
    pub effect: Effect,
    /// Principal matcher.
    pub principal: Match,
    /// Action matcher (mapped from the capability being exercised).
    pub action: Match,
    /// Resource matcher (mapped from the data classification, else `default`).
    pub resource: Match,
}

impl Rule {
    fn applies(&self, principal: &str, action: &str, resource: &str) -> bool {
        self.principal.matches(principal)
            && self.action.matches(action)
            && self.resource.matches(resource)
    }
}

/// A permit/forbid policy adapter (PRD Q6). Evaluates rules with Cedar semantics
/// and defers all other semantics to the shared core via [`finish_decision`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RuleAdapter {
    /// The policy rules, evaluated together (order-independent; forbid wins).
    pub rules: Vec<Rule>,
}

impl RuleAdapter {
    /// Create an adapter from a rule set.
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    /// The capability (action) decision under Cedar semantics: allowed iff at
    /// least one `permit` matches and no `forbid` matches (default deny).
    fn capability_decision(&self, req: &PolicyRequest) -> Result<(), String> {
        let principal = req.principal.as_str();
        let action = req.capability.as_str();
        let resource = req.data_classification.as_deref().unwrap_or("default");

        let forbidden = self
            .rules
            .iter()
            .any(|r| r.effect == Effect::Forbid && r.applies(principal, action, resource));
        if forbidden {
            return Err(format!(
                "action '{action}' is forbidden by policy (forbid overrides permit)"
            ));
        }
        let permitted = self
            .rules
            .iter()
            .any(|r| r.effect == Effect::Permit && r.applies(principal, action, resource));
        if permitted {
            Ok(())
        } else {
            Err(format!(
                "no permit rule matches action '{action}' (default deny)"
            ))
        }
    }
}

impl PolicyEngine for RuleAdapter {
    fn evaluate(
        &self,
        authority: &AuthorityEnvelope,
        snapshot: &PolicySnapshot,
        req: &PolicyRequest,
    ) -> Decision {
        // Adapter supplies only the capability decision; the core semantics
        // (taint, egress routing, obligations) are shared (PRD Q6).
        finish_decision(self.capability_decision(req), authority, snapshot, req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entelechy_ir::{EffectClass, Taint};

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

    fn adapter() -> RuleAdapter {
        RuleAdapter::new(vec![
            Rule {
                effect: Effect::Permit,
                principal: Match::Any,
                action: Match::Is("draft_reply".into()),
                resource: Match::Any,
            },
            Rule {
                effect: Effect::Forbid,
                principal: Match::Any,
                action: Match::Is("refund".into()),
                resource: Match::Any,
            },
        ])
    }

    #[test]
    fn permit_rule_authorizes() {
        let d = adapter().evaluate(
            &AuthorityEnvelope::empty(),
            &PolicySnapshot::default(),
            &req("draft_reply", EffectClass::Write),
        );
        assert!(d.allowed, "{d:?}");
    }

    #[test]
    fn forbid_overrides_permit() {
        // Even with a broad permit, an explicit forbid wins (Cedar semantics).
        let mut a = adapter();
        a.rules.push(Rule {
            effect: Effect::Permit,
            principal: Match::Any,
            action: Match::Any,
            resource: Match::Any,
        });
        let d = a.evaluate(
            &AuthorityEnvelope::empty(),
            &PolicySnapshot::default(),
            &req("refund", EffectClass::Irreversible),
        );
        assert!(!d.allowed);
    }

    #[test]
    fn default_deny() {
        let d = adapter().evaluate(
            &AuthorityEnvelope::empty(),
            &PolicySnapshot::default(),
            &req("delete_account", EffectClass::Write),
        );
        assert!(!d.allowed);
    }

    #[test]
    fn adapter_still_honors_core_taint_semantics() {
        // Permitted capability, but a tainted value without a gate is still denied
        // by the shared core (IR-I3) — the adapter does not bypass semantics.
        let mut r = req("draft_reply", EffectClass::Write);
        r.taint = Taint::Tainted;
        let d = adapter().evaluate(&AuthorityEnvelope::empty(), &PolicySnapshot::default(), &r);
        assert!(!d.allowed, "{d:?}");
    }
}
